pub mod non_blocking_pipe;

#[cfg(not(target_arch = "wasm32"))]
pub use native_runtime::NativeRuntime;
#[cfg(not(target_arch = "wasm32"))]
mod native_runtime;

#[cfg(target_arch = "wasm32")]
pub use worker_runtime::WorkerRuntime;
#[cfg(target_arch = "wasm32")]
mod message_writer;
#[cfg(target_arch = "wasm32")]
mod thread_worker;
#[cfg(target_arch = "wasm32")]
mod worker_message;
#[cfg(target_arch = "wasm32")]
pub mod worker_pool;
#[cfg(target_arch = "wasm32")]
mod worker_runtime;

use crate::compile::Compiled;
use futures::{AsyncRead, AsyncWrite};

pub trait Runtime {
    type Error: Into<Box<dyn std::error::Error>>;

    fn spawn(
        &self,
        compiled: Compiled,
    ) -> impl std::future::Future<
        Output = Result<
            (
                u64,
                impl AsyncWrite + Unpin + Send + Sync + 'static,
                impl AsyncRead + Unpin + Send + Sync + 'static,
            ),
            Self::Error,
        >,
    >;
    fn terminate(self, instance_id: u64) -> impl std::future::Future<Output = ()>;
}
