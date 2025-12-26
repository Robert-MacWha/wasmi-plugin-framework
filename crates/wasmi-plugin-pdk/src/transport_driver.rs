use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex, atomic::AtomicU64},
};

use futures::channel::oneshot;
use serde_json::Value;
use thiserror::Error;
use tracing::info;
use wasmi_plugin_rt::yield_now;

use crate::{
    poll_oneoff::wait_for_stdin,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};

pub struct TransportDriver<R, W> {
    reader: Arc<Mutex<BufReader<R>>>,
    writer: Arc<Mutex<W>>,
    #[allow(clippy::type_complexity)]
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<RpcResponse, RpcErrorResponse>>>>>,
    next_id: Arc<AtomicU64>,
}

impl<R, W> Clone for TransportDriver<R, W> {
    fn clone(&self) -> Self {
        Self {
            reader: self.reader.clone(),
            writer: self.writer.clone(),
            pending: self.pending.clone(),
            next_id: self.next_id.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum DriverError {
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

    #[error("Unmatched response ID")]
    UnmatchedResponseId,

    #[error("Error Response: {0:?}")]
    ErrorResponse(RpcErrorResponse),

    #[error("Missing Response")]
    MissingResponse,
}

impl From<DriverError> for RpcError {
    fn from(value: DriverError) -> Self {
        RpcError::custom(value.to_string())
    }
}

impl<R: Read, W: Write> TransportDriver<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: Arc::new(Mutex::new(BufReader::new(reader))),
            writer: Arc::new(Mutex::new(writer)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn run(&mut self) -> Result<(), DriverError> {
        let mut line = String::new();

        loop {
            self.step(&mut self.reader.lock().unwrap(), &mut line)?;
            yield_now().await;
        }
    }

    /// Asynchronous call that sends a request and waits for the response.
    pub async fn call_async(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcResponse, DriverError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);

        let (recv_tx, revc_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, recv_tx);
        self.write_message(&request)?;

        // Await the response
        let res = revc_rx.await?;
        let res = res.map_err(DriverError::ErrorResponse)?;
        Ok(res)
    }

    /// Synchronous call that sends a request and waits for the response. Handles any
    /// unrelated incoming messages while waiting.
    pub fn call(&self, method: &str, params: Value) -> Result<RpcResponse, DriverError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);

        let (res_tx, mut res_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, res_tx);
        self.write_message(&request)?;

        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();

        // Drive IO until we get the response
        loop {
            let would_block = self.step(&mut reader, &mut line)?;

            // Check if we have received the response
            match res_rx.try_recv() {
                Ok(Some(res)) => {
                    let res = res.map_err(DriverError::ErrorResponse)?;
                    return Ok(res);
                }
                Ok(None) => {
                    if would_block {
                        wait_for_stdin();
                    }
                }
                Err(e) => {
                    return Err(DriverError::OneshotCanceled(e));
                }
            }
        }
    }

    /// Perform a single read step, processing one incoming message if available.
    /// Returns Ok(true) if no message was available (WouldBlock) or Ok(false)
    /// if a message was processed.
    fn step(
        &self,
        reader: &mut std::sync::MutexGuard<'_, BufReader<R>>,
        line: &mut String,
    ) -> Result<bool, DriverError> {
        match reader.read_line(line) {
            Ok(0) => return Err(DriverError::Eof),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
            Err(e) => return Err(DriverError::Io(e)),
        }

        let msg: RpcMessage = serde_json::from_str(line)?;
        line.clear();

        self.insert_response(msg)?;
        Ok(false)
    }

    // TODO: Catch WouldBlock and retry later
    pub fn write_message(&self, message: &RpcMessage) -> Result<(), DriverError> {
        let message_str = serde_json::to_string(message)?;
        let msg = format!("{}\n", message_str);
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(msg.as_bytes())?;
            writer.flush()?;
        }

        Ok(())
    }

    fn insert_response(&self, message: RpcMessage) -> Result<(), DriverError> {
        match message {
            RpcMessage::RpcResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    info!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender.send(Ok(res)).map_err(|_| DriverError::OneshotSend)?;
            }
            RpcMessage::RpcErrorResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    info!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender
                    .send(Err(res))
                    .map_err(|_| DriverError::OneshotSend)?;
            }
            RpcMessage::RpcRequest(_) => {
                info!("Ignoring request message");
            }
        }

        Ok(())
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
