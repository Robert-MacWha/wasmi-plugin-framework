use std::sync::Arc;

use futures::{
    AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, FutureExt, SinkExt, StreamExt,
    io::BufReader, stream::FuturesUnordered,
};
use serde_json::Value;
use thiserror::Error;
use tracing::{error, warn};
use wasmi_plugin_pdk::{
    api::RequestHandler,
    router::BoxFuture,
    rpc_message::{RpcError, RpcMessage, RpcResponse},
};
use wasmi_plugin_rt::yield_now;

/// A session for communicating with oneshot plugin over an async transport.
pub struct PluginSession<R, W, H> {
    reader: R,
    writer: W,
    handler: Arc<H>,
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
    Rpc(#[from] RpcError),
    #[error("Channel closed")]
    ChannelClosed,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, H: RequestHandler<RpcError>>
    PluginSession<R, W, H>
{
    /// Create a new PluginSession with the given reader, writer, and request handler.
    pub fn new(reader: R, writer: W, handler: H) -> Self {
        PluginSession {
            reader,
            writer,
            handler: Arc::new(handler),
        }
    }

    /// Calls a method asynchronously on the plugin and waits for the response.
    pub async fn call_async(
        self,
        method: &str,
        params: Value,
    ) -> Result<RpcResponse, PluginSessionError> {
        let (inbound_tx, inbound_rx) = futures::channel::mpsc::unbounded();
        let (outbound_tx, outbound_rx) = futures::channel::mpsc::unbounded();

        //? Send initial request
        let id = 1;
        let request = RpcMessage::request(id, method.to_string(), params);
        outbound_tx.unbounded_send(request).unwrap();

        let reader_fut = reader_task(self.reader, inbound_tx).fuse();
        let writer_fut = writer_task(self.writer, outbound_rx).fuse();
        let dispatcher_fut =
            dispatcher_task(self.handler.clone(), inbound_rx, outbound_tx, id).fuse();

        futures::pin_mut!(reader_fut, writer_fut, dispatcher_fut);

        futures::select_biased! {
            result = dispatcher_fut => result,
            reader_result = reader_fut => {
                reader_result?;
                //? If reader task ends, it means the pipe has closed. However,
                //? the final response might still be in flight in the dispatcher
                //? task, so we wait for it to complete. If the pipe did EOF without
                //? sending a response, the dispatcher will eventually starve
                //? and EOF itself.
                dispatcher_fut.await
            }
            writer_result = writer_fut => {
                writer_result?;
                Err(PluginSessionError::ChannelClosed)
            }
        }
    }
}

async fn reader_task<R: AsyncRead + Unpin>(
    reader: R,
    tx: futures::channel::mpsc::UnboundedSender<RpcMessage>,
) -> Result<(), PluginSessionError> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                //? EOF reached. Returning Ok allows the dispatcher to finish
                //? processing any pending messages.
                return Ok(());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                yield_now().await;
                continue;
            }
            Err(e) => return Err(PluginSessionError::Io(e)),
        }

        let msg: RpcMessage =
            serde_json::from_str(&line).map_err(|e| PluginSessionError::Serde(e, line.clone()))?;
        tx.unbounded_send(msg)
            .map_err(|_| PluginSessionError::ChannelClosed)?;
    }
}

async fn writer_task<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: futures::channel::mpsc::UnboundedReceiver<RpcMessage>,
) -> Result<(), PluginSessionError> {
    while let Some(msg) = rx.next().await {
        let msg: String = serde_json::to_string(&msg).map_err(|e| {
            error!("Serde error while serializing message: {}", e);
            PluginSessionError::Serde(e, "".to_string())
        })?;

        let msg = format!("{}\n", msg);
        writer.write_all(msg.as_bytes()).await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn dispatcher_task<H: RequestHandler<RpcError>>(
    handler: Arc<H>,
    mut inbound: futures::channel::mpsc::UnboundedReceiver<RpcMessage>,
    mut outbound: futures::channel::mpsc::UnboundedSender<RpcMessage>,
    awaiting_id: u64,
) -> Result<RpcResponse, PluginSessionError> {
    let mut pending: FuturesUnordered<BoxFuture<'_, (u64, Result<Value, RpcError>)>> =
        FuturesUnordered::new();

    loop {
        futures::select_biased! {
            // Complete handler
            (id, result) = pending.select_next_some() => {
                let msg = match result {
                        Ok(value) => RpcMessage::response(id, value),
                        Err(error) => RpcMessage::error_response(id, error),
                    };
                    outbound.send(msg).await.map_err(|_| PluginSessionError::ChannelClosed)?;
            },

            // Inbound message
            msg = inbound.next() => {
                let Some(msg) = msg else {
                    return Err(PluginSessionError::ChannelClosed);
                };

                match msg {
                    RpcMessage::RpcResponse(resp) if resp.id == awaiting_id => return Ok(resp),
                    RpcMessage::RpcErrorResponse(err) if err.id == awaiting_id => return Err(err.error.into()),
                    RpcMessage::RpcRequest(req) => {
                        let handler = handler.clone();
                        let fut = async move {
                            let result = handler.handle(&req.method, req.params).await;
                            (req.id, result)
                        };
                        pending.push(Box::pin(fut));
                    },
                     _ => {
                        warn!("Unexpected message: {:?}", msg);
                    }
                }
            }
        }
    }
}
