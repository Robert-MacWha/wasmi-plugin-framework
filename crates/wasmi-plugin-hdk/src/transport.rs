use futures::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, io::BufReader};
use serde_json::Value;
use thiserror::Error;
use tracing::{error, info, warn};
use wasmi_plugin_pdk::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};
use wasmi_plugin_rt::yield_now;

pub struct Transport<R, W> {
    reader: BufReader<R>,
    writer: W,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serde error: {0}, data: {1}")]
    Serde(serde_json::Error, String),

    #[error("EOF")]
    Eof,

    #[error("Error Response: {0:?}")]
    ErrorResponse(RpcErrorResponse),
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Transport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Transport {
            reader: BufReader::new(reader),
            writer,
        }
    }

    pub async fn call(
        mut self,
        method: &str,
        params: Value,
        handler: Option<impl RequestHandler<RpcError>>,
    ) -> Result<RpcResponse, TransportError> {
        let id = 1;
        let request = RpcMessage::request(id, method.to_string(), params);
        info!("Sending request: {:?}", request);
        self.write_message(request).await?;

        let mut line = String::new();

        loop {
            match self.reader.read_line(&mut line).await {
                Ok(0) => {
                    warn!("EOF reached while reading from transport");
                    return Err(TransportError::Eof);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(e) => return Err(TransportError::Io(e)),
            }

            // Process complete lines
            let msg: RpcMessage =
                serde_json::from_str(&line).map_err(|e| TransportError::Serde(e, line.clone()))?;
            line.clear();

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

    async fn write_message(&mut self, msg: RpcMessage) -> Result<(), TransportError> {
        let msg: String = serde_json::to_string(&msg).map_err(|e| {
            error!("Serde error while serializing message: {}", e);
            TransportError::Serde(e, "".to_string())
        })?;
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
