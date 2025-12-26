#![cfg(target_arch = "wasm32")]

use std::sync::OnceLock;

use thiserror::Error;
use tracing::{Level, error, info};
use wasm_bindgen::{
    JsCast, JsValue,
    prelude::{Closure, wasm_bindgen},
};
use wasmi_plugin_hdk::{
    runtime::{shared_pipe::SharedPipe, worker_protocol::WorkerMessage},
    wasi::wasi_ctx::{self, WasiCtx},
};
use web_sys::{
    DedicatedWorkerGlobalScope, MessageEvent,
    js_sys::{self, SharedArrayBuffer},
};

mod message_writer;

static BUFFERS: OnceLock<(SAB, SAB)> = OnceLock::new();

struct SAB(SharedArrayBuffer);

unsafe impl Send for SAB {}
unsafe impl Sync for SAB {}

#[derive(Debug, Error)]
enum WorkerError {
    #[error("Deserialization Error: {0}")]
    DeserializeError(#[from] wasmer::DeserializeError),
    #[error("WASI Error: {0}")]
    WasiError(#[from] wasi_ctx::WasiError),
    #[error("Runtime Error: {0}")]
    RuntimeError(#[from] wasmer::RuntimeError),
}

#[wasm_bindgen]
pub fn start_worker() {
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
            .set_max_level(Level::INFO)
            .build(),
    );

    info!("Worker: Starting up");
    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();

    // Register onmessage with channels
    let onmessage: Closure<dyn FnMut(MessageEvent)> = Closure::wrap(Box::new(on_message));
    global.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // Send ready
    let booted_msg = serde_wasm_bindgen::to_value(&WorkerMessage::Booted).unwrap();
    global.post_message(&booted_msg).unwrap();
}

fn on_message(e: MessageEvent) {
    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();
    let data = e.data();

    // Use serde to parse the variant.
    // If it's "Initialize", it won't have fields in the enum, so we use Reflect.
    let Ok(msg) = serde_wasm_bindgen::from_value::<WorkerMessage>(data.clone()) else {
        error!("Worker: Failed to deserialize message");
        return;
    };

    match msg {
        WorkerMessage::Initialize => {
            on_initialize(&global, data);
        }

        WorkerMessage::Load { wasm_module, name } => {
            on_load(global, wasm_module, name);
        }
        _ => {}
    }
}

fn on_initialize(global: &DedicatedWorkerGlobalScope, data: JsValue) {
    let stdin_buf = get_sab(&data, "stdin");
    let stdout_buf = get_sab(&data, "stdout");

    let (Some(si), Some(so)) = (stdin_buf, stdout_buf) else {
        error!("Worker: Initialize message missing SharedArrayBuffers");
        return;
    };

    if BUFFERS.set((SAB(si), SAB(so))).is_err() {
        info!("Worker: Warning - Attempted to re-initialize buffers");
    }

    let msg = serde_wasm_bindgen::to_value(&WorkerMessage::Initialized).unwrap();
    global.post_message(&msg).unwrap();
}

fn on_load(global: DedicatedWorkerGlobalScope, wasm_module: Vec<u8>, name: String) {
    let Some((stdin, stdout)) = BUFFERS.get() else {
        error!("Worker: Received Load message before pipes were initialized");
        return;
    };

    if let Err(err) = run_instance(&wasm_module, name, &stdin.0, &stdout.0) {
        error!("Worker: Run error: {:?}", err);
    }

    let idle_msg = serde_wasm_bindgen::to_value(&WorkerMessage::Idle).unwrap();
    global.post_message(&idle_msg).unwrap();
}

fn get_sab(obj: &JsValue, key: &str) -> Option<SharedArrayBuffer> {
    js_sys::Reflect::get(obj, &key.into())
        .ok()
        .and_then(|v| v.dyn_into::<SharedArrayBuffer>().ok())
}

fn run_instance(
    wasm_module: &[u8],
    name: String,
    stdin: &SharedArrayBuffer,
    stdout: &SharedArrayBuffer,
) -> Result<(), WorkerError> {
    let stdin = SharedPipe::new("WORKER_STDIN", stdin);
    let stdout = SharedPipe::new("WORKER_STDOUT", stdout);
    // TODO: Switch this back to SharedPipe for potentially increased performance?
    let stderr = message_writer::MessageWriter::new(name);

    let mut store = wasmer::Store::default();
    let module = unsafe { wasmer::Module::deserialize(&store, wasm_module) }?;

    let wasi_ctx = WasiCtx::new()
        .set_stdin(stdin)
        .set_stdout(stdout)
        .set_stderr(stderr);
    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}
