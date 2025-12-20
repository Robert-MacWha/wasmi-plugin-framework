use std::io::{BufWriter, Write};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use tokio::runtime::Builder;

use crate::{
    api::MaybeSend,
    client::Client,
    router::Router,
    rpc_message::{RpcError, RpcErrorResponse, RpcMessage, RpcResponse},
};

pub struct Server {
    router: Router<Client>,
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
            router: Router::default(),
        }
    }

    /// Registers a method handler with the server.
    pub fn with_method<P, R, F, Fut>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn(&mut Client, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, RpcError>> + MaybeSend + 'static,
    {
        self.router = self.router.with_method(name, func);
        self
    }

    pub fn run(&self) {
        let x = self.try_run();

        if let Err(e) = x {
            eprintln!("Plugin server error: {}", e);
            std::process::exit(1);
        }
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
        let rt = Builder::new_current_thread().enable_time().build().unwrap();
        let local = tokio::task::LocalSet::new();

        let resp = rt.block_on(local.run_until(async move {
            let mut client = Client::new();
            self.router
                .handle_with_state(&mut client, &request.method, request.params)
                .await
        }));

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
