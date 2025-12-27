//! Web Worker runtime bridge, running the Wasm module in a Web Worker.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use futures::io::BufReader;
use futures::lock::Mutex;
use futures::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use thiserror::Error;
use tracing::{error, info};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::js_sys::WebAssembly;

use crate::compile::Compiled;
use crate::runtime::Runtime;
use crate::runtime::non_blocking_pipe::non_blocking_pipe;
use crate::wasi::wasi_ctx::{self, WasiReader};
use crate::worker_pool::worker_pool::{SpawnError, WorkerHandle, WorkerId, WorkerPool};

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
        ),
        Self::Error,
    > {
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();
        // let stderr_writer: MessageWriter = MessageWriter::new("".into());

        handle_stderr(&compiled.name, stderr_reader);

        let module = compiled.js_module;
        let pool = WorkerPool::global();
        let handle = pool.run_with(&module, move |extra| async move {
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
        })?;

        let id = handle.id;
        self.active_sessions.lock().await.insert(handle.id, handle);

        Ok((id, stdin_writer, stdout_reader))
    }

    async fn terminate(self, instance_id: u64) {
        let instance_id = instance_id as WorkerId;
        if let Some(session) = self.active_sessions.lock().await.remove(&instance_id) {
            session.terminate();
        }
    }
}

async fn run_instance(
    js_module: wasm_bindgen::JsValue,
    wasm_bytes: &[u8],
    stdin: impl WasiReader + 'static,
    stdout: impl Write + Send + Sync + 'static,
    stderr: impl Write + Send + Sync + 'static,
) -> Result<(), RunError> {
    info!("Starting Wasm instance in WorkerRuntime");
    let mut store = wasmer::Store::default();

    info!("js_module type: {:?}", js_module.js_typeof());
    info!(
        "is WebAssembly.Module: {:?}",
        js_module.is_instance_of::<WebAssembly::Module>()
    );
    info!("Casting Wasm module");
    let js_module: WebAssembly::Module = js_module.dyn_into().unwrap();
    info!("Creating Wasm module");
    let module = (js_module, wasm_bytes).into();

    info!("Creating WASI context");
    let wasi_ctx = wasi_ctx::WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);

    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}

fn handle_stderr(name: &str, stderr: impl AsyncRead + Unpin + Send + Sync + 'static) {
    let name = name.to_string();
    spawn_local(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let name = name;
        loop {
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    info!("[{}] {}", name, line.trim_end());
                    line.clear();
                }
                Err(e) => {
                    error!("Error reading Wasm stderr: {}", e);
                    break;
                }
            }
        }
    });
}
