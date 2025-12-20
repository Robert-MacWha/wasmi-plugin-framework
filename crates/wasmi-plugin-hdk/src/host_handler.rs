use futures::future::BoxFuture;
use serde_json::Value;
use wasmi_plugin_pdk::rpc_message::RpcError;

use crate::plugin::PluginId;

pub trait HostHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        plugin: PluginId,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>>;
}
