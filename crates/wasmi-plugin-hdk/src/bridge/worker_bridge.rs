//! Web Worker wasmer bridge, running the Wasm module in a Web Worker.

use std::io::{self, Read, Write};

use futures::StreamExt;
use futures::channel::mpsc;
use thiserror::Error;
use wasm_bindgen_futures::spawn_local;
use web_sys::Worker;
use web_sys::{MessageEvent, js_sys, wasm_bindgen::prelude::Closure};

use web_sys::wasm_bindgen::{JsCast, JsError, JsValue};

use crate::bridge::Bridge;
use crate::wasi::non_blocking_pipe::non_blocking_pipe;

// TODO: Add stdin_writer and stdout_reader methods
pub struct WorkerBridge {
    worker: SendWorker,
    // We keep this to ensure the JS callback isn't garbage collected
    _onmessage_closure: Closure<dyn FnMut(MessageEvent)>,
}

#[derive(Clone)]
struct SendWorker(Worker);

struct WorkerWriter {
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

#[derive(Debug, Error)]
pub enum WorkerBridgeError {
    #[error("JavaScript error: {0:?}")]
    JsValue(JsValue),
}

impl WorkerBridge {
    pub fn new(
        name: &str,
        wasm_bytes: &[u8],
    ) -> Result<
        (
            Self,
            impl Read + Send + Sync + 'static,
            impl Write + Send + Sync + 'static,
        ),
        WorkerBridgeError,
    > {
        let worker_url = "";
        let worker =
            SendWorker(Worker::new(worker_url).map_err(|e| WorkerBridgeError::JsValue(e))?);

        let (stdout_reader, mut stdout_writer) = non_blocking_pipe();

        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Ok(array) = e.data().dyn_into::<js_sys::Uint8Array>() {
                let _ = stdout_writer.write(&array.to_vec());
            }
        }) as Box<dyn FnMut(MessageEvent)>);

        worker
            .0
            .set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded::<Vec<u8>>();
        let worker_inner = worker.clone();

        spawn_local(async move {
            while let Some(data) = stdin_rx.next().await {
                let array = js_sys::Uint8Array::from(&data[..]);
                if let Err(_) = worker_inner.post_message(&array) {
                    break;
                }
            }
        });

        let bridge = WorkerBridge {
            worker,
            _onmessage_closure: onmessage_callback,
        };

        let writer = WorkerWriter { tx: stdin_tx };

        Ok((bridge, stdout_reader, writer))
    }
}

unsafe impl Send for WorkerBridge {}
unsafe impl Sync for WorkerBridge {}

impl Bridge for WorkerBridge {
    fn terminate(self: Box<Self>) {
        self.worker.terminate();
    }
}

impl SendWorker {
    fn post_message(&self, msg: &JsValue) -> Result<(), JsValue> {
        self.0.post_message(msg)
    }

    fn terminate(&self) {
        self.0.terminate();
    }
}

impl Write for WorkerWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tx
            .unbounded_send(buf.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Worker task dropped"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
