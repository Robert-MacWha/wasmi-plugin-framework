use std::{sync::Arc, time::Duration};

use futures::{AsyncBufReadExt, AsyncRead, FutureExt, future::FusedFuture, io::BufReader};
use serde_json::Value;
use thiserror::Error;
use tracing::info;
use wasmi_plugin_pdk::{
    api::RequestHandler,
    router::BoxFuture,
    rpc_message::{RpcError, RpcResponse},
    transport::AsyncTransport,
};

use crate::host_handler::HostHandler;
use crate::runtime;
use crate::runtime::Runtime;
use crate::session::{PluginSession, PluginSessionError};
use crate::time::sleep;
use crate::{compile::Compiled, plugin_id::PluginId};

/// Plugin is an async-capable instance of a plugin
#[derive(Clone)]
pub struct Plugin {
    inner: Arc<InnerPlugin>,
}

pub struct PluginBuilder {
    id: PluginId,
    name: String,
    handler: Arc<dyn HostHandler>,
    logger: Arc<dyn PluginLogger>,
    wasm_bytes: Vec<u8>,
    timeout: Duration,
}

struct InnerPlugin {
    id: PluginId,
    handler: Arc<dyn HostHandler>,
    logger: Arc<dyn PluginLogger>,
    compiled: Compiled,
    timeout: Duration,
}

pub trait PluginLogger: Send + Sync + 'static {
    fn log(&self, name: &str, message: &str) {
        info!(target: "plugin", "[plugin] [{}] {}", name, message);
    }
}

struct DefaultPluginLogger;
impl PluginLogger for DefaultPluginLogger {}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("Session error")]
    SessionError(#[from] PluginSessionError),
    #[error("Runtime Error")]
    RuntimeError(#[from] Box<dyn std::error::Error>),
    #[error("Plugin timeout")]
    PluginTimeout,
    #[error("Plugin compilation error")]
    CompileError(#[from] wasmer::CompileError),
}

impl From<PluginError> for RpcError {
    fn from(value: PluginError) -> Self {
        RpcError::custom(value.to_string())
    }
}

impl Plugin {
    pub fn builder(
        name: impl Into<String>,
        wasm_bytes: Vec<u8>,
        handler: Arc<dyn HostHandler>,
    ) -> PluginBuilder {
        PluginBuilder {
            id: PluginId::new(),
            name: name.into(),
            handler,
            logger: Arc::new(DefaultPluginLogger),
            wasm_bytes,
            timeout: Duration::from_secs(10),
        }
    }
}

impl PluginBuilder {
    pub async fn build(self) -> Result<Plugin, PluginError> {
        let compiled = Compiled::new(&self.name, &self.wasm_bytes).await?;

        let inner = InnerPlugin {
            id: self.id,
            handler: self.handler,
            logger: self.logger,
            compiled,
            timeout: self.timeout,
        };

        Ok(Plugin {
            inner: Arc::new(inner),
        })
    }

    pub fn with_id(mut self, id: PluginId) -> Self {
        self.id = id;
        self
    }

    pub fn with_logger(mut self, logger: impl PluginLogger) -> Self {
        self.logger = Arc::new(logger);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Plugin {
    pub fn name(&self) -> &str {
        &self.inner.compiled.name
    }

    pub fn id(&self) -> PluginId {
        self.inner.id
    }
}

impl AsyncTransport<PluginError> for Plugin {
    async fn call_async(&self, method: &str, params: Value) -> Result<RpcResponse, PluginError> {
        let runtime = self.create_runtime();
        let (id, stdin_writer, stdout_reader, stderr_reader) = runtime
            .spawn(self.inner.compiled.clone())
            .await
            .map_err(|e| PluginError::RuntimeError(e.into()))?;

        //? stderr logging
        let name = self.inner.compiled.name.clone();
        let logger = self.inner.logger.clone();
        let stderr_task = log_stderr(&name, stderr_reader, logger).fuse();

        //? transport setup
        let handler = PluginCallback {
            handler: self.inner.handler.clone(),
            uuid: self.inner.id,
        };
        let session = PluginSession::new(stdout_reader, stdin_writer, handler);
        let session_task = session.call_async(method, params).fuse();

        //? timeout
        let timeout = sleep(self.inner.timeout).fuse();

        //? Pump futures until (session_task AND stderr_task) OR timeout
        futures::pin_mut!(session_task, timeout, stderr_task);

        let mut rpc_result = None;
        loop {
            futures::select! {
                res = session_task => {
                    rpc_result = Some(res);
                    if stderr_task.is_terminated() {
                        break;
                    }
                }
                _ = timeout => {
                    //? Assume the instance has hung, so terminate its runtime
                    runtime.terminate(id).await;
                    return Err(PluginError::PluginTimeout);
                }
                _ = stderr_task => {
                    if rpc_result.is_some() {
                        break;
                    }
                }
            }
        }

        Ok(rpc_result.expect("Loop exited without result or timeout")?)
    }
}

impl Plugin {
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

/// Helper struct that implements RequestHandler by forwarding it to a HostHandler
/// with the added `uuid` context.
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

async fn log_stderr(
    name: &str,
    reader: impl AsyncRead + Unpin + Send + Sync + 'static,
    logger: Arc<dyn PluginLogger>,
) {
    let name = name.to_string();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 {
            break;
        }
        logger.log(&name, line.trim_end());
        line.clear();
    }
}
