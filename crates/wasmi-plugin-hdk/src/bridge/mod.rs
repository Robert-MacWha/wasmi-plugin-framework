#[cfg(not(target_arch = "wasm32"))]
mod native_bridge;
#[cfg(not(target_arch = "wasm32"))]
pub use native_bridge::NativeBridge;

#[cfg(target_arch = "wasm32")]
mod worker_bridge;
#[cfg(target_arch = "wasm32")]
pub use worker_bridge::WorkerBridge;
#[cfg(target_arch = "wasm32")]
pub mod shared_pipe;
#[cfg(target_arch = "wasm32")]
pub mod worker_protocol;

mod non_blocking_pipe;

pub trait Bridge {
    fn terminate(self: Box<Self>);
}
