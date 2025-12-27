use std::io::Write;

use wasm_bindgen::JsCast;
use web_sys::{console::log_1, js_sys};

use crate::runtime::worker_message::WorkerMessage;

pub struct MessageWriter {
    name: String,
    global: web_sys::DedicatedWorkerGlobalScope,
}

unsafe impl Send for MessageWriter {}
unsafe impl Sync for MessageWriter {}

impl MessageWriter {
    pub fn new(name: String) -> Self {
        let global = js_sys::global().unchecked_into::<web_sys::DedicatedWorkerGlobalScope>();
        Self { name, global }
    }
}

impl Write for MessageWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        log_1(js_sys::Uint8Array::from(buf).as_ref());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
