use std::io::{BufRead, BufReader};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::select;
use tracing::error;

use crate::{
    router::{MaybeSend, Router},
    rpc_message::{RpcError, RpcErrorContext, RpcMessage, RpcRequest},
    transport::Transport,
};

/// A one-shot executor for plugins. Reads the host request from stdin,
/// dispatches it to the appropriate handler, and writes the response
/// to stdout.
pub struct PluginRunner {
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

    #[error(transparent)]
    Rpc(#[from] RpcError),

    #[error("Transport error: {0}")]
    Transport(#[from] crate::transport::TransportError),
}

#[allow(clippy::new_without_default)]
impl PluginRunner {
    pub fn new() -> Self {
        let router = Router::new();
        Self { router }
    }

    /// Register a RPC method handler with the runner. The handler function
    /// will be invoked if the request matches the given method name.
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

    /// Run the plugin, blocking the current thread until the incoming
    /// request has been handled.
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

        let (transport, driver) = Transport::new(std::io::stdin(), std::io::stdout());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .context("Error creating tokio runtime")?;

        let resp = rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(async {
                select! {
                    res = self.router.handle_with_state(transport, &request.method, request.params) => res,
                    drive_err = driver.run() => {
                        match drive_err {
                            Ok(_) => Err(RpcError::custom("Transport driver exited unexpectedly")),
                            Err(e) => Err(e.into()),
                        }
                    }
                }
            }).await
        });

        //? Send response
        let resp = match resp {
            Ok(result) => RpcMessage::response(request.id, result),
            Err(error) => RpcMessage::error_response(request.id, error),
        };
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
