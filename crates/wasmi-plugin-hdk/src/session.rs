use futures::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, io::BufReader};
use serde_json::Value;
use thiserror::Error;
use tracing::{error, info, warn};
use wasmi_plugin_pdk::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcMessage, RpcResponse},
};
use wasmi_plugin_rt::yield_now;

/// A session for communicating with oneshot plugin over an async transport.
pub struct PluginSession<R, W, H> {
    reader: BufReader<R>,
    writer: W,
    handler: H,
}

#[derive(Debug, Error)]
pub enum PluginSessionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serde error: {0}, data: {1}")]
    Serde(serde_json::Error, String),
    #[error("EOF")]
    Eof,
    #[error(transparent)]
    Rpc(RpcError),
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, H: RequestHandler<RpcError>>
    PluginSession<R, W, H>
{
    /// Create a new PluginSession with the given reader, writer, and request handler.
    pub fn new(reader: R, writer: W, handler: H) -> Self {
        PluginSession {
            reader: BufReader::new(reader),
            writer,
            handler,
        }
    }

    /// Calls a method asynchronously on the plugin and waits for the response.
    pub async fn call_async(
        mut self,
        method: &str,
        params: Value,
    ) -> Result<RpcResponse, PluginSessionError> {
        let id = 1;
        let request = RpcMessage::request(id, method.to_string(), params);
        self.write_message(request).await?;

        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line).await {
                Ok(0) => {
                    warn!("EOF reached while reading from transport");
                    return Err(PluginSessionError::Eof);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(e) => return Err(PluginSessionError::Io(e)),
            }
            info!("PluginSession: Received line: {}", line.trim());

            // Process complete lines
            let msg: RpcMessage = serde_json::from_str(&line)
                .map_err(|e| PluginSessionError::Serde(e, line.clone()))?;

            match msg {
                RpcMessage::RpcResponse(resp) if resp.id == id => return Ok(resp),
                RpcMessage::RpcErrorResponse(err) if err.id == id => {
                    return Err(PluginSessionError::Rpc(err.error));
                }
                RpcMessage::RpcRequest(req) => {
                    self.handle_request(req.id, &req.method, req.params).await?;
                }
                _ => {
                    warn!("Unexpected message received: {:?}", msg);
                    continue;
                }
            }
        }
    }

    async fn write_message(&mut self, msg: RpcMessage) -> Result<(), PluginSessionError> {
        let msg: String = serde_json::to_string(&msg).map_err(|e| {
            error!("Serde error while serializing message: {}", e);
            PluginSessionError::Serde(e, "".to_string())
        })?;
        let msg = format!("{}\n", msg);
        self.writer.write_all(msg.as_bytes()).await?;
        self.writer.flush().await?;

        Ok(())
    }

    async fn handle_request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<(), PluginSessionError> {
        match self.handler.handle(method, params).await {
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
