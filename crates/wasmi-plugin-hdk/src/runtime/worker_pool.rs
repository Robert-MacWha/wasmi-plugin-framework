use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::oneshot;
use thiserror::Error;
use tracing::{error, info, warn};
use wasm_bindgen::{
    JsCast, JsValue,
    prelude::{Closure, wasm_bindgen},
};
use web_sys::{
    Blob, Url, Worker, WorkerOptions, WorkerType,
    js_sys::{self, Array, Reflect, Uint8Array},
};

use crate::runtime::worker_message::WorkerMessage;

pub type WorkerId = u64;

pub struct WorkerPool {
    state: Arc<Mutex<PoolState>>,
}

struct PoolState {
    workers: Vec<Arc<WorkerHandle>>,
}

enum WorkerState {
    Idle,
    Busy,
    Terminated,
}

pub struct WorkerHandle {
    pub id: WorkerId,
    state: Arc<Mutex<WorkerState>>,
    worker: web_sys::Worker,
    _on_message: wasm_bindgen::prelude::Closure<dyn FnMut(web_sys::MessageEvent)>,
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("JavaScript error: {0:?}")]
    JsError(JsValue),
}

static GLOBAL_POOL: OnceLock<WorkerPool> = OnceLock::new();

unsafe impl Send for WorkerHandle {}
unsafe impl Sync for WorkerHandle {}

impl WorkerPool {
    pub fn global() -> &'static WorkerPool {
        GLOBAL_POOL.get_or_init(|| WorkerPool {
            state: Arc::new(Mutex::new(PoolState { workers: vec![] })),
        })
    }

    pub async fn run<F>(&self, f: F) -> Result<Arc<WorkerHandle>, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        let handle = self.get_or_spawn_worker().await?;
        handle.run(f)?;
        Ok(handle)
    }

    pub fn terminate(&self, worker_id: WorkerId) {
        let mut state = self.state.lock().unwrap();
        if let Some(pos) = state.workers.iter().position(|w| w.id == worker_id) {
            let handle = state.workers.remove(pos);
            handle.terminate();
        }
    }

    async fn get_or_spawn_worker(&self) -> Result<Arc<WorkerHandle>, SpawnError> {
        // Remove any that are terminated
        self.state.lock().unwrap().workers.retain(|w| {
            let worker_state = w.state.lock().unwrap();
            !matches!(*worker_state, WorkerState::Terminated)
        });

        // Try to find an idle worker
        for worker in self.state.lock().unwrap().workers.iter() {
            let mut worker_state = worker.state.lock().unwrap();
            if matches!(*worker_state, WorkerState::Idle) {
                *worker_state = WorkerState::Busy;
                return Ok(worker.clone());
            }
        }

        // Otherwise, spawn a new one
        let id = self.state.lock().unwrap().workers.len() as u64;
        let handle = WorkerHandle::spawn(id).await?;
        let handle = Arc::new(handle);
        self.state.lock().unwrap().workers.push(handle.clone());
        Ok(handle)
    }
}

impl WorkerHandle {
    pub async fn spawn(id: u64) -> Result<WorkerHandle, SpawnError> {
        let name = format!("worker-{}", id);

        // 2. Create the worker
        let options = WorkerOptions::new();
        options.set_name(&name);
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&worker_url(), &options).unwrap();

        // 3. Set up onmessage handler
        let (init_tx, init_rx) = oneshot::channel();
        let state = Arc::new(Mutex::new(WorkerState::Busy));
        let on_message = on_message_handler(init_tx, state.clone(), name.clone());
        let on_message = Closure::wrap(on_message);
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        // 4. Send the "init" message with memory and module
        let msg = js_sys::Object::new();
        Reflect::set(&msg, &"type".into(), &"init".into()).unwrap();
        Reflect::set(&msg, &"memory".into(), &wasm_bindgen::memory()).unwrap();
        Reflect::set(&msg, &"module".into(), &current_module()).unwrap();
        Reflect::set(&msg, &"sdkUrl".into(), &sdk_url().into()).unwrap();
        worker.post_message(&msg).unwrap();

        // 5. Wait for initialization
        init_rx.await.unwrap();

        Ok(WorkerHandle {
            id,
            state,
            worker,
            _on_message: on_message,
        })
    }

    fn run(&self, f: impl FnOnce() + Send + 'static) -> Result<(), SpawnError> {
        // 1. Box the function and get a raw pointer
        let box_f: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
        let ptr = Box::into_raw(box_f) as u32;

        // 2. Load my worker
        let worker = &self.worker;

        // 3. Send the "run" message with raw ptr
        let msg = js_sys::Object::new();
        Reflect::set(&msg, &"type".into(), &"run".into()).unwrap();
        Reflect::set(&msg, &"taskPtr".into(), &ptr.into()).unwrap();
        worker.post_message(&msg).unwrap();

        Ok(())
    }

    pub fn terminate(&self) {
        self.worker.terminate();
        let mut state = self.state.lock().unwrap();
        *state = WorkerState::Terminated;
    }
}

fn worker_url() -> String {
    let script = include_str!("./worker.js");

    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/javascript");
    let blob = Blob::new_with_u8_array_sequence_and_options(
        Array::from_iter([Uint8Array::from(script.as_bytes())]).as_ref(),
        &options,
    )
    .unwrap();

    Url::create_object_url_with_blob(&blob).unwrap()
}

fn sdk_url() -> String {
    #[wasm_bindgen]
    extern "C" {
        // Adding the thread_local_v2 attribute satisfies the deprecation warning
        #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
        static IMPORT_META_URL: String;
    }

    IMPORT_META_URL.with(|url| url.clone())
}

fn current_module() -> JsValue {
    wasm_bindgen::module().dyn_into().unwrap()
}

fn on_message_handler(
    init_tx: oneshot::Sender<()>,
    state: Arc<Mutex<WorkerState>>,
    name: String,
) -> Box<dyn FnMut(web_sys::MessageEvent)> {
    let mut init_tx = Some(init_tx);

    Box::new(move |e: web_sys::MessageEvent| {
        let Ok(msg) = serde_wasm_bindgen::from_value::<WorkerMessage>(e.data()) else {
            error!(
                "WorkerBridge: Received unknown message from worker: {:?}",
                e.data()
            );
            return;
        };

        match msg {
            WorkerMessage::Log { message, level, ts } => {
                log_worker_message(&name, message, level, ts);
            }
            WorkerMessage::Ready => {
                info!(target: "WORKER", "[{}] Ready", name);
                if let Some(tx) = init_tx.take() {
                    let _ = tx.send(());
                }
            }
            WorkerMessage::Idle => {
                info!(target: "WORKER", "[{}] Idle", name);
                let mut worker_state = state.lock().unwrap();
                *worker_state = WorkerState::Idle;
            }
        }
    })
}

fn log_worker_message(name: &str, msg: String, level: String, ts: f64) {
    let _ = ts;

    match level.as_str() {
        "warn" => warn!(target: "WORKER", "[{}] {}", name, msg),
        "error" => error!(target: "WORKER", "[{}] {}", name, msg),
        _ => info!(target: "WORKER", "[{}] {}", name, msg),
    }
}
