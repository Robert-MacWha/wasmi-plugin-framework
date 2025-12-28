use serde_json::Value;
use wasmi_plugin_pdk::{router::BoxFuture, rpc_message::RpcError};

use crate::plugin_id::PluginId;

/// HostHandler is an extension of `wasmi-pdk::RequestHandler` that adds
/// the plugin ID as the first arg in the `handle` method. It should be used
/// by hosts that manage multiple plugins to differentiate request sources.
pub trait HostHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        plugin: PluginId,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>>;
}
