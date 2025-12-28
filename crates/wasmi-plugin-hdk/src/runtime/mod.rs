pub mod non_blocking_pipe;

#[cfg(not(target_arch = "wasm32"))]
pub use native_runtime::NativeRuntime;
#[cfg(not(target_arch = "wasm32"))]
mod native_runtime;

#[cfg(target_arch = "wasm32")]
pub use worker_runtime::WorkerRuntime;
#[cfg(target_arch = "wasm32")]
mod worker_runtime;

use crate::compile::Compiled;
use futures::{AsyncRead, AsyncWrite};
use wasmi_plugin_pdk::router::MaybeSend;

pub trait Runtime {
    type Error: Into<Box<dyn std::error::Error>>;

    /// Spawn a new plugin instance from the given compiled module. Returns the
    /// instance ID and host-side stdio pipes.
    fn spawn(
        &self,
        compiled: Compiled,
    ) -> impl std::future::Future<
        Output = Result<
            (
                u64,
                impl AsyncWrite + Unpin + Send + Sync + 'static,
                impl AsyncRead + Unpin + Send + Sync + 'static,
                impl AsyncRead + Unpin + Send + Sync + 'static,
            ),
            Self::Error,
        >,
    > + MaybeSend;

    /// Forcefully terminate a hanging or otherwise failedplugin instance.
    fn terminate(&self, instance_id: u64) -> impl std::future::Future<Output = ()> + MaybeSend;
}
