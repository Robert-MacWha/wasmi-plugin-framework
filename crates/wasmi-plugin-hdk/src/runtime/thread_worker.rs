use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
pub async fn execute_worker_task(ptr: u32) {
    //* SAFETY: We transfer ownership of the closure from the caller's `spawn`
    //* function to this web worker. The pointer must be a FnOnce closure
    //* allocated on the heap.
    let closure = unsafe {
        let raw = ptr as *mut Box<dyn FnOnce() + Send>;
        Box::from_raw(raw)
    };

    (*closure)();
}
