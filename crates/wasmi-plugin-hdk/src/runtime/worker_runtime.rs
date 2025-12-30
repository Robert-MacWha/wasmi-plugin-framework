use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use futures::AsyncReadExt;
use futures::channel::oneshot::{self, Sender};
use futures::future::{BoxFuture, Shared};
use futures::{AsyncRead, AsyncWrite, FutureExt};
use thiserror::Error;
use tracing::{error, info, warn};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasmi_plugin_coordinator_protocol::{CoordinatorMessage, InstanceId};
use wasmi_plugin_wasi::non_blocking_pipe::non_blocking_pipe;
use web_sys::js_sys::Reflect;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType, js_sys};

use crate::compile::Compiled;
use crate::runtime::Runtime;

pub struct WorkerRuntime {
    inner: Arc<Mutex<InnerWorkerRuntime>>,
}

pub struct InnerWorkerRuntime {
    coordinator: Worker,
    instances: HashMap<InstanceId, PluginInstance>,
    ready_tx: Option<oneshot::Sender<()>>,
    shared_ready: Shared<BoxFuture<'static, ()>>,
    _onmessage_closure: Option<Closure<dyn FnMut(web_sys::MessageEvent)>>,
}

struct PluginInstance {
    stdout_writer: Box<dyn Write + Send + Sync + 'static>,
    stderr_writer: Box<dyn Write + Send + Sync + 'static>,
    run_plugin_resp_tx: Option<Sender<Result<(), String>>>,
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("Worker Error: {0}")]
    WorkerError(String),
    #[error("Oneshot cancelled")]
    OneshotCancelled,
    #[error("Serde Error: {0}")]
    Serde(#[from] serde_wasm_bindgen::Error),
    #[error("Post Message Error: {0:?}")]
    PostMessage(JsValue),
    #[error("Reflect Error: {0:?}")]
    Reflect(JsValue),
}

static RUNTIME: OnceLock<WorkerRuntime> = OnceLock::new();

const WORKER_LOADER_JS: &str = include_str!("../../../wasmi-plugin-coordinator/src/worker.js");
const WORKER_JS_GLUE: &str =
    include_str!("../../../wasmi-plugin-coordinator/pkg/wasmi_plugin_coordinator.js");
const WORKER_WASM_BYTES: &[u8] =
    include_bytes!("../../../wasmi-plugin-coordinator/pkg/wasmi_plugin_coordinator_bg.wasm");

unsafe impl Send for InnerWorkerRuntime {}
unsafe impl Sync for InnerWorkerRuntime {}

impl WorkerRuntime {
    pub fn global() -> &'static WorkerRuntime {
        RUNTIME.get_or_init(Self::init)
    }

    fn init() -> Self {
        info!("Initializing WorkerRuntime");
        let options = WorkerOptions::new();
        options.set_name("coordinator");
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&worker_url().unwrap(), &options).unwrap();

        let (ready_tx, ready_rx) = futures::channel::oneshot::channel();
        let shared_ready = ready_rx.map(|_| ()).boxed().shared();

        let inner = Arc::new(Mutex::new(InnerWorkerRuntime {
            coordinator: worker,
            instances: HashMap::new(),
            ready_tx: Some(ready_tx),
            shared_ready,
            _onmessage_closure: None,
        }));

        let inner_clone = inner.clone();
        let onmessage = Closure::wrap(Box::new(move |e: MessageEvent| {
            on_message(inner_clone.clone(), e);
        }) as Box<dyn FnMut(MessageEvent)>);

        inner
            .lock()
            .unwrap()
            .coordinator
            .set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        inner.lock().unwrap()._onmessage_closure = Some(onmessage);

        Self { inner }
    }

    async fn wait_ready(&self) {
        let shared_ready = {
            let inner = self.inner.lock().unwrap();
            inner.shared_ready.clone()
        };
        shared_ready.await;
    }
}

impl Runtime for &WorkerRuntime {
    type Error = SpawnError;

    async fn spawn(
        &self,
        compiled: Compiled,
    ) -> Result<
        (
            u64,
            impl AsyncWrite + Unpin + Send + Sync + 'static,
            impl AsyncRead + Unpin + Send + Sync + 'static,
            impl AsyncRead + Unpin + Send + Sync + 'static,
        ),
        Self::Error,
    > {
        self.wait_ready().await;

        let instance_id = InstanceId::new();
        let (stdin_reader, stdin_writer) = non_blocking_pipe();
        let (stdout_reader, stdout_writer) = non_blocking_pipe();
        let (stderr_reader, stderr_writer) = non_blocking_pipe();
        let (run_plugin_resp_tx, run_plugin_resp_rx) = futures::channel::oneshot::channel();

        let plugin_instance = PluginInstance {
            stdout_writer: Box::new(stdout_writer),
            stderr_writer: Box::new(stderr_writer),
            run_plugin_resp_tx: Some(run_plugin_resp_tx),
        };

        self.inner
            .lock()
            .unwrap()
            .instances
            .insert(instance_id, plugin_instance);

        // Start stdin proxy
        stdin_proxy(stdin_reader, instance_id, self.inner.clone());

        // Notify the coordinator
        let wasm_bytes_js = js_sys::Uint8Array::from(&compiled.wasm_bytes[..]);
        let wasm_module_js = compiled.js_module.clone();
        let spawn_msg = CoordinatorMessage::RunPlugin { id: instance_id };
        let msg_value = serde_wasm_bindgen::to_value(&spawn_msg)?;
        Reflect::set(&msg_value, &"wasm_bytes".into(), &wasm_bytes_js)
            .map_err(SpawnError::Reflect)?;
        Reflect::set(&msg_value, &"wasm_module".into(), &wasm_module_js)
            .map_err(SpawnError::Reflect)?;
        let transfer_list = js_sys::Array::of1(&wasm_bytes_js.buffer());

        self.inner
            .lock()
            .unwrap()
            .coordinator
            .post_message_with_transfer(&msg_value, &transfer_list)
            .map_err(SpawnError::PostMessage)?;

        // Await the response
        match run_plugin_resp_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(err_msg)) => return Err(SpawnError::WorkerError(err_msg)),
            Err(_) => return Err(SpawnError::OneshotCancelled),
        }

        Ok((
            instance_id.into(),
            stdin_writer,
            stdout_reader,
            stderr_reader,
        ))
    }

    async fn terminate(&self, instance_id: u64) {
        self.wait_ready().await;

        let instance_id: InstanceId = instance_id.into();
        self.inner.lock().unwrap().instances.remove(&instance_id);

        // Also notify the coordinator
        let terminate_msg = CoordinatorMessage::Terminate { id: instance_id };
        let msg_value = serde_wasm_bindgen::to_value(&terminate_msg).unwrap();

        self.inner
            .lock()
            .unwrap()
            .coordinator
            .post_message(&msg_value)
            .unwrap();
    }
}

fn worker_url() -> Result<String, JsValue> {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/wasm");
    let wasm_blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::Uint8Array::from(WORKER_WASM_BYTES)),
        &options,
    )?;
    let wasm_url = web_sys::Url::create_object_url_with_blob(&wasm_blob)?;

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/javascript");
    let sdk_script = format!("{}\n//# sourceURL=coordinator.js", WORKER_JS_GLUE);
    let sdk_blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::JsString::from(sdk_script)),
        &options,
    )?;
    let sdk_url = web_sys::Url::create_object_url_with_blob(&sdk_blob)?;

    let loader_script = WORKER_LOADER_JS
        .replace("{sdk_url}", &sdk_url)
        .replace("{wasm_url}", &wasm_url);

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::JsString::from(loader_script)),
        &options,
    )?;

    web_sys::Url::create_object_url_with_blob(&blob)
}

fn on_message(inner: Arc<Mutex<InnerWorkerRuntime>>, e: web_sys::MessageEvent) {
    let data = e.data();

    let Ok(msg) = serde_wasm_bindgen::from_value::<CoordinatorMessage>(data.clone()) else {
        error!("Worker: Failed to deserialize message");
        return;
    };

    match msg {
        CoordinatorMessage::Ready => {
            info!("Coordinator is ready");
            let mut inner = inner.lock().unwrap();
            if let Some(tx) = inner.ready_tx.take() {
                let _ = tx.send(());
            }
        }
        CoordinatorMessage::Stdout { id } => {
            let Ok(data) = Reflect::get(&data, &"data".into()) else {
                error!("Failed to get stdout data from message");
                return;
            };
            let data: Vec<u8> = data.unchecked_into::<js_sys::Uint8Array>().to_vec();

            let mut inner = inner.lock().unwrap();
            if let Some(instance) = inner.instances.get_mut(&id) {
                if let Err(e) = instance.stdout_writer.write_all(&data) {
                    error!("Failed to write to stdout pipe: {}", e);
                }
            } else {
                warn!("Received stdout for unknown instance ID {:?}", id);
            }
        }
        CoordinatorMessage::Stderr { id } => {
            let Ok(data) = Reflect::get(&data, &"data".into()) else {
                error!("Failed to get stderr data from message");
                return;
            };
            let data: Vec<u8> = data.unchecked_into::<js_sys::Uint8Array>().to_vec();

            let mut inner = inner.lock().unwrap();
            if let Some(instance) = inner.instances.get_mut(&id) {
                if let Err(e) = instance.stderr_writer.write_all(&data) {
                    error!("Failed to write to stderr pipe: {}", e);
                }
            } else {
                warn!(
                    "Received stderr for unknown instance ID {:?}: {}",
                    id,
                    String::from_utf8_lossy(&data)
                );
            }
        }
        CoordinatorMessage::RunPluginResp { id, status } => {
            let mut inner = inner.lock().unwrap();
            if let Some(instance) = inner.instances.get_mut(&id) {
                if let Some(tx) = instance.run_plugin_resp_tx.take() {
                    let _ = tx.send(status);
                } else {
                    warn!("No response channel found for instance {:?}", id);
                }
            } else {
                warn!("Received RunPluginResp for unknown instance ID {:?}", id);
            }
        }
        CoordinatorMessage::Log { message } => {
            info!("[Coordinator] {}", message);
        }
        _ => {
            warn!("Unexpected message received from coordinator");
        }
    }
}

fn stdin_proxy(
    reader: impl AsyncRead + Unpin + Send + 'static,
    id: InstanceId,
    inner: Arc<Mutex<InnerWorkerRuntime>>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut reader = reader;
        let mut buffer = [0u8; 1024];

        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = js_sys::Uint8Array::from(&buffer[..n]);
                    let stdin_msg = CoordinatorMessage::Stdin { id };
                    let msg_value = serde_wasm_bindgen::to_value(&stdin_msg).unwrap();
                    Reflect::set(&msg_value, &"data".into(), &data).unwrap();
                    let transfer_list = js_sys::Array::of1(&data.buffer());

                    let inner = inner.lock().unwrap();
                    if let Err(e) = inner
                        .coordinator
                        .post_message_with_transfer(&msg_value, &transfer_list)
                    {
                        error!("Failed to post stdin message: {:?}", e);
                        break;
                    }
                }
                Err(e) => {
                    error!("Error reading from stdin: {}", e);
                    break;
                }
            }
        }
    });
}
