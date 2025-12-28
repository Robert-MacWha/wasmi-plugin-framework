use std::io::{BufRead, BufReader};

use futures::{FutureExt, select};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tracing::{error, info};

use crate::{
    router::{MaybeSend, Router},
    rpc_message::{RpcError, RpcMessage, RpcRequest},
    transport::Transport,
};

/// Server is a RPC server that can handle requests by dispatching them to registered
/// handler functions. It stores a shared state `S` that is passed into each handler.
pub struct PluginServer {
    router: Router<Transport>,
}

#[derive(Debug, Error)]
enum PluginServerError {
    #[error("Invalid Message: {0:?}")]
    InvalidMessage(RpcMessage),

    #[error("serde_json error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("RPC error: {0}")]
    Rpc(#[from] RpcError),

    #[error("Transport error: {0}")]
    Transport(#[from] crate::transport::TransportError),
}

#[allow(clippy::new_without_default)]
impl PluginServer {
    pub fn new() -> Self {
        let router = Router::new();
        Self { router }
    }

    pub fn with_method<P, R, F, Fut>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn(Transport, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, RpcError>> + MaybeSend + 'static,
    {
        self.router = self.router.with_method(name, func);
        self
    }

    pub fn run(self) {
        match self.try_run() {
            Ok(()) => {}
            Err(e) => {
                error!("PluginServer encountered an error: {:?}", e);
                std::process::exit(1);
            }
        }
    }

    fn try_run(&self) -> Result<(), PluginServerError> {
        //? Read request
        let request = self.read_request()?;
        info!("PluginServer: Received request: {:?}", request);

        let (transport, mut driver) = Transport::new(std::io::stdin(), std::io::stdout());
        let resp = futures::executor::block_on(async {
            select! {
                res = self.router.handle_with_state(transport, &request.method, request.params).fuse() => {
                    res
                },
                drive_err = driver.run().fuse() => {
                    panic!("Transport driver exited unexpectedly: {:?}", drive_err);
                }
            }
        });

        //? Send response
        let resp = match resp {
            Ok(result) => RpcMessage::response(request.id, result),
            Err(error) => RpcMessage::error_response(request.id, error),
        };
        info!("PluginServer: Sending response: {:?}", resp);
        driver.write_message(&resp)?;

        Ok(())
    }

    fn read_request(&self) -> Result<RpcRequest, PluginServerError> {
        let mut reader = BufReader::new(std::io::stdin());
        let mut line = String::new();
        loop {
            match reader.read_line(&mut line) {
                Ok(_) if !line.is_empty() => break,
                Ok(_) => continue, // EOF or empty read
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        let msg: RpcMessage = serde_json::from_str(&line)?;
        let RpcMessage::RpcRequest(msg) = msg else {
            return Err(PluginServerError::InvalidMessage(msg));
        };
        Ok(msg)
    }
}
