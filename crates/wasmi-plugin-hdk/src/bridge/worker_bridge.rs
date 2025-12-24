//! Web Worker wasmer bridge, running the Wasm module in a Web Worker.

use std::io::{BufRead, BufReader, Read, Write};

use futures::channel::oneshot::{Receiver, Sender};
use thiserror::Error;
use tracing::{error, info};
use wasm_bindgen_futures::spawn_local;
use web_sys::Worker;
use web_sys::js_sys::{Reflect, SharedArrayBuffer};
use web_sys::{MessageEvent, js_sys, wasm_bindgen::prelude::Closure};

use web_sys::wasm_bindgen::{JsCast, JsValue};

use crate::bridge::Bridge;
use crate::bridge::shared_pipe::SharedPipe;
use crate::bridge::worker_protocol::WorkerMessage;

pub struct WorkerBridge {
    worker: SendWorker,
    // Keep this to ensure the JS callback isn't garbage collected
    _onmessage_closure: Closure<dyn FnMut(MessageEvent)>,
}

#[derive(Clone)]
struct SendWorker(Worker);

#[derive(Debug, Error)]
pub enum WorkerBridgeError {
    #[error("JavaScript error: {0:?}")]
    JsValue(JsValue),
    #[error("Serde serialization error: {0}")]
    Serde(#[from] serde_wasm_bindgen::Error),
    #[error("Receiver error: {0}")]
    Receiver(#[from] futures::channel::oneshot::Canceled),
}

const WORKER_JS_GLUE: &str = include_str!("../../../worker/pkg/worker.js");
const WORKER_WASM_BYTES: &[u8] = include_bytes!("../../../worker/pkg/worker_bg.wasm");

unsafe impl Send for WorkerBridge {}
unsafe impl Sync for WorkerBridge {}

impl WorkerBridge {
    pub fn new(
        name: &str,
        wasm_bytes: &[u8],
    ) -> Result<
        (
            Self,
            impl Write + Send + Sync + 'static,
            impl Read + Send + Sync + 'static,
        ),
        WorkerBridgeError,
    > {
        info!("Creating WorkerBridge for plugin: {}", &name);

        let (ready_tx, ready_rx) = futures::channel::oneshot::channel::<()>();
        let stdin = SharedPipe::new_with_capacity(16384);
        let stdout = SharedPipe::new_with_capacity(16384);
        let stderr = SharedPipe::new_with_capacity(16384);

        //? Setup and send worker
        let worker_url = worker_script()?;
        let worker =
            SendWorker(Worker::new(&worker_url).map_err(|e| WorkerBridgeError::JsValue(e))?);

        //? Worker onmessage callback
        let onmessage_callback = Closure::wrap(on_message(ready_tx));
        worker
            .0
            .set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));

        //? Send all stdin data to worker
        let worker_inner = worker.clone();
        let wasm = wasm_bytes.to_vec();
        let stdin_buf = stdin.buffer();
        let stdout_buf = stdout.buffer();
        let stderr_buf = stderr.buffer();
        spawn_local(async move {
            match load(
                ready_rx,
                wasm,
                stdin_buf,
                stdout_buf,
                stderr_buf,
                &worker_inner,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => {
                    error!("[WorkerBridge] Failed to load Wasm: {}", e);
                }
            }
        });

        //? Log messages from stderr as plugin logs
        let name = name.to_string();
        spawn_local(async move {
            let mut stderr = BufReader::new(stderr);
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match stderr.read_line(&mut buffer) {
                    Ok(0) => {
                        break;
                    }
                    Ok(_) => {
                        info!("[plugin] [{}] {}", name, buffer.trim_end());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        gloo_timers::future::TimeoutFuture::new(0).await;
                        continue;
                    }
                    Err(e) => {
                        error!("[WorkerBridge] Error reading from stderr: {}", e);
                        break;
                    }
                }
            }
        });

        let bridge = WorkerBridge {
            worker,
            _onmessage_closure: onmessage_callback,
        };
        Ok((bridge, stdin, stdout))
    }
}

/// Creates a blob URL containing the worker script that lodas and executes the
/// wasm module.
fn worker_script() -> Result<String, WorkerBridgeError> {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/wasm");
    let wasm_blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::Uint8Array::from(WORKER_WASM_BYTES)),
        &options,
    )
    .map_err(WorkerBridgeError::JsValue)?;
    let wasm_url = web_sys::Url::create_object_url_with_blob(&wasm_blob)
        .map_err(WorkerBridgeError::JsValue)?;

    // 2. Create a script that initializes the glue
    let loader_script = format!(
        r#"
        console.log = function(...args) {{
            self.postMessage({{ type: 'Log', message: args.join(' ') }});
        }};
        console.error = function(...args) {{
            self.postMessage({{ type: 'Error', message: args.join(' ') }});
        }};

        console.log("Worker Shim: Script starting...");
        
        {0} // WORKER_JS_GLUE

        wasm_bindgen('{1}').then(() => {{
            console.log("Worker Shim: Rust Wasm Initialized");
            wasm_bindgen.start_worker();
        }}).catch(err => console.error("Worker Init Failed:", err));
        "#,
        WORKER_JS_GLUE, wasm_url
    );

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::JsString::from(loader_script)),
        &options,
    )
    .map_err(WorkerBridgeError::JsValue)?;

    web_sys::Url::create_object_url_with_blob(&blob).map_err(WorkerBridgeError::JsValue)
}

// Callback for worker onmessage events, when the host receives a message from the worker.
fn on_message(ready_tx: Sender<()>) -> Box<dyn FnMut(MessageEvent)> {
    let mut ready_tx = Some(ready_tx);
    Box::new(move |e: MessageEvent| {
        let Ok(msg) = serde_wasm_bindgen::from_value::<WorkerMessage>(e.data()) else {
            error!(
                "WorkerBridge: Received unknown message from worker: {:?}",
                e.data()
            );
            return;
        };

        match msg {
            WorkerMessage::Log { message } => {
                info!("[WORKER] {}", message);
            }
            WorkerMessage::Error { message } => {
                error!("[WORKER ERROR] {}", message);
            }
            WorkerMessage::Ready => {
                info!("[WorkerBridge] Worker is ready");

                if let Some(tx) = ready_tx.take() {
                    let _ = tx.send(());
                }
            }
            _ => {
                error!("[WorkerBridge] Received unexpected message type from worker");
            }
        }
    })
}

/// Sends the Load message to the worker, transferring the Wasm bytes and pipes
async fn load(
    ready_rx: Receiver<()>,
    wasm: Vec<u8>,
    stdin: SharedArrayBuffer,
    stdout: SharedArrayBuffer,
    stderr: SharedArrayBuffer,
    worker_inner: &SendWorker,
) -> Result<(), WorkerBridgeError> {
    ready_rx.await?;

    // Create Load message
    let msg = serde_wasm_bindgen::to_value(&WorkerMessage::Load { wasm })?;
    Reflect::set(&msg, &"stdin".into(), &stdin).map_err(WorkerBridgeError::JsValue)?;
    Reflect::set(&msg, &"stdout".into(), &stdout).map_err(WorkerBridgeError::JsValue)?;
    Reflect::set(&msg, &"stderr".into(), &stderr).map_err(WorkerBridgeError::JsValue)?;

    // Extract the wasm as Uint8Array for transfer
    let payload = js_sys::Reflect::get(&msg, &"wasm".into()).map_err(WorkerBridgeError::JsValue)?;
    let uint8_array = payload
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(WorkerBridgeError::JsValue)?;
    let buffer = uint8_array.buffer();

    // Post the message with transfer
    //? Transfer will more efficiently move the Wasm bytes to the worker by
    //? moving the underlying ArrayBuffer instead of copying it. This will
    //? invalidate the original buffer in the main thread (zero it).
    let transfer_list = js_sys::Array::of1(&buffer);
    worker_inner
        .post_message_with_transfer(&msg, &transfer_list)
        .map_err(WorkerBridgeError::JsValue)?;
    Ok(())
}

impl Bridge for WorkerBridge {
    fn terminate(self: Box<Self>) {
        self.worker.terminate();
    }
}

impl SendWorker {
    pub fn post_message_with_transfer(
        &self,
        msg: &JsValue,
        transfer: &js_sys::Array,
    ) -> Result<(), JsValue> {
        self.0.post_message_with_transfer(msg, transfer)
    }

    fn terminate(&self) {
        self.0.terminate();
    }
}
