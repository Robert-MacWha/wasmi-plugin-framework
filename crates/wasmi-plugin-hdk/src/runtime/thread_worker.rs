use std::panic;

use tracing::error;
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
pub async fn execute_worker_task(ptr: u32) {
    set_panic_hook();

    //* SAFETY: We transfer ownership of the closure from the caller's `spawn`
    //* function to this web worker. The pointer must be a FnOnce closure
    //* allocated on the heap.
    let closure = unsafe {
        let raw = ptr as *mut Box<dyn FnOnce() + Send>;
        Box::from_raw(raw)
    };

    (*closure)();
}

fn set_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        // 1. Try to get the message as a &str (for panic!("msg"))
        // 2. Or as a String (for panic!("format {}", arg))
        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic payload".to_string()
        };

        // Get location information
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        // Log via tracing
        error!(
            target: "panic",
            panic_message = %message,
            panic_location = %location,
            "A panic occurred"
        );
    }));
}
