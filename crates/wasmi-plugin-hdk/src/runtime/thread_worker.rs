use std::panic;

use futures::future::LocalBoxFuture;
use tracing::error;
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

#[wasm_bindgen]
pub async fn execute_worker_task(ptr: u32) -> web_sys::js_sys::Promise {
    // set_panic_hook();

    //* SAFETY: We transfer ownership of the closure from the caller's `spawn`
    //* function to this web worker. The pointer must be a FnOnce closure
    //* allocated on the heap.
    let boxed: Box<Box<dyn FnOnce() -> LocalBoxFuture<'static, ()> + Send>> =
        unsafe { Box::from_raw(ptr as *mut _) };
    let fut = boxed();

    wasm_bindgen_futures::future_to_promise(async move {
        fut.await;
        Ok(JsValue::UNDEFINED)
    })
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
