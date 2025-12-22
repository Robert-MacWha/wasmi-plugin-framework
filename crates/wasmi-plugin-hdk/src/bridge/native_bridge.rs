use futures::AsyncBufReadExt;
use std::io::{Read, Write};
use thiserror::Error;
use tokio::spawn;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::info;
use wasmer::{FunctionEnv, Instance, Module, Store};

use crate::bridge::Bridge;
use crate::compile::Compiled;
use crate::wasi::non_blocking_pipe::non_blocking_pipe;
use crate::wasi::wasi_ctx::WasiCtx;

pub struct NativeBridge {
    wasm_handle: JoinHandle<()>,
    stderr_handle: JoinHandle<()>,
}

#[derive(Debug, Error)]
pub enum NativeBridgeError {
    #[error("Wasmer runtime error: {0}")]
    WasmerError(#[from] wasmer::RuntimeError),
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),
}

impl NativeBridge {
    pub fn new(
        name: &str,
        compiled: Compiled,
    ) -> Result<
        (
            Self,
            impl Read + Send + Sync + 'static,
            impl Write + Send + Sync + 'static,
        ),
        NativeBridgeError,
    > {
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let wasm_handle = spawn_blocking(move || {
            let result = Self::run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
            if let Err(e) = result {
                eprintln!("Plugin execution error: {:?}", e);
            }
        });

        let name = name.to_string();
        let stderr_handle = spawn(async move {
            let mut buf_reader = futures::io::BufReader::new(stderr_reader);
            let mut line = String::new();
            while buf_reader.read_line(&mut line).await.is_ok_and(|n| n > 0) {
                info!("[plugin] [{}], {}", name, line.trim_end());
                line.clear();
            }
        });

        let bridge = NativeBridge {
            wasm_handle,
            stderr_handle,
        };

        Ok((bridge, stdout_reader, stdin_writer))
    }

    fn run_instance(
        compiled: Compiled,
        stdin: impl Read + Send + Sync + 'static,
        stdout: impl Write + Send + Sync + 'static,
        stderr: impl Write + Send + Sync + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let engine = compiled.engine;
        let module = compiled.module;

        let mut store = Store::new(engine);

        // TODO: Switch to using wasmer-wasix
        let wasi_ctx = WasiCtx::new()
            .set_stdin(Box::new(stdin))
            .set_stdout(Box::new(stdout))
            .set_stderr(Box::new(stderr));
        let function_env = FunctionEnv::new(&mut store, wasi_ctx);
        let import_object = WasiCtx::import_object(&mut store, &function_env);
        let instance = Instance::new(&mut store, &module, &import_object)?;
        function_env.as_mut(&mut store).memory =
            Some(instance.exports.get_memory("memory")?.clone());

        // Start the plugin
        let start = instance.exports.get_function("_start")?;
        start.call(&mut store, &[])?;

        Ok(())
    }
}

impl Bridge for NativeBridge {
    fn terminate(self: Box<Self>) {
        self.stderr_handle.abort();
        self.wasm_handle.abort();

        // Currently just drops the handle to stop the thread
        // TODO: Trigger out of gas termination
    }
}
