//! Native wasmer bridge, running the Wasm module in a separate thread.

use futures::AsyncBufReadExt;
use std::io::{Read, Write};
use thiserror::Error;
use tokio::spawn;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::info;
use wasmer::{Instance, Store};

use crate::bridge::Bridge;
use crate::bridge::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
use crate::bridge::virtual_file::{WasiInputFile, WasiOutputFile};
use crate::compile::Compiled;
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
        compiled: Compiled,
    ) -> Result<
        (
            Self,
            impl Write + Send + Sync + 'static,
            impl Read + Send + Sync + 'static,
        ),
        NativeBridgeError,
    > {
        let name = compiled.name.clone();
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let wasm_handle = spawn_blocking(move || {
            let result = Self::run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
            if let Err(e) = result {
                eprintln!("Plugin execution error: {:?}", e);
            }
        });

        // TODO: stderr task to log to console

        let bridge = NativeBridge {
            wasm_handle,
            stderr_handle,
        };

        Ok((bridge, stdin_writer, stdout_reader))
    }

    fn run_instance(
        compiled: Compiled,
        stdin: NonBlockingPipeReader,
        stdout: NonBlockingPipeWriter,
        stderr: NonBlockingPipeWriter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let engine = compiled.engine;
        let module = compiled.module;

        let store = Store::new(&engine);

        let mut wasi_ctx = WasiCtx::new()
            .set_stdin(stdin)
            .set_stdout(stdout)
            .set_stderr(stderr);
        let imports = wasi_ctx.import_object(&store, &module);
        let instance = Instance::new(&mut store, module, imports)?;

        let memory = instance.exports.get_memory("memory")?;
        wasi_ctx.set_memory(memory);

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
