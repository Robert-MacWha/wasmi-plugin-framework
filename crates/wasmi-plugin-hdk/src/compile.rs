#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub engine: wasmer::Engine,
    pub module: wasmer::Module,
}

impl Compiled {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let engine = wasmer::Engine::default();
        let module = wasmer::Module::new(&engine, wasm_bytes)?;

        Ok(Compiled {
            name: name.to_string(),
            engine,
            module,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let engine = wasmer::Engine::default();
        let module = wasmer::Module::new(&engine, wasm_bytes)?;
        Ok(Compiled {
            name: name.to_string(),
            module,
        })
    }
}
