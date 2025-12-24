use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::Value;
use tracing::info;
use wasmi_plugin_rt::yield_now;

use crate::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcRequest, RpcResponse, to_rpc_err},
};

/// Single-threaded, concurrent-safe json-rpc transport layer
///
/// TODO: Create async version that accepts async readers/writers and yields properly
/// For the client-side, considering setting up a centralized "reactor" to handle all reads.
/// Then we can have the transport send requests to the reactor and await responses via channels.
/// If the main executor notices that everything but the reactor is blocked, it can yield via
/// poll_oneoff to free up the web worker and not mindlessly spin.
#[derive(Clone)]
pub struct Transport {
    id: Arc<AtomicU64>,
    reader: Arc<Mutex<Box<dyn BufRead + Send + Sync>>>,
    writer: Arc<Mutex<Box<dyn Write + Send + Sync>>>,

    pending: Arc<Mutex<HashMap<u64, Result<RpcResponse, RpcErrorResponse>>>>,
}

impl Transport {
    pub fn new(
        reader: impl Read + Send + Sync + 'static,
        writer: impl Write + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader: Arc::new(Mutex::new(Box::new(BufReader::new(reader)))),
            writer: Arc::new(Mutex::new(Box::new(writer))),
        }
    }
}

impl Transport {
    /// Send a single call and wait for the response.
    pub fn call(&self, method: &str, params: Value) -> Result<RpcResponse, RpcError> {
        let responses = self.call_many(std::iter::once((method, params)))?;
        let resp = responses
            .into_iter()
            .next()
            .ok_or(RpcError::custom("Response does not exist"))?;

        Ok(resp)
    }

    /// Send multiple calls and wait for all responses. This allows async-like
    /// behavior because the host will process requests concurrently.
    ///
    /// The order of responses matches the order of requests. If any call fails,
    /// the entire batch will fail.
    pub fn call_many<I, S>(&self, calls: I) -> Result<Vec<RpcResponse>, RpcError>
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        let ids: Result<Vec<u64>, RpcError> = calls
            .into_iter()
            .map(|(method, params)| {
                let id = self.next_id();
                self.write(id, &method.into(), params)?;
                Ok(id)
            })
            .collect();
        let ids = ids?;

        let mut results = Vec::with_capacity(ids.len());
        let mut remaining: HashSet<u64> = ids.iter().copied().collect();

        while !remaining.is_empty() {
            let message = self.read()?;
            self.insert_response(message);

            //? Retrieve any fulfilled requests
            let mut fulfilled = Vec::new();
            for &id in &remaining {
                if let Some(result) = self.pending.lock().unwrap().remove(&id) {
                    results.push(result.map_err(|e| to_rpc_err(e.error))?);
                    fulfilled.push(id);
                }
            }

            for id in fulfilled {
                remaining.remove(&id);
            }
        }

        results.sort_by_key(|r| r.id);

        Ok(results)
    }

    fn write(&self, id: u64, method: &str, params: Value) -> Result<(), RpcError> {
        let request = RpcMessage::RpcRequest(crate::rpc_message::RpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        });

        self.write_message(&request)
    }

    fn write_message(&self, message: &RpcMessage) -> Result<(), RpcError> {
        let message_str = serde_json::to_string(message).map_err(to_rpc_err)?;
        let msg = format!("{}\n", message_str);
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(msg.as_bytes()).map_err(to_rpc_err)?;
            writer.flush().map_err(to_rpc_err)?;
        }

        Ok(())
    }

    /// Blocking read of a single message
    pub fn read(&self) -> Result<RpcMessage, RpcError> {
        let mut line = String::new();

        //? Loop until we successfully read a line or hit EOF, continuing on WouldBlock
        //? errors.
        loop {
            let res = self.reader.lock().unwrap().read_line(&mut line);
            match res {
                Ok(0) => return Err(RpcError::custom("EOF reached while reading from transport")),
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    return Err(RpcError::custom(format!(
                        "Failed to read from transport: {}",
                        e
                    )));
                }
            }
        }

        serde_json::from_str(&line).map_err(to_rpc_err)
    }

    /// Inserts a response message into the pending map. If the message is a request,
    /// it is ignored.
    fn insert_response(&self, message: RpcMessage) {
        match message {
            RpcMessage::RpcResponse(resp) => {
                self.pending.lock().unwrap().insert(resp.id, Ok(resp));
            }
            RpcMessage::RpcErrorResponse(err_resp) => {
                self.pending
                    .lock()
                    .unwrap()
                    .insert(err_resp.id, Err(err_resp));
            }
            RpcMessage::RpcRequest(_) => {
                info!("Ignoring request message");
            }
        }
    }

    fn next_id(&self) -> u64 {
        self.id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Transport {
    /// Asynchronously send a single call and wait for the response. This allows
    /// other async tasks to run while waiting for the response.
    pub async fn async_call(&self, method: &str, params: Value) -> Result<RpcResponse, RpcError> {
        self.async_call_impl(method, params, None).await
    }

    /// Asynchronously send a single call and wait for the response, while also handling
    /// incoming requests using the provided handler.
    ///
    /// Used primarily by hosts to handle requests from guests while waiting for responses.
    ///
    /// ```text
    /// host   ->   guest: balance()
    ///   host <-   guest: balanceOf(1)
    ///   host  ->> guest: 42
    ///   host <-   guest: balanceOf(2)
    ///   host  ->> guest: 100
    /// host   <<-  guest: (42, 100)
    /// ```
    pub async fn async_call_with_handler(
        &self,
        method: &str,
        params: Value,
        handler: &dyn RequestHandler<RpcError>,
    ) -> Result<RpcResponse, RpcError> {
        self.async_call_impl(method, params, Some(handler)).await
    }

    async fn async_call_impl(
        &self,
        method: &str,
        params: Value,
        handler: Option<&dyn RequestHandler<RpcError>>,
    ) -> Result<RpcResponse, RpcError> {
        let id = self.next_id();
        self.write(id, method, params)?;

        loop {
            let message = self.async_read().await?;
            match message {
                RpcMessage::RpcRequest(RpcRequest {
                    id, method, params, ..
                }) => {
                    let Some(handler) = handler else {
                        info!("Ignoring request message as no handler is provided");
                        continue;
                    };

                    match handler.handle(&method, params).await {
                        Ok(result) => {
                            self.write_message(&RpcMessage::response(id, result))?;
                        }
                        Err(error) => {
                            self.write_message(&RpcMessage::error_response(id, error))?;
                        }
                    }
                }
                _ => self.insert_response(message),
            }

            //? If our request has been fulfilled, return the result
            if let Some(result) = self.pending.lock().unwrap().remove(&id) {
                return result.map_err(|e| to_rpc_err(e.error));
            }
        }
    }

    /// Async read of a single message, yielding when the reader would block
    async fn async_read(&self) -> Result<RpcMessage, RpcError> {
        let mut line = String::new();

        loop {
            let res = self.reader.lock().unwrap().read_line(&mut line);
            match res {
                Ok(0) => return Err(RpcError::custom("EOF reached while reading from transport")),
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(e) => {
                    return Err(RpcError::custom(format!(
                        "Failed to read from transport: {}",
                        e
                    )));
                }
            }
        }

        serde_json::from_str(&line).map_err(to_rpc_err)
    }
}
