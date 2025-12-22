use std::collections::HashMap;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tracing::warn;

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

type HandlerFn<S> = dyn Send + Sync + Fn(S, Value) -> BoxFuture<'static, Result<Value, RpcError>>;

pub struct Router<S> {
    handlers: HashMap<String, Box<HandlerFn<S>>>,
}

impl<S: Send + Sync + Clone + 'static> Default for Router<S> {
    fn default() -> Self {
        Router {
            handlers: HashMap::new(),
        }
    }
}

impl<S: Send + Sync + Clone + 'static> Router<S> {
    pub fn new() -> Router<S> {
        Self::default()
    }

    /// Register a new RPC method with the router. The method is identified by the
    /// given name, and the handler function should accept the shared state and
    /// deserialized params.
    ///
    /// Handlers should implement: `async fn handler(state: S, params: P) -> Result<R, RpcError>`
    pub fn with_method<P, R, F, Fut>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn(S, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, RpcError>> + MaybeSend + 'static,
    {
        // Handler function that parses json params, calls the provided func,
        // and serializes the result back to json.
        let f = Box::new(
            move |state: S, params: Value| -> BoxFuture<'static, Result<Value, RpcError>> {
                let parsed = serde_json::from_value(params);
                let Ok(p) = parsed else {
                    return Box::pin(async move { Err(RpcError::InvalidParams) });
                };

                let fut = func(state, p);
                Box::pin(async move {
                    let result = fut.await?;
                    Ok(serde_json::to_value(result).unwrap())
                })
            },
        );

        self.handlers.insert(name.to_string(), f);
        self
    }

    pub async fn handle_with_state(
        &self,
        state: S,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let Some(handler) = self.handlers.get(method) else {
            warn!("Method not found: {}", method);
            return Err(RpcError::MethodNotFound);
        };
        handler(state, params).await
    }
}
