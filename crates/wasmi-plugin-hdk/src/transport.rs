use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use serde_json::Value;
use thiserror::Error;
use tracing::info;
use wasmi_plugin_pdk::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};
use wasmi_plugin_rt::yield_now;

pub struct Transport<R, W> {
    reader: R,
    writer: W,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("EOF")]
    Eof,

    #[error("Error Response: {0:?}")]
    ErrorResponse(RpcErrorResponse),
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Transport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Transport { reader, writer }
    }

    pub async fn call(
        mut self,
        method: &str,
        params: Value,
        handler: Option<impl RequestHandler<RpcError>>,
    ) -> Result<RpcResponse, TransportError> {
        let id = 1;
        let request = RpcMessage::request(id, method.to_string(), params);
        self.write_message(request).await?;

        let mut buffer = Vec::new();
        let mut temp = [0u8; 256];

        loop {
            match self.reader.read(&mut temp).await {
                Ok(0) => return Err(TransportError::Eof),
                Ok(n) => {
                    buffer.extend_from_slice(&temp[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(e) => return Err(TransportError::Io(e)),
            }

            // Process complete lines
            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&buffer[..pos]).to_string();
                buffer.drain(..=pos);

                let msg: RpcMessage = serde_json::from_str(&line)?;

                match msg {
                    RpcMessage::RpcResponse(resp) if resp.id == id => return Ok(resp),
                    RpcMessage::RpcErrorResponse(err) if err.id == id => {
                        return Err(TransportError::ErrorResponse(err));
                    }
                    RpcMessage::RpcRequest(req) => {
                        self.handle_guest_req(req.id, &req.method, req.params, &handler)
                            .await?;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn write_message(&mut self, msg: RpcMessage) -> Result<(), TransportError> {
        let msg: String = serde_json::to_string(&msg)?;
        let msg = format!("{}\n", msg);
        self.writer.write_all(msg.as_bytes()).await?;
        self.writer.flush().await?;

        Ok(())
    }

    async fn handle_guest_req(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
        handler: &Option<impl RequestHandler<RpcError>>,
    ) -> Result<(), TransportError> {
        let Some(handler) = handler else {
            info!("No handler, request ignored");
            return Ok(());
        };

        match handler.handle(method, params).await {
            Ok(result) => {
                self.write_message(RpcMessage::response(id, result)).await?;
            }
            Err(error) => {
                self.write_message(RpcMessage::error_response(id, error))
                    .await?;
            }
        }

        Ok(())
    }
}
