use std::io::{Read, Write};

use serde_json::Value;

use crate::{
    rpc_message::RpcResponse,
    transport_driver::{DriverError, TransportDriver},
};

pub struct Transport<R = std::io::Stdin, W = std::io::Stdout> {
    driver: TransportDriver<R, W>,
}

impl Clone for Transport {
    fn clone(&self) -> Self {
        Self {
            driver: self.driver.clone(),
        }
    }
}

impl<R: Read, W: Write> Transport<R, W> {
    pub fn new(reader: R, writer: W) -> (Self, TransportDriver<R, W>) {
        let driver = TransportDriver::new(reader, writer);

        let transport = Self {
            driver: driver.clone(),
        };

        (transport, driver)
    }

    pub fn call(&self, method: &str, params: Value) -> Result<RpcResponse, DriverError> {
        self.driver.call(method, params)
    }

    pub async fn call_async(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcResponse, DriverError> {
        self.driver.call_async(method, params).await
    }
}
