use std::sync::{Arc, atomic::AtomicBool};

use crate::wasi::{
    non_blocking_pipe::{NonBlockingPipeReader, NonBlockingPipeWriter, non_blocking_pipe},
    wasi_ctx::{WasiCtx, add_to_linker},
    wasmi::spawn_wasm,
};
use thiserror::Error;
use wasmi::{Engine, Linker, Module, Store};

#[derive(Error, Debug)]
pub enum SpawnError {
    #[error("io error")]
    IoError(#[from] std::io::Error),
    #[error("start not found")]
    StartNotFound,
    #[error("wasmi error")]
    WasmiError(#[from] wasmi::Error),
}

/// Creates the plugin pipes and task.
pub fn spawn_plugin(
    engine: &Engine,
    module: &Module,
    max_fuel: Option<u64>,
) -> Result<
    (
        NonBlockingPipeWriter,
        NonBlockingPipeReader,
        NonBlockingPipeReader,
        Arc<AtomicBool>,
        impl Future<Output = ()>,
    ),
    SpawnError,
> {
    let is_running = Arc::new(AtomicBool::new(true));

    // Setup pipes
    let (stdin_reader, stdin_writer) = non_blocking_pipe();
    let (stdout_reader, stdout_writer) = non_blocking_pipe();
    let (stderr_reader, stderr_writer) = non_blocking_pipe();

    let mut linker = Linker::new(engine);
    let wasi = WasiCtx::new()
        .set_stdin(stdin_reader)
        .set_stdout(stdout_writer)
        .set_stderr(stderr_writer);

    let mut store = Store::new(engine, wasi);
    add_to_linker(&mut linker)?;

    let instance = linker.instantiate_and_start(&mut store, module)?;
    let start_func = instance
        .get_func(&store, "_start")
        .ok_or(SpawnError::StartNotFound)?;

    let fut = spawn_wasm(store, start_func, is_running.clone(), max_fuel);

    Ok((stdin_writer, stdout_reader, stderr_reader, is_running, fut))
}
