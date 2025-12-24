//! Native wasmer bridge, running the Wasm module in a separate thread.

use futures::AsyncBufReadExt;
use futures::io::BufReader;
use std::io::{Read, Write};
use thiserror::Error;
use tokio::spawn;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::{error, info};
use wasmer::Store;

use crate::bridge::Bridge;
use crate::bridge::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};
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
        compiled: &Compiled,
        _wasm_bytes: &[u8],
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

        let compiled = compiled.clone();
        let wasm_handle = spawn_blocking(move || {
            let result = Self::run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
            if let Err(e) = result {
                eprintln!("Plugin execution error: {:?}", e);
            }
        });

        let stderr_handle = spawn(async move {
            let mut stderr = BufReader::new(stderr_reader);
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
        });

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

        let mut store = Store::new(engine);

        let wasi_ctx = WasiCtx::new()
            .set_stdin(stdin)
            .set_stdout(stdout)
            .set_stderr(stderr);

        let start = wasi_ctx.into_fn(&mut store, &module)?;
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
