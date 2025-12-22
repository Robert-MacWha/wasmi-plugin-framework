#[cfg(not(target_arch = "wasm32"))]
mod native_bridge;

#[cfg(target_arch = "wasm32")]
mod worker_bridge;

#[cfg(not(target_arch = "wasm32"))]
pub use native_bridge::NativeBridge;

#[cfg(target_arch = "wasm32")]
pub use worker_bridge::WorkerBridge;

pub trait Bridge {
    fn terminate(self: Box<Self>);
}

// use std::io::{Read, Write};

// // Define a common error type for the bridge setup
// pub type BridgeError = Box<dyn std::error::Error + Send + Sync>;

// pub fn spawn_bridge(
//     name: &str,
//     wasm_bytes: &[u8],
// ) -> Result<
//     (
//         Box<dyn Bridge + Send + Sync>,
//         impl Read + Send + Sync + 'static,
//         impl Write + Send + Sync + 'static,
//     ),
//     BridgeError,
// > {
//     #[cfg(not(target_arch = "wasm32"))]
//     {
//         let (bridge, reader, writer) = native_bridge::NativeBridge::new(name, wasm_bytes)
//             .map_err(|e| Box::new(e) as BridgeError)?;
//         Ok((Box::new(bridge), reader, writer))
//     }

//     #[cfg(target_arch = "wasm32")]
//     {
//         let (bridge, reader, writer) = worker_bridge::WorkerBridge::new(name, wasm_bytes)
//             .map_err(|js_val| format!("{:?}", js_val))?;
//         Ok((Box::new(bridge), reader, writer))
//     }
// }
