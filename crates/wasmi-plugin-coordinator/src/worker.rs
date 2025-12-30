use std::{cell::RefCell, collections::HashMap, io::Write, rc::Rc, sync::Arc};

use futures::AsyncRead;
use futures::AsyncReadExt;
use thiserror::Error;
use tracing::{error, info, warn};
use wasm_bindgen::{
    JsCast,
    prelude::{Closure, wasm_bindgen},
};
use wasmi_plugin_coordinator_protocol::CoordinatorMessage;
use wasmi_plugin_coordinator_protocol::InstanceId;
use wasmi_plugin_wasi::non_blocking_pipe::NonBlockingPipeWriter;
use wasmi_plugin_wasi::non_blocking_pipe::non_blocking_pipe;
use wasmi_plugin_wasi::wasi_ctx;
use wasmi_plugin_wasi::wasi_ctx::WasiReader;
use web_sys::{
    DedicatedWorkerGlobalScope, MessageEvent,
    js_sys::{self, Reflect, Uint8Array, WebAssembly},
};

use crate::worker_pool::pool::{SpawnError, WorkerHandle, WorkerPool};

struct CoordinatorState {
    instance_channels: HashMap<InstanceId, NonBlockingPipeWriter>,
    instance_handles: HashMap<InstanceId, Arc<WorkerHandle>>,
}

#[derive(Debug, Error)]
enum RunError {
    #[error("WASI Error: {0}")]
    Wasi(#[from] wasi_ctx::WasiError),
    #[error("Wasmer Runtime Error: {0}")]
    Runtime(#[from] wasmer::RuntimeError),
    #[error("Compile Error: {0}")]
    Compile(#[from] wasmer::CompileError),
}

#[wasm_bindgen]
pub async fn start_coordinator() {
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
            .set_max_level(tracing::Level::INFO)
            .build(),
    );

    info!("Starting Wasmi Plugin Coordinator...");
    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();

    let state = CoordinatorState {
        instance_channels: HashMap::new(),
        instance_handles: HashMap::new(),
    };
    let state = Rc::new(RefCell::new(state));

    let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
        let mut state = state.borrow_mut();
        on_message(&mut state, e);
    }) as Box<dyn FnMut(MessageEvent)>);

    global.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // Send ready message to main thread
    info!("Coordinator ready");
    let ready_msg = CoordinatorMessage::Ready;
    let ready_value = serde_wasm_bindgen::to_value(&ready_msg).unwrap();
    global.post_message(&ready_value).unwrap();
}

fn on_message(state: &mut CoordinatorState, e: web_sys::MessageEvent) {
    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();
    let data = e.data();

    let msg = match serde_wasm_bindgen::from_value::<CoordinatorMessage>(data.clone()) {
        Ok(msg) => msg,
        Err(err) => {
            error!("Failed to deserialize message: {}", err);
            return;
        }
    };

    match msg {
        CoordinatorMessage::Stdin { id } => {
            let Ok(data) = Reflect::get(&data, &"data".into()) else {
                error!("Failed to get stdin data from message");
                return;
            };

            stdin(state, id, data.unchecked_into());
        }
        CoordinatorMessage::Terminate { id } => {
            terminate(state, id);
        }
        CoordinatorMessage::RunPlugin { id } => {
            let Ok(wasm_bytes) = Reflect::get(&data, &"wasm_bytes".into()) else {
                error!("Failed to get wasm_bytes from message");
                return;
            };

            let Ok(wasm_module) = Reflect::get(&data, &"wasm_module".into()) else {
                error!("Failed to get wasm_module from message");
                return;
            };

            let status = match run_plugin(
                state,
                id,
                wasm_bytes.unchecked_into(),
                wasm_module.unchecked_into(),
            ) {
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            };

            let response_msg = CoordinatorMessage::RunPluginResp { id, status };
            let response_value = serde_wasm_bindgen::to_value(&response_msg).unwrap();
            global.post_message(&response_value).unwrap();
        }

        _ => {
            warn!("Unknown message received");
        }
    }
}

fn stdin(state: &mut CoordinatorState, id: InstanceId, data: Uint8Array) {
    info!("Received stdin message for id {}", id);
    let data: Vec<u8> = data.to_vec();

    if let Some(writer) = state.instance_channels.get_mut(&id) {
        if let Err(e) = writer.write_all(&data) {
            error!("Failed to write to stdin pipe: {}", e);
        }
    } else {
        warn!("No stdin channel found for instance {}", id);
    }
}

fn terminate(state: &mut CoordinatorState, id: InstanceId) {
    if let Some(handle) = state.instance_handles.remove(&id) {
        handle.terminate();
    } else {
        warn!("No instance handle found for instance {}", id);
    }

    state.instance_channels.remove(&id);
}

fn run_plugin(
    state: &mut CoordinatorState,
    id: InstanceId,
    wasm_bytes: Uint8Array,
    wasm_module: WebAssembly::Module,
) -> Result<Arc<WorkerHandle>, SpawnError> {
    let wasm_bytes: Vec<u8> = wasm_bytes.to_vec();

    let (stdin_reader, stdin_writer) = non_blocking_pipe();
    let (stdout_reader, stdout_writer) = non_blocking_pipe();
    let (stderr_reader, stderr_writer) = non_blocking_pipe();

    // Start proxies
    stdout_proxy(stdout_reader, id.clone());
    stderr_proxy(stderr_reader, id.clone());

    state.instance_channels.insert(id.clone(), stdin_writer);

    let pool = WorkerPool::global();
    pool.run_with(&wasm_module, move |extra| async move {
        info!("Running Wasm instance for id {}", id);
        let res = run_instance(
            extra,
            wasm_bytes,
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

async fn run_instance(
    js_module: wasm_bindgen::JsValue,
    wasm_bytes: Vec<u8>,
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

    info!("Starting WASI instance...");
    let start = wasi_ctx.into_fn(&mut store, &module)?;
    start.call(&mut store, &[])?;

    Ok(())
}

fn stdout_proxy(reader: impl AsyncRead + Unpin + 'static, id: InstanceId) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut reader = reader;
        let mut buffer = [0u8; 1024];

        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = js_sys::Uint8Array::from(&buffer[..n]);
                    let stdout_msg = CoordinatorMessage::Stdout { id: id.clone() };
                    let msg_value = serde_wasm_bindgen::to_value(&stdout_msg).unwrap();
                    Reflect::set(&msg_value, &"data".into(), &data).unwrap();
                    let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();
                    let transfer_list = js_sys::Array::of1(&data.buffer());
                    global
                        .post_message_with_transfer(&msg_value, &transfer_list)
                        .unwrap();
                }
                Err(e) => {
                    error!("Error reading stdout for instance {}: {}", id, e);
                    break;
                }
            }
        }
    });
}

fn stderr_proxy(reader: impl AsyncRead + Unpin + 'static, id: InstanceId) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut reader = reader;
        let mut chunk_buffer = [0u8; 1024];
        let mut accumulated_data = Vec::with_capacity(1024);

        let send_buffer = |data: &[u8]| {
            if data.is_empty() {
                return;
            }

            let data = js_sys::Uint8Array::from(data);
            let stderr_msg = CoordinatorMessage::Stderr { id: id.clone() };
            let msg_value = serde_wasm_bindgen::to_value(&stderr_msg).unwrap();
            js_sys::Reflect::set(&msg_value, &"data".into(), &data).unwrap();

            let transfer_list = js_sys::Array::of1(&data.buffer());
            let global = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();
            global
                .post_message_with_transfer(&msg_value, &transfer_list)
                .unwrap();
        };

        loop {
            match reader.read(&mut chunk_buffer).await {
                Ok(0) => {
                    // EOF: Send any remaining data in the buffer
                    send_buffer(&accumulated_data);
                    break;
                }
                Ok(n) => {
                    accumulated_data.extend_from_slice(&chunk_buffer[..n]);

                    // If we've hit or exceeded 1 KiB, send it and clear
                    if accumulated_data.len() >= 1024 {
                        send_buffer(&accumulated_data);
                        accumulated_data.clear();
                    }
                }
                Err(e) => {
                    error!("Error reading stderr for instance {}: {}", id, e);
                    send_buffer(&accumulated_data);
                    break;
                }
            }
        }
    });
}
