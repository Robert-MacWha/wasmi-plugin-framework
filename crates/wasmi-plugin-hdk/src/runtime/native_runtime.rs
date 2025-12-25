//! Native wasmer runtime, running the Wasm module in a separate thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::io::BufReader;
use futures::lock::Mutex;
use futures::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use thiserror::Error;
use tokio::spawn;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::{error, info};
use wasmer::Store;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use crate::runtime::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
use crate::wasi::wasi_ctx::WasiCtx;

pub struct NativeRuntime {
    sessions: Arc<Mutex<HashMap<u64, NativeSession>>>,
    next_id: Arc<AtomicU64>,
}

struct NativeSession {
    wasm_handle: JoinHandle<()>,
    stderr_handle: JoinHandle<()>,
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
        ),
        Self::Error,
    > {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = compiled.name.clone();
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let wasm_handle = Self::wasm_handle(compiled, stdin_reader, stdout_writer, stderr_writer);
        let stderr_handle = Self::stderr_handle(name, stderr_reader);

        let session = NativeSession {
            wasm_handle,
            stderr_handle,
        };

        self.sessions.lock().await.insert(id, session);
        Ok((id, stdin_writer, stdout_reader))
    }

    async fn terminate(self, instance_id: u64) -> () {
        if let Some(session) = self.sessions.lock().await.remove(&instance_id) {
            // TODO: Use gas metering to force terminate the wasm runtime
            session.wasm_handle.abort();
            session.stderr_handle.abort();
        }
    }
}

impl NativeRuntime {
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

    fn wasm_handle(
        compiled: Compiled,
        stdin_reader: NonBlockingPipeReader,
        stdout_writer: NonBlockingPipeWriter,
        stderr_writer: NonBlockingPipeWriter,
    ) -> JoinHandle<()> {
        spawn_blocking(move || {
            let result = Self::run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
            if let Err(e) = result {
                eprintln!("Plugin execution error: {:?}", e);
            }
        })
    }

    fn stderr_handle(name: String, mut stderr_reader: NonBlockingPipeReader) -> JoinHandle<()> {
        spawn(async move {
            let mut stderr = BufReader::new(&mut stderr_reader);
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match stderr.read_line(&mut buffer).await {
                    Ok(0) => {
                        break;
                    }
                    Ok(_) => {
                        info!("[plugin] [{}] {}", name, buffer.trim_end());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(e) => {
                        error!("[WorkerBridge] Error reading from stderr: {}", e);
                        break;
                    }
                }
            }
        })
    }
}
