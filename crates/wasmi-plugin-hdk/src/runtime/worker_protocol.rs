use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    /// Send by the worker when it has booted into WASM and is listening for messages
    Booted,
    /// Send by the host to initialize the worker with necessary SABs. Attaches the
    /// shared memory pipes for stdin, stdout, and stderr (can't be sent via serde
    /// for reasons).
    Initialize,
    /// Send by the worker when it has initialized
    Initialized,
    Load {
        /// Serialized wasm module.  Deserialize with `wasmer::Module::deserialize`
        #[serde(with = "serde_bytes")]
        wasm_module: Vec<u8>,
        name: String,
    },
    Log {
        message: String,
    },
    PluginLog {
        message: String,
    },
    Idle,
}
