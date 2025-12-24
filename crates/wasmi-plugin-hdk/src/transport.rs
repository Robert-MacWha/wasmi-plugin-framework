use std::collections::HashMap;

use futures::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, channel::oneshot, io::BufReader,
};
use serde_json::Value;
use thiserror::Error;
use tracing::info;
use wasmi_plugin_pdk::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcRequest, RpcResponse},
};

pub struct Transport<R, W> {
    reader: BufReader<R>,
    writer: W,
    pending: HashMap<u64, oneshot::Sender<Result<RpcResponse, RpcErrorResponse>>>,
    next_id: u64,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("EOF")]
    EOF,

    #[error("Error Response: {0:?}")]
    ErrorResponse(RpcErrorResponse),

    #[error("Cancelled")]
    Cancelled(#[from] oneshot::Canceled),
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Transport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Transport {
            reader: BufReader::new(reader),
            writer,
            pending: HashMap::new(),
            next_id: 0,
        }
    }

    pub async fn call(
        mut self,
        method: &str,
        params: Value,
        handler: Option<impl RequestHandler<RpcError>>,
    ) -> Result<RpcResponse, TransportError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);
        self.write_message(request).await?;

        let (resp_tx, mut resp_rx) = oneshot::channel();
        self.pending.insert(id, resp_tx);

        let mut line = String::new();
        loop {
            match self.reader.read_line(&mut line).await {
                Ok(0) => return Err(TransportError::EOF),
                Ok(_) => {}
                Err(e) => return Err(TransportError::Io(e)),
            }

            let msg: RpcMessage = serde_json::from_str(&line)?;
            line.clear();
            self.handle_incoming(msg, &handler).await?;

            if let Some(res) = resp_rx.try_recv()? {
                let resp = res.map_err(TransportError::ErrorResponse)?;
                return Ok(resp);
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

    async fn handle_incoming(
        &mut self,
        msg: RpcMessage,
        handler: &Option<impl RequestHandler<RpcError>>,
    ) -> Result<(), TransportError> {
        match msg {
            RpcMessage::RpcResponse(resp) => {
                self.pending.remove(&resp.id).map(|tx| tx.send(Ok(resp)));
            }
            RpcMessage::RpcErrorResponse(err) => {
                self.pending.remove(&err.id).map(|tx| tx.send(Err(err)));
            }
            RpcMessage::RpcRequest(RpcRequest {
                id, method, params, ..
            }) => {
                let Some(handler) = handler else {
                    info!("No handler, request ignored");
                    return Ok(());
                };

                match handler.handle(&method, params).await {
                    Ok(result) => {
                        self.write_message(RpcMessage::response(id, result)).await?;
                    }
                    Err(error) => {
                        self.write_message(RpcMessage::error_response(id, error))
                            .await?;
                    }
                }
            }
        }

        Ok(())
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}
