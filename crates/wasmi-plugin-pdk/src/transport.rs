use std::io::{Read, Write};

use serde_json::Value;

use crate::{
    api::ApiError,
    rpc_message::RpcResponse,
    transport_driver::{DriverError, TransportDriver},
};

pub trait SyncTransport<E: ApiError> {
    fn call(&self, method: &str, params: Value) -> Result<RpcResponse, E>;
}

pub trait AsyncTransport<E: ApiError> {
    fn call_async(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<RpcResponse, E>> + Send;
}

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
}

impl SyncTransport<DriverError> for Transport {
    fn call(&self, method: &str, params: Value) -> Result<RpcResponse, DriverError> {
        self.driver.call(method, params)
    }
}

impl AsyncTransport<DriverError> for Transport {
    fn call_async(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<RpcResponse, DriverError>> + Send {
        self.driver.call_async(method, params)
    }
}
