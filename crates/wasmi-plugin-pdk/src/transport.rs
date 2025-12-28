use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex, atomic::AtomicU64},
};

use futures::channel::oneshot;
use serde_json::Value;
use thiserror::Error;
use tracing::{info, warn};
use wasmi_plugin_rt::yield_now;

use crate::{
    poll_oneoff::wait_for_stdin,
    router::MaybeSend,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};

pub trait SyncTransport<E>: Send + Sync + 'static {
    fn call(&self, method: &str, params: Value) -> Result<RpcResponse, E>;
}

pub trait AsyncTransport<E>: Send + Sync + 'static {
    fn call_async(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<RpcResponse, E>> + MaybeSend;
}

pub struct Transport<R = std::io::Stdin, W = std::io::Stdout> {
    inner: Arc<TransportInner<R, W>>,
}

pub struct TransportDriver<R = std::io::Stdin, W = std::io::Stdout> {
    inner: Arc<TransportInner<R, W>>,
}

struct TransportInner<R, W> {
    reader: Mutex<BufReader<R>>,
    writer: Mutex<W>,
    #[allow(clippy::type_complexity)]
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<RpcResponse, RpcErrorResponse>>>>,
    next_id: AtomicU64,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde_json error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("oneshot send error")]
    OneshotSend,

    #[error("oneshot Canceled")]
    OneshotCanceled(#[from] oneshot::Canceled),

    #[error("EOF")]
    Eof,

    #[error(transparent)]
    RpcError(#[from] RpcError),
}

impl From<TransportError> for RpcError {
    fn from(err: TransportError) -> Self {
        RpcError::Custom(err.to_string())
    }
}

impl<R, W> Clone for Transport<R, W> {
    fn clone(&self) -> Self {
        Transport {
            inner: self.inner.clone(),
        }
    }
}

impl<R: Read, W: Write> Transport<R, W> {
    pub fn new(reader: R, writer: W) -> (Self, TransportDriver<R, W>) {
        let inner = Arc::new(TransportInner {
            reader: Mutex::new(BufReader::new(reader)),
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
        });

        (
            Transport {
                inner: inner.clone(),
            },
            TransportDriver { inner },
        )
    }
}

impl SyncTransport<TransportError> for Transport {
    fn call(&self, method: &str, params: Value) -> Result<RpcResponse, TransportError> {
        self.inner.call(method, params)
    }
}

impl AsyncTransport<TransportError> for Transport {
    async fn call_async(&self, method: &str, params: Value) -> Result<RpcResponse, TransportError> {
        self.inner.call_async(method, params).await
    }
}

impl<R: Read, W: Write> TransportDriver<R, W> {
    pub async fn run(&self) -> Result<(), TransportError> {
        self.inner.run().await
    }

    pub fn write_message(&self, message: &RpcMessage) -> Result<(), TransportError> {
        self.inner.write_message(message)
    }
}

impl<R: Read, W: Write> TransportInner<R, W> {
    async fn run(&self) -> Result<(), TransportError> {
        let mut line = String::new();

        loop {
            self.step(&mut self.reader.lock().unwrap(), &mut line)?;
            yield_now().await;
        }
    }

    /// Synchronously call a method, blocking until the response is received.
    ///
    /// This method takes over the current thread to drive IO until the response
    /// is received. If responses to other requests (those made asynchronously) arrive
    /// in the meantime, they will be processed and queued for delivery.
    fn call(&self, method: &str, params: Value) -> Result<RpcResponse, TransportError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);

        let (res_tx, mut res_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, res_tx);
        self.write_message(&request)?;

        //? Lock reader since we'll be driving IO blockingly
        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();

        // Drive IO until we get our response
        loop {
            let would_block = self.step(&mut reader, &mut line)?;

            match res_rx.try_recv() {
                Ok(Some(res)) => {
                    let res = res.map_err(|e| TransportError::RpcError(e.error))?;
                    return Ok(res);
                }
                Ok(None) => {
                    if would_block {
                        wait_for_stdin();
                    }
                }
                Err(e) => {
                    return Err(TransportError::OneshotCanceled(e));
                }
            }
        }
    }

    /// Asynchronously call a method, returning a future that resolves
    /// to the response.
    ///
    /// Multiple concurrent async calls can be made, and responses will be
    /// matched to requests by ID.
    async fn call_async(&self, method: &str, params: Value) -> Result<RpcResponse, TransportError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);

        let (recv_tx, revc_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, recv_tx);
        self.write_message(&request)?;

        // Await the response
        let res = revc_rx.await?;
        let res = res.map_err(|e| TransportError::RpcError(e.error))?;
        Ok(res)
    }

    /// Perform a single read, processing one incoming message if available.
    /// Returns `Ok(true)` if no message was available (WouldBlock) or `Ok(false)`
    /// if a message was processed.
    fn step(
        &self,
        reader: &mut std::sync::MutexGuard<'_, BufReader<R>>,
        line: &mut String,
    ) -> Result<bool, TransportError> {
        match reader.read_line(line) {
            Ok(0) => return Err(TransportError::Eof),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
            Err(e) => return Err(TransportError::Io(e)),
        }

        let msg: RpcMessage = serde_json::from_str(line)?;
        line.clear();

        self.handle_message(msg)?;
        Ok(false)
    }

    // TODO: Catch WouldBlock
    pub fn write_message(&self, message: &RpcMessage) -> Result<(), TransportError> {
        let message_str = serde_json::to_string(message)?;
        let msg = format!("{}\n", message_str);
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(msg.as_bytes())?;
            writer.flush()?;
        }

        Ok(())
    }

    fn handle_message(&self, message: RpcMessage) -> Result<(), TransportError> {
        match message {
            RpcMessage::RpcResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    warn!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender
                    .send(Ok(res))
                    .map_err(|_| TransportError::OneshotSend)?;
            }
            RpcMessage::RpcErrorResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    warn!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender
                    .send(Err(res))
                    .map_err(|_| TransportError::OneshotSend)?;
            }
            RpcMessage::RpcRequest(req) => {
                info!("Ignoring request message");
                self.write_message(&RpcMessage::error_response(
                    req.id,
                    RpcError::custom("Cannot handle requests"),
                ))?;
            }
        }

        Ok(())
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
