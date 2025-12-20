use std::{
    collections::HashMap,
    io::{BufWriter, Write},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

use crate::{
    client::Client,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};

pub struct Server {
    methods: HashMap<String, Box<dyn Fn(&mut Client, Value) -> Result<Value, RpcError>>>,
}

#[derive(Debug, Error)]
pub enum PluginServerError {
    #[error("Missing Request")]
    MissingRequest,

    #[error("Method Not Found")]
    MethodNotFound,

    #[error("Invalid Message: {0:?}")]
    InvalidMessage(RpcMessage),

    #[error("serde_json error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl Server {
    pub fn new() -> Self {
        Self {
            methods: HashMap::new(),
        }
    }

    /// Registers a method handler with the server.
    pub fn with_method<P, R, F>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn(&mut Client, P) -> Result<R, RpcError> + 'static,
    {
        let f = Box::new(
            move |client: &mut Client, params: Value| -> Result<Value, RpcError> {
                let parsed = serde_json::from_value(params);
                let Ok(p) = parsed else {
                    return Err(RpcError::InvalidParams);
                };

                let result = func(client, p)?;
                Ok(serde_json::to_value(result).unwrap())
            },
        );

        self.methods.insert(name.to_string(), f);
        self
    }

    pub fn run(&self) {
        self.try_run().unwrap();
    }

    /// Runs the server, blocking the current thread to process a single request.
    fn try_run(&self) -> Result<(), PluginServerError> {
        let reader = std::io::stdin();
        let mut writer = BufWriter::new(std::io::stdout());

        //? Read request
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|_| PluginServerError::MissingRequest)?;

        let message: RpcMessage = serde_json::from_str(&line)?;
        let RpcMessage::RpcRequest(request) = message else {
            return Err(PluginServerError::InvalidMessage(message));
        };

        //? Dispatch handler
        let handler = self
            .methods
            .get(&request.method)
            .ok_or(PluginServerError::MethodNotFound)?;

        let mut client = Client::new();
        let resp = handler(&mut client, request.params);

        //? Send response
        let resp = match resp {
            Ok(result) => RpcMessage::RpcResponse(RpcResponse {
                jsonrpc: "2.0".into(),
                id: request.id,
                result,
            }),
            Err(error) => RpcMessage::RpcErrorResponse(RpcErrorResponse {
                jsonrpc: "2.0".into(),
                id: request.id,
                error,
            }),
        };

        let serialized = serde_json::to_string(&resp)?;
        let msg = format!("{}\n", serialized);
        writer.write_all(msg.as_bytes())?;
        writer.flush()?;

        Ok(())
    }
}
