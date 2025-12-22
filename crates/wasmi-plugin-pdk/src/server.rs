use std::io::Write;

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use tokio::runtime::Builder;

use crate::{
    router::{MaybeSend, Router},
    rpc_message::{RpcError, RpcMessage},
    transport::Transport,
};

/// Server is a RPC server that can handle requests by dispatching them to registered
/// handler functions. It stores a shared state `S` that is passed into each handler.
pub struct PluginServer {
    router: Router<Transport>,
}

#[derive(Debug, Error)]
enum PluginServerError {
    #[error("Missing Request")]
    MissingRequest,

    #[error("Invalid Message: {0:?}")]
    InvalidMessage(RpcMessage),

    #[error("serde_json error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

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
                eprintln!("Plugin server error: {}", e);
                std::process::exit(1);
            }
        }
        // let transport = self.transport.clone();

        // let rt = Builder::new_current_thread().enable_time().build().unwrap();
        // let local = tokio::task::LocalSet::new();

        // rt.block_on(local.run_until(async move {
        //     let _ = transport.process_next_line(Some(&self)).await;
        // }));
    }

    fn try_run(&self) -> Result<(), PluginServerError> {
        //? Read request
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|_| PluginServerError::MissingRequest)?;

        let message: RpcMessage = serde_json::from_str(&line)?;
        let RpcMessage::RpcRequest(request) = message else {
            return Err(PluginServerError::InvalidMessage(message));
        };

        //? Dispatch handler
        let rt = Builder::new_current_thread().enable_time().build()?;
        let local = tokio::task::LocalSet::new();

        let resp = rt.block_on(local.run_until(async move {
            let transport = Transport::new(std::io::stdin(), std::io::stdout());
            self.router
                .handle_with_state(transport, &request.method, request.params)
                .await
        }));

        //? Send response
        let resp = match resp {
            Ok(result) => RpcMessage::response(request.id, result),
            Err(error) => RpcMessage::error_response(request.id, error),
        };

        let serialized = serde_json::to_string(&resp)?;
        let msg = format!("{}\n", serialized);
        std::io::stdout().write_all(msg.as_bytes())?;
        std::io::stdout().flush()?;

        Ok(())
    }
}
