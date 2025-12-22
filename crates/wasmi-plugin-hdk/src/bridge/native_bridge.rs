//! Native wasmer bridge, running the Wasm module in a separate thread.

use futures::AsyncBufReadExt;
use std::io::{Read, Write};
use thiserror::Error;
use tokio::spawn;
use tokio::task::{JoinHandle, spawn_blocking};
use tracing::info;
use wasmer_wasix::runners::wasi::{RuntimeOrEngine, WasiRunner};

use crate::bridge::Bridge;
use crate::bridge::virtual_file::{WasiInputFile, WasiOutputFile};
use crate::compile::Compiled;
use crate::wasi::non_blocking_pipe::{
    NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe,
};

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
            impl Read + Send + Sync + 'static,
            impl Write + Send + Sync + 'static,
        ),
        NativeBridgeError,
    > {
        let name = compiled.name.clone();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();

        let wasm_handle = spawn_blocking(move || {
            let result = Self::run_instance(compiled, stdin_reader, stdout_writer, stderr_writer);
            if let Err(e) = result {
                eprintln!("Plugin execution error: {:?}", e);
            }
        });

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
        stdin: NonBlockingPipeReader,
        stdout: NonBlockingPipeWriter,
        stderr: NonBlockingPipeWriter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let engine = compiled.engine;
        let module = compiled.module;

        let stdin = WasiInputFile { reader: stdin };
        let stdout = WasiOutputFile { writer: stdout };
        let stderr = WasiOutputFile { writer: stderr };

        let mut runner = WasiRunner::new();
        runner
            .with_stdin(Box::new(stdin))
            .with_stdout(Box::new(stdout))
            .with_stderr(Box::new(stderr));

        let _ = runner.run_wasm(
            RuntimeOrEngine::Engine(engine),
            &compiled.name,
            module,
            compiled.module_hash,
        );

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
