use std::io::{Read, Write};

use serde_json::Value;
use wasmi_plugin_pdk::{
    api::RequestHandler,
    rpc_message::{RpcError, RpcResponse},
};

pub struct Client<R: Read + 'static, W: Write + 'static> {
    c: wasmi_plugin_pdk::client::Client<R, W>,
}

impl<R: Read + 'static, W: Write + 'static> Client<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        let c = wasmi_plugin_pdk::client::Client::new_with(reader, writer);
        Self { c }
    }
}

impl<R: Read + 'static, W: Write + 'static> Client<R, W> {
    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        handler: impl RequestHandler<RpcError>,
    ) -> Result<RpcResponse, RpcError> {
        self.c
            .async_call_with_handler(method, params, handler)
            .await
    }
}
