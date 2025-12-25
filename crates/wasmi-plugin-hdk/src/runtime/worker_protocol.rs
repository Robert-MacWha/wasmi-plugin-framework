use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    Ready,
    /// Load wasm bytes into the worker. Also has attached stdin/stdout/stderr
    /// pipes, which are SharedArrayBuffers but can't be serialized.
    Load {
        /// Serialized wasm module.  
        #[serde(with = "serde_bytes")]
        wasm_module: Vec<u8>,
    },
    Log {
        message: String,
    },
    Idle,
}
