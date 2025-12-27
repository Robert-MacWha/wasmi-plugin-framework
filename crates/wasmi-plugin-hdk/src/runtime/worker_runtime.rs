//! Web Worker runtime bridge, running the Wasm module in a Web Worker.

use std::collections::HashMap;
use std::sync::Arc;

use futures::lock::Mutex;
use futures::{AsyncRead, AsyncWrite};
use thiserror::Error;
use tracing::error;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use crate::runtime::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
use crate::runtime::worker_pool::{SpawnError, WorkerHandle, WorkerId, WorkerPool};
use crate::wasi::wasi_ctx;

pub struct WorkerRuntime {
    active_sessions: Arc<Mutex<HashMap<WorkerId, Arc<WorkerHandle>>>>,
}

#[derive(Debug, Error)]
enum RunError {
    #[error("WASI Error: {0}")]
    WasiError(#[from] wasi_ctx::WasiError),
    #[error("Runtime Error: {0}")]
    RuntimeError(#[from] wasmer::RuntimeError),
}

impl WorkerRuntime {
    pub fn new() -> Self {
        WorkerRuntime {
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for WorkerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime for WorkerRuntime {
    type Error = SpawnError;

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
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let pool = WorkerPool::global();
        let handle = pool
            .run(move || {
                let res = run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
                if let Err(e) = res {
                    error!("Error running Wasm instance: {}", e);
                }
            })
            .await?;

        let _ = stderr_reader; // Currently unused

        let id = handle.id;
        self.active_sessions.lock().await.insert(handle.id, handle);

        return Ok((id, stdin_writer, stdout_reader));
    }

    async fn terminate(self, instance_id: u64) {
        let instance_id = instance_id as WorkerId;
        if let Some(session) = self.active_sessions.lock().await.remove(&instance_id) {
            session.terminate();
        }
    }
}

fn run_instance(
    compiled: Compiled,
    stdin: NonBlockingPipeReader,
    stdout: NonBlockingPipeWriter,
    stderr: NonBlockingPipeWriter,
) -> Result<(), RunError> {
    let mut store = wasmer::Store::default();
    let module = compiled.module;
    let wasi_ctx = wasi_ctx::WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);

    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}
