use std::io::Write;

use wasm_bindgen::JsCast;
use web_sys::js_sys;

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
        let msg = String::from_utf8_lossy(buf).to_string();
        let msg = serde_wasm_bindgen::to_value(&WorkerMessage::PluginLog { message: msg }).unwrap();

        self.global.post_message(&msg).unwrap();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
