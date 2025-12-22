use wasmer::{Engine, Module};

#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    pub engine: Engine,
    pub module: Module,
    pub module_hash: wasmer_types::ModuleHash,
}

impl Compiled {
    pub fn new(name: &str, wasm_bytes: &[u8]) -> Result<Self, wasmer::CompileError> {
        let engine = Engine::default();
        let module = Module::new(&engine, wasm_bytes)?;
        let module_hash = wasmer_types::ModuleHash::xxhash(wasm_bytes);

        Ok(Compiled {
            name: name.to_string(),
            engine,
            module,
            module_hash,
        })
    }
}
