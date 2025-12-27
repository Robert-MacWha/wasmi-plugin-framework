#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub engine: wasmer::Engine,
    pub module: wasmer::Module,
}

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

        //? Because we're in wasm32-unknown-unknown AND we want to have Shared Memory
        //? (target-feature=+atomics,+bulk-memory),
        //? we have to use the WebAssembly.compile async function and pass the compiled
        //? native mmodule to wasmer. Wasmer only supports compiling via `WebAssembly::new()`,
        //? which is illegal for use with Shared Memory.
        let js_bytes = web_sys::js_sys::Uint8Array::from(wasm_bytes);
        let promise = web_sys::js_sys::WebAssembly::compile(&js_bytes.into());
        let js_module_value = wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
        let js_module = js_module_value
            .dyn_into::<web_sys::js_sys::WebAssembly::Module>()
            .unwrap();

        let module: wasmer::Module = (js_module, wasm_bytes).into();

        Ok(Compiled {
            name: name.to_string(),
            module,
        })
    }
}
