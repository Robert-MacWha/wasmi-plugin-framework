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

use crate::rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse};

pub struct TransportDriver<R, W> {
    reader: Arc<Mutex<BufReader<R>>>,
    writer: Arc<Mutex<W>>,
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
    EOF,

    #[error("Unmatched response ID")]
    UnmatchedResponseId,

    #[error("Error Response: {0:?}")]
    ErrorResponse(RpcErrorResponse),

    #[error("Missing Response")]
    MissingResponse,
}

impl From<TransportError> for RpcError {
    fn from(value: TransportError) -> Self {
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

    pub async fn run(self) -> Result<(), TransportError> {
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
    ) -> Result<RpcResponse, TransportError> {
        let id = self.next_id();
        let request = RpcMessage::request(id, method.to_string(), params);

        let (recv_tx, revc_rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, recv_tx);
        self.write_message(&request)?;

        // Await the response
        let res = revc_rx.await?;
        let res = res.map_err(TransportError::ErrorResponse)?;
        Ok(res)
    }

    /// Synchronous call that sends a request and waits for the response. Handles any
    /// unrelated incoming messages while waiting.
    pub fn call(&self, method: &str, params: Value) -> Result<RpcResponse, TransportError> {
        let resps = self.call_many(std::iter::once((method, params)))?;
        let resp = resps
            .into_iter()
            .next()
            .ok_or(TransportError::MissingResponse)?;
        let resp = resp.map_err(TransportError::ErrorResponse)?;
        Ok(resp)
    }

    /// Synchronous call that sends multiple requests and waits for all responses. Handles
    /// any unrelated incoming messages while waiting.
    pub fn call_many<I, S>(
        &self,
        calls: I,
    ) -> Result<Vec<Result<RpcResponse, RpcErrorResponse>>, TransportError>
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        //? Send all requests & collect their IDs
        let mut receivers = Vec::new();
        let mut ids = Vec::new();
        for (method, params) in calls {
            let id = self.next_id();
            let (tx, rx) = oneshot::channel();

            self.pending.lock().unwrap().insert(id, tx);
            self.write_message(&RpcMessage::request(id, method.into(), params))?;

            receivers.push(rx);
            ids.push(id);
        }

        let mut results = Vec::new();
        let mut reader = self.reader.lock().unwrap();
        let mut line = String::new();

        //? Drive IO while any responses are still missing
        while !receivers.is_empty() {
            self.step(&mut reader, &mut line)?;

            //? Check for completed responses and collect them, dropping the
            //? corresponding receivers
            for i in (0..receivers.len()).rev() {
                let rx = &mut receivers[i];
                match rx.try_recv() {
                    Ok(Some(res)) => {
                        results.push(res);
                        let _ = receivers.swap_remove(i);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err(TransportError::OneshotCanceled(e));
                    }
                }
            }
        }

        //? Sort into original order
        results.sort_by_key(|r| match r {
            Ok(res) => res.id,
            Err(err) => err.id,
        });

        Ok(results)
    }

    /// Perform a single read step, processing one incoming message if available.
    /// Returns immediately if no message is available.
    fn step(
        &self,
        reader: &mut std::sync::MutexGuard<'_, BufReader<R>>,
        line: &mut String,
    ) -> Result<(), TransportError> {
        match reader.read_line(line) {
            Ok(0) => return Err(TransportError::EOF),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(TransportError::Io(e)),
        }

        let msg: RpcMessage = serde_json::from_str(line)?;
        line.clear();

        self.insert_response(msg)?;
        Ok(())
    }

    fn write_message(&self, message: &RpcMessage) -> Result<(), TransportError> {
        let message_str = serde_json::to_string(message)?;
        let msg = format!("{}\n", message_str);
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(msg.as_bytes())?;
            writer.flush()?;
        }

        Ok(())
    }

    fn insert_response(&self, message: RpcMessage) -> Result<(), TransportError> {
        match message {
            RpcMessage::RpcResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    info!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender
                    .send(Ok(res))
                    .map_err(|_| TransportError::OneshotSend)?;
            }
            RpcMessage::RpcErrorResponse(res) => {
                let sender = self.pending.lock().unwrap().remove(&res.id);
                let Some(sender) = sender else {
                    info!("Ignoring unmatched response ID: {}", res.id);
                    return Ok(());
                };

                sender
                    .send(Err(res))
                    .map_err(|_| TransportError::OneshotSend)?;
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
