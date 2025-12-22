use std::io::{Read, Write};
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
    transport::Transport,
};
use wasmi_plugin_rt::sleep;

use crate::bridge::{self, Bridge};
#[cfg(not(target_arch = "wasm32"))]
use crate::compile::Compiled;
use crate::host_handler::HostHandler;

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
    max_fuel: Option<u64>,
    #[cfg(not(target_arch = "wasm32"))]
    compiled: Compiled,
    wasm_bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum PluginError {
    // #[error("spawn error")]
    // SpawnError(#[from] SpawnError),
    #[error("transport error")]
    RpcError(#[from] RpcError),
    #[error("plugin died")]
    PluginDied,
}

impl Plugin {
    pub fn new(
        name: &str,
        wasm_bytes: Vec<u8>,
        handler: Arc<dyn HostHandler>,
    ) -> Result<Self, wasmer::CompileError> {
        Ok(Plugin {
            name: name.to_string(),
            id: PluginId::new(),
            handler,
            logger: Box::new(default_plugin_logger),
            max_fuel: None,
            #[cfg(not(target_arch = "wasm32"))]
            compiled: Compiled::new(&wasm_bytes)?,
            wasm_bytes,
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

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> PluginId {
        self.id
    }
}

impl PluginError {
    pub fn as_rpc_code(&self) -> RpcError {
        match self {
            PluginError::RpcError(code) => code.clone(),
            _ => RpcError::Custom(self.to_string()),
        }
    }
}

impl From<PluginError> for RpcError {
    fn from(err: PluginError) -> Self {
        match err {
            PluginError::RpcError(code) => code,
            _ => RpcError::Custom(err.to_string()),
        }
    }
}

impl Plugin {
    pub async fn call(&self, method: &str, params: Value) -> Result<RpcResponse, PluginError> {
        //? Construct URL from worker bytes
        let (bridge, stdout_reader, stdin_writer) = self.create_bridge().unwrap();

        let transport = Transport::new(stdout_reader, stdin_writer);
        let handler = PluginCallback {
            handler: self.handler.clone(),
            uuid: self.id,
        };

        let transport_task = transport
            .async_call_with_handler(method, params, &handler)
            .fuse();
        let timeout = sleep(Duration::from_secs(60)).fuse();
        futures::pin_mut!(transport_task, timeout);

        futures::select! {
            res = transport_task => {
                bridge.terminate();
                return Ok(res?);
            },
            _ = timeout => {
                bridge.terminate();
                return Err(PluginError::PluginDied);
            }
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn create_bridge(
        &self,
    ) -> Result<
        (
            Box<dyn Bridge + Send + Sync>,
            Box<dyn Read + Send + Sync>,
            Box<dyn Write + Send + Sync>,
        ),
        PluginError,
    > {
        let (bridge, stdout, stdin) = bridge::NativeBridge::new(&self.name, self.compiled.clone())
            .map_err(|_| PluginError::PluginDied)?;
        Ok((Box::new(bridge), Box::new(stdout), Box::new(stdin)))
    }

    #[cfg(target_arch = "wasm32")]
    fn create_bridge(
        &self,
    ) -> Result<
        (
            Box<dyn Bridge + Send + Sync>,
            Box<dyn Read + Send + Sync>,
            Box<dyn Write + Send + Sync>,
        ),
        PluginError,
    > {
        // Web uses the bytes to send to the Worker
        let (bridge, stdout, stdin) = bridge::WorkerBridge::new(&self.name, &self.wasm_bytes)
            .map_err(|_| PluginError::PluginDied)?;
        Ok((Box::new(bridge), Box::new(stdout), Box::new(stdin)))
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
