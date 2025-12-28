use std::{fmt::Display, sync::Arc, time::Duration};

use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::info;
use uuid::Uuid;
use wasmi_plugin_pdk::{
    api::RequestHandler,
    router::BoxFuture,
    rpc_message::{RpcError, RpcResponse},
    transport::AsyncTransport,
};

use crate::compile::Compiled;
use crate::host_handler::HostHandler;
use crate::runtime;
use crate::runtime::Runtime;
use crate::time::sleep;
use crate::transport::{Transport, TransportError};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(Uuid);

impl Default for PluginId {
    fn default() -> Self {
        PluginId(Uuid::new_v4())
    }
}

impl PluginId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<u128> for PluginId {
    fn from(value: u128) -> Self {
        PluginId(Uuid::from_u128(value))
    }
}

impl Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{}", self.0) // full: {:#}
        } else {
            let uuid_str = self.0.as_simple().to_string();
            write!(f, "{}", &uuid_str[uuid_str.len() - 6..]) // short {}
        }
    }
}

type Logger = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Plugin is an async-capable instance of a plugin
pub struct Plugin {
    name: String,
    id: PluginId,
    handler: Arc<dyn HostHandler>,
    logger: Logger,
    compiled: Compiled,
    timeout: Duration,
    #[allow(dead_code)]
    // TODO: Use fuel to terminate native plugins
    max_fuel: Option<u64>,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("Transport error")]
    TransportError(#[from] TransportError),
    #[error("Bridge Error")]
    BridgeError(#[from] Box<dyn std::error::Error>),
    #[error("Plugin timeout")]
    PluginTimeout,
}

impl Plugin {
    pub async fn new(
        name: &str,
        wasm_bytes: &[u8],
        handler: Arc<dyn HostHandler>,
    ) -> Result<Self, wasmer::CompileError> {
        Ok(Plugin {
            name: name.to_string(),
            id: PluginId::new(),
            handler,
            logger: Box::new(default_plugin_logger),
            max_fuel: None,
            timeout: Duration::from_secs(10),
            compiled: Compiled::new(name, wasm_bytes).await?,
        })
    }

    /// Sets the plugin ID for this instance. If no ID is provided a random
    /// UUID is generated.
    pub fn with_id(mut self, id: PluginId) -> Self {
        self.id = id;
        self
    }

    /// Sets a custom logger for the plugin instance. Plugins log messages to
    /// stderr, which are captured and passed to this logger function. If
    /// no logger is provided a default logger is used.
    pub fn with_logger(mut self, logger: Logger) -> Self {
        self.logger = Box::new(logger);
        self
    }

    /// Sets a timeout duration for plugin calls. If a call takes longer than
    /// this duration, it will be terminated and an error will be returned.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> PluginId {
        self.id
    }
}

impl AsyncTransport<PluginError> for Plugin {
    async fn call_async(&self, method: &str, params: Value) -> Result<RpcResponse, PluginError> {
        self.call(method, params).await
    }
}

impl Plugin {
    pub async fn call(&self, method: &str, params: Value) -> Result<RpcResponse, PluginError> {
        let runtime = self.create_runtime();
        let (id, stdin_writer, stdout_reader) = runtime
            .spawn(self.compiled.clone())
            .await
            .map_err(|e| PluginError::BridgeError(e.into()))?;

        let handler = PluginCallback {
            handler: self.handler.clone(),
            uuid: self.id,
        };
        let transport = Transport::new(stdout_reader, stdin_writer, handler);

        let transport_task = transport.call_async(method, params).fuse();
        let timeout = sleep(self.timeout).fuse();
        futures::pin_mut!(transport_task, timeout);

        futures::select! {
            res = transport_task => {
                Ok(res?)
            },
            _ = timeout => {
                runtime.terminate(id).await;
                Err(PluginError::PluginTimeout)
            }
        }
    }

    #[allow(clippy::type_complexity)]
    #[cfg(not(target_arch = "wasm32"))]
    fn create_runtime(&self) -> impl Runtime + Send + Sync + 'static {
        runtime::NativeRuntime::new()
    }

    #[allow(clippy::type_complexity)]
    #[cfg(target_arch = "wasm32")]
    fn create_runtime(&self) -> impl Runtime + Send + Sync + 'static {
        runtime::WorkerRuntime::new()
    }
}

struct PluginCallback {
    handler: Arc<dyn HostHandler>,
    uuid: PluginId,
}

impl RequestHandler<RpcError> for PluginCallback {
    fn handle<'a>(&'a self, method: &str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let method = method.to_string();
        Box::pin(async move { self.handler.handle(self.uuid, &method, params).await })
    }
}

fn default_plugin_logger(name: &str, msg: &str) {
    info!(target: "plugin", "[plugin] [{}] {}", name, msg);
}
