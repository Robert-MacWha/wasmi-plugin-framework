use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use futures::channel::oneshot::{self};
use thiserror::Error;
use tracing::{error, info};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{
    MessageEvent,
    js_sys::{self, Reflect, SharedArrayBuffer, Uint8Array},
};

use crate::{
    compile::Compiled,
    runtime::{shared_pipe::SharedPipe, worker_protocol::WorkerMessage},
};

pub type WorkerId = u32;

pub struct WorkerPool {
    workers: Vec<Arc<WorkerHandle>>,
    next_id: AtomicU64,
    script_url: String,
}

pub struct WorkerSession {
    pub id: WorkerId,
    handle: Arc<WorkerHandle>,
}

struct WorkerHandle {
    id: WorkerId,
    state: Arc<Mutex<WorkerState>>,
    inner: web_sys::Worker,
    stdin: SharedArrayBuffer,
    stdout: SharedArrayBuffer,

    //? Store handle to prevent javascript garbage collection
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("JavaScript error")]
    JsValue(JsValue),
    #[error("Serde serialization error: {0}")]
    Serde(#[from] serde_wasm_bindgen::Error),
    #[error("Module serialization error: {0}")]
    ModuleSerialization(#[from] wasmer::SerializeError),
    #[error("Receiver error: {0}")]
    Receiver(#[from] futures::channel::oneshot::Canceled),
}

#[derive(Clone, Copy, PartialEq)]
enum WorkerState {
    Idle,
    Busy,
    Terminated,
}

const SHARED_PIPE_CAPACITY: u32 = 16384;
const WORKER_LOADER_JS: &str = include_str!("worker_loader.js");
const WORKER_JS_GLUE: &str = include_str!("../../../worker/pkg/worker.js");
const WORKER_WASM_BYTES: &[u8] = include_bytes!("../../../worker/pkg/worker_bg.wasm");

impl WorkerPool {
    pub fn new() -> Result<Self, JsValue> {
        let pool = WorkerPool {
            workers: Vec::new(),
            next_id: AtomicU64::new(0),
            script_url: create_worker_script()?,
        };
        Ok(pool)
    }

    pub async fn run(
        &mut self,
        compiled: Compiled,
    ) -> Result<(WorkerSession, SharedPipe, SharedPipe), PoolError> {
        let worker = self.get_or_create_worker().await?;

        let stdin = SharedPipe::new("HOST_STDIN", &worker.stdin);
        let stdout = SharedPipe::new("HOST_STDOUT", &worker.stdout);

        // Reset the header (read/write pointers) in the shared memory
        stdin.reset();
        stdout.reset();

        worker.run(compiled)?;

        Ok((
            WorkerSession {
                id: worker.id,
                handle: worker,
            },
            stdin,
            stdout,
        ))
    }

    async fn get_or_create_worker(&mut self) -> Result<Arc<WorkerHandle>, PoolError> {
        self.workers
            .retain(|w| w.state() != WorkerState::Terminated);

        //? Look for an idle worker
        if let Some(idle) = self.workers.iter().find(|w| w.state() == WorkerState::Idle) {
            idle.set_state(WorkerState::Busy);
            return Ok(idle.clone());
        }

        //? If none found, create a new worker
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) as WorkerId;
        let worker = WorkerHandle::new(id, &self.script_url).await?;
        let worker = Arc::new(worker);
        worker.set_state(WorkerState::Busy);
        self.workers.push(worker.clone());

        Ok(worker)
    }
}

impl WorkerSession {
    pub fn terminate(&self) {
        self.handle.terminate();
    }
}

unsafe impl Send for WorkerHandle {}
unsafe impl Sync for WorkerHandle {}

impl WorkerHandle {
    pub async fn new(id: WorkerId, script_url: &str) -> Result<Self, PoolError> {
        let inner = web_sys::Worker::new(script_url).map_err(PoolError::JsValue)?;
        let state = Arc::new(Mutex::new(WorkerState::Idle));

        let stdin = SharedPipe::new_with_capacity("HOST_STDIN", SHARED_PIPE_CAPACITY).buffer();
        let stdout = SharedPipe::new_with_capacity("HOST_STDOUT", SHARED_PIPE_CAPACITY).buffer();

        let (booted_tx, booted_rx) = oneshot::channel();
        let (init_tx, init_rx) = oneshot::channel();
        let on_message = Closure::wrap(WorkerHandle::on_message_handler(
            booted_tx,
            init_tx,
            id,
            state.clone(),
        ));
        inner.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let worker = WorkerHandle {
            id,
            state,
            inner,
            stdin,
            stdout,
            _on_message: on_message,
        };

        booted_rx.await?;
        worker.initalize()?;
        init_rx.await?;

        Ok(worker)
    }

    pub fn state(&self) -> WorkerState {
        self.state.lock().unwrap().clone()
    }

    pub fn set_state(&self, state: WorkerState) {
        let mut guard = self.state.lock().unwrap();
        *guard = state;
    }

    pub fn run(&self, compiled: Compiled) -> Result<(), PoolError> {
        self.set_state(WorkerState::Busy);

        let wasm_bytes = compiled.module.serialize()?;
        let msg = serde_wasm_bindgen::to_value(&WorkerMessage::Load {
            wasm_module: wasm_bytes.to_vec(),
            name: compiled.name.clone(),
        })?;

        // Transfer WASM buffer to worker
        let uint8_array = Uint8Array::new(
            &Reflect::get(&msg, &"wasm_module".into()).map_err(PoolError::JsValue)?,
        );
        let transfer_list = js_sys::Array::of1(&uint8_array.buffer());

        self.inner
            .post_message_with_transfer(&msg, &transfer_list)
            .map_err(PoolError::JsValue)?;

        Ok(())
    }

    pub fn terminate(&self) {
        //? Create temp SharedPipe instances to dump their state to logs
        // let stdin = SharedPipe::new("DUMP_STDIN", &self.stdin);
        // let stdout = SharedPipe::new("DUMP_STDOUT", &self.stdout);

        // stdin.dump();
        // stdout.dump();

        self.inner.terminate();
        self.set_state(WorkerState::Terminated);
    }

    fn on_message_handler(
        booted_tx: oneshot::Sender<()>,
        init_tx: oneshot::Sender<()>,
        id: WorkerId,
        state: Arc<Mutex<WorkerState>>,
    ) -> Box<dyn FnMut(MessageEvent)> {
        let mut booted_tx = Some(booted_tx);
        let mut init_tx = Some(init_tx);
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
                    info!("[WORKER_{}] {}", id, message);
                }
                WorkerMessage::PluginLog { message } => {
                    info!("[PLUGIN_{}] {}", id, message);
                }
                WorkerMessage::Booted => {
                    if let Some(tx) = booted_tx.take() {
                        info!("[WORKER_{}] Booted", id);
                        let _ = tx.send(());
                    }
                }
                WorkerMessage::Initialized => {
                    if let Some(tx) = init_tx.take() {
                        info!("[WORKER_{}] Initialized", id);
                        let _ = tx.send(());
                    }
                }
                WorkerMessage::Idle => {
                    let mut guard = state.lock().unwrap();
                    // Only transition to Idle if currently Busy
                    if *guard == WorkerState::Busy {
                        *guard = WorkerState::Idle;
                        info!("[WORKER_{}] Idle", id);
                    }
                }
                _ => {
                    error!("[WORKER_{}] Unexpected message", id);
                }
            }
        })
    }

    fn initalize(&self) -> Result<(), PoolError> {
        let init_msg = serde_wasm_bindgen::to_value(&WorkerMessage::Initialize)?;

        Reflect::set(&init_msg, &"stdin".into(), &self.stdin).map_err(PoolError::JsValue)?;
        Reflect::set(&init_msg, &"stdout".into(), &self.stdout).map_err(PoolError::JsValue)?;

        self.inner
            .post_message(&init_msg)
            .map_err(PoolError::JsValue)?;
        Ok(())
    }
}

/// Creates a blob URL containing the worker script that lodas and executes the
/// wasm module.
fn create_worker_script() -> Result<String, JsValue> {
    //? Create a blob URL for the worker WASM module
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/wasm");
    let wasm_blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::Uint8Array::from(WORKER_WASM_BYTES)),
        &options,
    )?;
    let wasm_url = web_sys::Url::create_object_url_with_blob(&wasm_blob)?;

    //?
    let loader_script = WORKER_LOADER_JS
        .replace("{ WORKER_JS_GLUE }", WORKER_JS_GLUE)
        .replace("{ WASM_URL }", &wasm_url);

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(
        &js_sys::Array::of1(&js_sys::JsString::from(loader_script)),
        &options,
    )?;

    web_sys::Url::create_object_url_with_blob(&blob)
}
