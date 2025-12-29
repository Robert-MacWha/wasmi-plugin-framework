//! Web Worker runtime bridge, running the Wasm module in a Web Worker.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use futures::lock::Mutex;
use futures::{AsyncRead, AsyncWrite};
use thiserror::Error;
use tracing::{error, info, warn};
use wasm_bindgen::JsCast;
use wasmi_plugin_rt::yield_now;
use web_sys::js_sys::WebAssembly;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use crate::runtime::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
use crate::wasi::wasi_ctx::{self, WasiReader};
use crate::worker_pool::pool::{SpawnError, WorkerHandle, WorkerId, WorkerPool};

pub struct WorkerRuntime {
    active_sessions: Arc<Mutex<HashMap<WorkerId, Arc<WorkerHandle>>>>,
}

#[derive(Debug, Error)]
enum RunError {
    #[error("WASI Error: {0}")]
    Wasi(#[from] wasi_ctx::WasiError),
    #[error("Runtime Error: {0}")]
    Runtime(#[from] wasmer::RuntimeError),
    #[error("Compile Error: {0}")]
    Compile(#[from] wasmer::CompileError),
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
            impl AsyncRead + Unpin + Send + Sync + 'static,
        ),
        Self::Error,
    > {
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let handle = wasm_handle(compiled, stdin_reader, stdout_writer, stderr_writer)?;

        let id = handle.id;
        self.active_sessions.lock().await.insert(id, handle);

        Ok((id, stdin_writer, stdout_reader, stderr_reader))
    }

    async fn terminate(&self, instance_id: u64) {
        let instance_id = instance_id as WorkerId;
        if let Some(session) = self.active_sessions.lock().await.remove(&instance_id) {
            session.terminate();
        }
    }
}

fn wasm_handle(
    compiled: Compiled,
    stdin_reader: NonBlockingPipeReader,
    stdout_writer: NonBlockingPipeWriter,
    stderr_writer: NonBlockingPipeWriter,
) -> Result<Arc<WorkerHandle>, SpawnError> {
    let pool = WorkerPool::global();
    let module = compiled.js_module;
    pool.run_with(&module, move |extra| async move {
        let res = run_instance(
            extra,
            &compiled.wasm_bytes,
            stdin_reader,
            stdout_writer,
            stderr_writer,
        )
        .await;
        if let Err(e) = res {
            error!("Error running Wasm instance: {}", e);
        }
    })
}

/// Run a Wasm instance with WASI and asyncify support.
///
/// https://kripken.github.io/blog/wasm/2019/07/16/asyncify.html
async fn run_instance(
    js_module: wasm_bindgen::JsValue,
    wasm_bytes: &[u8],
    stdin: impl WasiReader + 'static,
    stdout: impl Write + Send + Sync + 'static,
    stderr: impl Write + Send + Sync + 'static,
) -> Result<(), RunError> {
    let mut store = wasmer::Store::default();

    let js_module: WebAssembly::Module = js_module.dyn_into().unwrap();
    let module = (js_module, wasm_bytes).into();

    let wasi_ctx = wasi_ctx::WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);

    info!("Starting Wasm instance");
    let (start, ctx) = wasi_ctx.into_fn(&mut store, &module)?;
    let mut unwind_count = 0;
    loop {
        start.call(&mut store, &[])?;

        match ctx.as_ref(&store).asyncify_state {
            wasi_ctx::AsyncifyState::Unwinding => {
                // Unwind
                let stop_unwind = ctx
                    .as_mut(&mut store)
                    .asyncify_stop_unwind_fn
                    .as_ref()
                    .unwrap()
                    .clone();
                stop_unwind.call(&mut store, &[])?;

                // Yield to JS
                unwind_count += 1;
                yield_now().await;

                // Rewind
                ctx.as_mut(&mut store).asyncify_state = wasi_ctx::AsyncifyState::Rewinding;
                let start_rewind = ctx
                    .as_mut(&mut store)
                    .asyncify_start_rewind_fn
                    .as_ref()
                    .unwrap()
                    .clone();
                let addr = ctx.as_ref(&store).asyncify_data_addr;
                start_rewind.call(&mut store, &[addr.into()])?;
            }
            wasi_ctx::AsyncifyState::Normal => {
                break;
            }
            wasi_ctx::AsyncifyState::Rewinding => {
                warn!("wasi_ctx is unexpectedly Rewinding");
                break;
            }
        }
    }

    info!("Wasm instance exited after {} unwinds", unwind_count);

    Ok(())
}
