#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    pub module_hash: wasmer_types::ModuleHash,
    #[cfg(not(target_arch = "wasm32"))]
    pub engine: Engine,
    #[cfg(not(target_arch = "wasm32"))]
    pub module: Module,
}

impl Compiled {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;
        let module_hash = wasmer_types::ModuleHash::xxhash(wasm_bytes);

        Ok(Compiled {
            name: name.to_string(),
            module_hash,
            engine,
            module,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let module_hash = wasmer_types::ModuleHash::xxhash(wasm_bytes);

        Ok(Compiled {
            name: name.to_string(),
            module_hash,
        })
    }
}
