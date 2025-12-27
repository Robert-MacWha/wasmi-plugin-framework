#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub engine: wasmer::Engine,
    #[cfg(not(target_arch = "wasm32"))]
    pub module: wasmer::Module,

    #[cfg(target_arch = "wasm32")]
    pub js_module: web_sys::js_sys::WebAssembly::Module,
    #[cfg(target_arch = "wasm32")]
    pub wasm_bytes: Arc<[u8]>,
}

// SAFETY: WebAssembly.Module is one of the few JS objects that is
// "thread-safe" in the browser because it is immutable and
// can be transferred/cloned across workers without issues.
unsafe impl Send for Compiled {}
unsafe impl Sync for Compiled {}

impl Compiled {
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let engine = wasmer::Engine::default();
        let module = wasmer::Module::new(&engine, wasm_bytes)?;

        Ok(Compiled {
            name: name.to_string(),
            engine,
            module,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;
        use web_sys::js_sys::{Uint8Array, WebAssembly};

        let js_bytes = Uint8Array::from(wasm_bytes);
        let promise = WebAssembly::compile(&js_bytes.into());
        let js_module_value = JsFuture::from(promise).await.unwrap();
        let js_module = js_module_value.dyn_into::<WebAssembly::Module>().unwrap();

        Ok(Compiled {
            name: name.to_string(),
            js_module,
            wasm_bytes: Arc::from(wasm_bytes),
        })
    }
}
