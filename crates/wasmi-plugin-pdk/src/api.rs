use serde_json::Value;

use crate::rpc_message::RpcError;

#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;

#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = futures::future::BoxFuture<'a, T>;

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

pub trait ApiError: From<RpcError> + Send + Sync {}
impl<T> ApiError for T where T: From<RpcError> + Send + Sync {}

/// JSON-RPC request handler.
///
/// The error type `E` must implement `From<RpcError>`, since the transport layer
/// may need to post errors of this type.
pub trait RequestHandler<E>: Send + Sync {
    fn handle<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, E>>;
}
