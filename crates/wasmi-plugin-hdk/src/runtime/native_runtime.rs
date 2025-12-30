//! Native wasmer runtime, running the Wasm module in a separate thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::lock::Mutex;
use futures::{AsyncRead, AsyncWrite};
use thiserror::Error;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::error;
use wasmer::Store;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use wasmi_plugin_wasi::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
use wasmi_plugin_wasi::wasi_ctx::WasiCtx;

pub struct NativeRuntime {
    sessions: Arc<Mutex<HashMap<u64, NativeSession>>>,
    next_id: Arc<AtomicU64>,
}

struct NativeSession {
    wasm_handle: JoinHandle<()>,
}

#[derive(Debug, Error)]
pub enum NativeRuntimeError {
    #[error("Wasmer runtime error: {0}")]
    WasmerError(#[from] wasmer::RuntimeError),
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
}

impl NativeRuntime {
    pub fn new() -> Self {
        NativeRuntime {
            sessions: Arc::new(Mutex::new(HashMap::default())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime for NativeRuntime {
    type Error = NativeRuntimeError;

    async fn spawn(
        &self,
        compiled: Compiled,
    ) -> Result<
        (
            u64,
            impl AsyncWrite + Unpin + Send + Sync + 'static,
            impl AsyncRead + Unpin + Send + Sync + 'static,
            impl AsyncRead + Unpin + Send + Sync + 'static,
        ),
        Self::Error,
    > {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let wasm_handle = wasm_handle(compiled, stdin_reader, stdout_writer, stderr_writer);

        let session = NativeSession { wasm_handle };

        self.sessions.lock().await.insert(id, session);
        Ok((id, stdin_writer, stdout_reader, stderr_reader))
    }

    async fn terminate(&self, instance_id: u64) -> () {
        if let Some(session) = self.sessions.lock().await.remove(&instance_id) {
            // TODO: Use gas metering to force terminate the wasm runtime
            session.wasm_handle.abort();
        }
    }
}

fn wasm_handle(
    compiled: Compiled,
    stdin_reader: NonBlockingPipeReader,
    stdout_writer: NonBlockingPipeWriter,
    stderr_writer: NonBlockingPipeWriter,
) -> JoinHandle<()> {
    spawn_blocking(move || {
        let result = run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
        if let Err(e) = result {
            error!("Plugin execution error: {:?}", e);
        }
    })
}

fn run_instance(
    compiled: Compiled,
    stdin: NonBlockingPipeReader,
    stdout: NonBlockingPipeWriter,
    stderr: NonBlockingPipeWriter,
) -> Result<(), Box<dyn std::error::Error>> {
    let engine = compiled.engine;
    let module = compiled.module;

    let mut store = Store::new(engine);

    let wasi_ctx = WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);

    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}
