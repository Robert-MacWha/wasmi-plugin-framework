use futures::{FutureExt, future::LocalBoxFuture};
use tracing::{error, info};
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};
use web_sys::js_sys;

#[wasm_bindgen]
pub fn init_compute_worker() {
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_console_config(tracing_wasm::ConsoleConfig::ReportWithoutConsoleColor)
            .set_max_level(tracing::Level::INFO)
            .build(),
    );

    info!("Compute worker initialized");
}

#[wasm_bindgen]
pub fn execute_worker_task(ptr: u32) -> js_sys::Promise {
    info!("Executing worker task...");

    type Task = Box<dyn FnOnce() -> LocalBoxFuture<'static, ()> + Send>;
    let task: Box<Task> = unsafe { Box::from_raw(ptr as *mut Task) };
    let fut = task();

    wasm_bindgen_futures::future_to_promise(async move {
        if let Err(e) = std::panic::AssertUnwindSafe(fut).catch_unwind().await {
            error!("Panic in thread worker: {:?}", e);
        }
        Ok(JsValue::UNDEFINED)
    })
}

#[wasm_bindgen]
pub fn execute_worker_task_with(ptr: u32, extra: JsValue) -> js_sys::Promise {
    info!("Executing worker task with extra...");

    type Task = Box<dyn FnOnce(JsValue) -> LocalBoxFuture<'static, ()> + Send>;
    let task: Box<Task> = unsafe { Box::from_raw(ptr as *mut Task) };
    let fut = task(extra);

    wasm_bindgen_futures::future_to_promise(async move {
        info!("Executing worker task with extra...");
        if let Err(e) = std::panic::AssertUnwindSafe(fut).catch_unwind().await {
            error!("Panic in thread worker: {:?}", e);
        }
        Ok(JsValue::UNDEFINED)
    })
}
