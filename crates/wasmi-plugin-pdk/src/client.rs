use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    sync::{Mutex, atomic::AtomicU64},
};

use serde_json::Value;
use std::sync::atomic::Ordering;
use tracing::info;
use wasmi_plugin_rt::yield_now;

use crate::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcRequest, RpcResponse, to_rpc_err},
};

pub struct Client<R: Read + 'static = std::io::Stdin, W: Write + 'static = std::io::Stdout> {
    reader: Mutex<BufReader<R>>,
    writer: Mutex<W>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Result<RpcResponse, RpcErrorResponse>>>,
}

impl Client<std::io::Stdin, std::io::Stdout> {
    pub fn new() -> Self {
        Self {
            reader: Mutex::new(BufReader::new(std::io::stdin())),
            writer: Mutex::new(std::io::stdout()),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }
}

impl<R: Read + 'static, W: Write + 'static> Client<R, W> {
    pub fn new_with(reader: R, writer: W) -> Self {
        Self {
            reader: Mutex::new(BufReader::new(reader)),
            writer: Mutex::new(writer),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Send a single call and wait for the response.
    pub fn call(&mut self, method: &str, params: Value) -> Result<RpcResponse, RpcError> {
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
    pub fn call_many<I, S>(&mut self, calls: I) -> Result<Vec<RpcResponse>, RpcError>
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

    fn write(&mut self, id: u64, method: &str, params: Value) -> Result<(), RpcError> {
        let request = RpcMessage::RpcRequest(crate::rpc_message::RpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        });

        self.write_message(&request)
    }

    fn write_message(&mut self, message: &RpcMessage) -> Result<(), RpcError> {
        let message_str = serde_json::to_string(message).map_err(to_rpc_err)?;
        let msg = format!("{}\n", message_str);
        {
            let mut writer = self.writer.lock().unwrap();
            writer.write_all(msg.as_bytes()).map_err(to_rpc_err)?;
            writer.flush().map_err(to_rpc_err)?;
        }

        Ok(())
    }

    fn read(&mut self) -> Result<RpcMessage, RpcError> {
        let mut line = String::new();

        //? Loop until we successfully read a line or hit EOF, continuing on WouldBlock
        //? errors.
        loop {
            match self.reader.lock().unwrap().read_line(&mut line) {
                Ok(0) => return Err(RpcError::custom("EOF reached while reading from transport")),
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    return Err(RpcError::custom(&format!(
                        "Failed to read from transport: {}",
                        e
                    )));
                }
            }
        }

        Ok(serde_json::from_str(&line).map_err(to_rpc_err)?)
    }

    /// Inserts a response message into the pending map. If the message is a request,
    /// it is ignored.
    fn insert_response(&mut self, message: RpcMessage) {
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

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl<R: Read + 'static, W: Write + 'static> Client<R, W> {
    /// Asynchronously send a single call and wait for the response. This allows
    /// other async tasks to run while waiting for the response.
    pub async fn async_call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<RpcResponse, RpcError> {
        let id = self.next_id();
        self.write(id, method, params)?;

        loop {
            let message = self.async_read().await?;
            self.insert_response(message);

            if let Some(result) = self.pending.lock().unwrap().remove(&id) {
                return result.map_err(|e| to_rpc_err(e.error));
            }
        }
    }

    /// Asynchronously send a single call and wait for the response, while also handling
    /// incoming requests using the provided handler.
    ///
    /// Used primarily by hosts to handle requests from guests while waiting for responses.
    ///
    /// ```#no_run
    /// host   ->   guest: balance()
    ///   host <-   guest: balanceOf(1)
    ///   host  ->> guest: 42
    ///   host <-   guest: balanceOf(2)
    ///   host  ->> guest: 100
    /// host   <<-  guest: (42, 100)
    /// ```
    pub async fn async_call_with_handler(
        &mut self,
        method: &str,
        params: Value,
        handler: impl RequestHandler<RpcError>,
    ) -> Result<RpcResponse, RpcError> {
        let id = self.next_id();
        self.write(id, method, params)?;

        loop {
            let message = self.async_read().await?;
            match message {
                RpcMessage::RpcRequest(RpcRequest {
                    jsonrpc,
                    id,
                    method,
                    params,
                }) => match handler.handle(&method, params).await {
                    Ok(result) => {
                        self.write_message(&RpcMessage::response(&jsonrpc, id, result))?;
                    }
                    Err(error) => {
                        self.write_message(&RpcMessage::error_response(&jsonrpc, id, error))?;
                    }
                },
                _ => self.insert_response(message),
            }

            if let Some(result) = self.pending.lock().unwrap().remove(&id) {
                return result.map_err(|e| to_rpc_err(e.error));
            }
        }
    }

    async fn async_read(&mut self) -> Result<RpcMessage, RpcError> {
        let mut line = String::new();

        loop {
            match self.reader.lock().unwrap().read_line(&mut line) {
                Ok(0) => return Err(RpcError::custom("EOF reached while reading from transport")),
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    yield_now().await;
                    continue;
                }
                Err(e) => {
                    return Err(RpcError::custom(&format!(
                        "Failed to read from transport: {}",
                        e
                    )));
                }
            }
        }

        Ok(serde_json::from_str(&line).map_err(to_rpc_err)?)
    }
}
