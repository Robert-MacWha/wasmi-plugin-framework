use std::collections::HashMap;

use futures::future::BoxFuture;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use wasmi_plugin_pdk::{api::MaybeSend, rpc_message::RpcError};

use crate::{host_handler::HostHandler, plugin::PluginId};

pub struct Server<S: Clone + Send + Sync + 'static> {
    state: S,
    methods: HashMap<
        String,
        Box<
            dyn Send
                + Sync
                + Fn((PluginId, S), Value) -> BoxFuture<'static, Result<Value, RpcError>>,
        >,
    >,
}

impl<S: Default + Clone + Send + Sync + 'static> Default for Server<S> {
    fn default() -> Self {
        Self {
            state: S::default(),
            methods: HashMap::default(),
        }
    }
}

impl<S: Clone + Send + Sync + 'static> Server<S> {
    pub fn new(state: S) -> Self {
        Self {
            state,
            methods: HashMap::default(),
        }
    }

    pub fn with_method<P, R, F, Fut>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn((PluginId, S), P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, RpcError>> + MaybeSend + 'static,
    {
        let f = Box::new(
            move |state: (PluginId, S),
                  params: Value|
                  -> BoxFuture<'static, Result<Value, RpcError>> {
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

        self.methods.insert(name.to_string(), f);
        self
    }
}

impl<S: Clone + Send + Sync + 'static> HostHandler for Server<S> {
    fn handle<'a>(
        &'a self,
        plugin: PluginId,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let state = (plugin, self.state.clone());
            let method = self.methods.get(method).ok_or(RpcError::MethodNotFound)?;
            method(state, params).await
        })
    }
}
