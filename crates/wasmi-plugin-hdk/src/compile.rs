use wasmi::{Config, Engine, Module};

pub fn compile_plugin(wasm_bytes: Vec<u8>) -> Result<(Engine, Module), wasmi::Error> {
    let mut config = Config::default();
    config.consume_fuel(true);
    // https://github.com/wasmi-labs/wasmi/issues/1647
    // TODO: Switch to lazy execution, seems faster.
    config.compilation_mode(wasmi::CompilationMode::Eager);
    config.set_max_cached_stacks(100);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, wasm_bytes)?;

    Ok((engine, module))
}
