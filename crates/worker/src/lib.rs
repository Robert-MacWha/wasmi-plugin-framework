#![cfg(target_arch = "wasm32")]

use thiserror::Error;
use wasm_bindgen::{
    JsCast, JsValue,
    prelude::{Closure, wasm_bindgen},
};
use wasmi_plugin_hdk::{
    bridge::{shared_pipe::SharedPipe, worker_protocol::WorkerMessage},
    wasi::wasi_ctx::{self, WasiCtx},
};
use web_sys::{
    DedicatedWorkerGlobalScope, MessageEvent, console,
    js_sys::{self, SharedArrayBuffer},
};

#[derive(Debug, Error)]
enum WorkerError {
    #[error("Compile Error: {0}")]
    CompileError(#[from] wasmer::CompileError),
    #[error("WASI Error: {0}")]
    WasiError(#[from] wasi_ctx::WasiError),
    #[error("Runtime Error: {0}")]
    RuntimeError(#[from] wasmer::RuntimeError),
}

#[wasm_bindgen]
pub fn start_worker() {
    log("Worker: Starting up...");
    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();

    // Register onmessage with channels
    let onmessage: Closure<dyn FnMut(MessageEvent)> = Closure::wrap(Box::new(on_message));
    global.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // Send ready
    let ready_msg = WorkerMessage::Ready;
    let ready_msg = serde_wasm_bindgen::to_value(&ready_msg).unwrap();
    global.post_message(&ready_msg).unwrap();
}

fn on_message(e: MessageEvent) {
    let Ok(msg) = serde_wasm_bindgen::from_value::<WorkerMessage>(e.data()) else {
        log("Worker: Failed to deserialize message from Host");
        return;
    };

    let WorkerMessage::Load { wasm } = msg else {
        error("Worker: Received unexpected message type");
        return;
    };

    let stdin_buf = get_sab(&e.data(), "stdin");
    let stdout_buf = get_sab(&e.data(), "stdout");
    let stderr_buf = get_sab(&e.data(), "stderr");

    let Some(stdin) = stdin_buf else {
        error("Load message missing stdin SharedArrayBuffer");
        return;
    };

    let Some(stdout) = stdout_buf else {
        error("Load message missing stdout SharedArrayBuffer");
        return;
    };

    let Some(stderr) = stderr_buf else {
        error("Load message missing stderr SharedArrayBuffer");
        return;
    };

    if let Err(err) = run_instance(&wasm, stdin, stdout, stderr) {
        error(&format!("Worker: Error running instance: {:?}", err));
    } else {
        log("Worker: Instance finished execution");
    }
}

fn get_sab(obj: &JsValue, key: &str) -> Option<SharedArrayBuffer> {
    js_sys::Reflect::get(obj, &key.into())
        .ok()
        .and_then(|v| v.dyn_into::<SharedArrayBuffer>().ok())
}

fn run_instance(
    wasm_bytes: &[u8],
    stdin: SharedArrayBuffer,
    stdout: SharedArrayBuffer,
    stderr: SharedArrayBuffer,
) -> Result<(), WorkerError> {
    let stdin = SharedPipe::new(&stdin);
    let stdout = SharedPipe::new(&stdout);
    let stderr = SharedPipe::new(&stderr);

    let mut store = wasmer::Store::default();
    let module = wasmer::Module::new(&store, wasm_bytes)?;

    let wasi_ctx = WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);
    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}

fn log(s: &str) {
    console::log_1(&s.into());
}

fn error(s: &str) {
    console::error_1(&s.into());
}
