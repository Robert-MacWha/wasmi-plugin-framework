use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use wasmi_plugin_pdk::{
    router::{BoxFuture, MaybeSend, Router},
    rpc_message::RpcError,
};

use crate::{host_handler::HostHandler, plugin::PluginId};

pub struct HostServer<S: Clone + Send + Sync + 'static> {
    state: S,
    router: Router<(PluginId, S)>,
}

impl<S: Default + Clone + Send + Sync + 'static> Default for HostServer<S> {
    fn default() -> Self {
        Self {
            state: S::default(),
            router: Router::new(),
        }
    }
}

impl<S: Clone + Send + Sync + 'static> HostServer<S> {
    pub fn new(state: S) -> Self {
        Self {
            router: Router::new(),
            state,
        }
    }

    pub fn with_method<P, R, F, Fut>(mut self, name: &str, func: F) -> Self
    where
        P: DeserializeOwned + 'static,
        R: Serialize + 'static,
        F: Fn((PluginId, S), P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, RpcError>> + MaybeSend + 'static,
    {
        self.router = self.router.with_method(name, func);
        self
    }
}

impl<S: Clone + Send + Sync + 'static> HostHandler for HostServer<S> {
    fn handle<'a>(
        &'a self,
        plugin: PluginId,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            let state = (plugin, self.state.clone());
            self.router.handle_with_state(state, method, params).await
        })
    }
}
