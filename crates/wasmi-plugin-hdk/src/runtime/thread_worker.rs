use futures::{FutureExt, future::LocalBoxFuture};
use tracing::error;
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};
use web_sys::js_sys;

#[wasm_bindgen]
pub fn execute_worker_task(ptr: u32) -> js_sys::Promise {
    let boxed: Box<Box<dyn FnOnce() -> LocalBoxFuture<'static, ()> + Send>> =
        unsafe { Box::from_raw(ptr as *mut _) };
    let fut = boxed();

    wasm_bindgen_futures::future_to_promise(async move {
        if let Err(e) = std::panic::AssertUnwindSafe(fut).catch_unwind().await {
            error!("Panic in thread worker: {:?}", e);
        }
        Ok(JsValue::UNDEFINED)
    })
}
