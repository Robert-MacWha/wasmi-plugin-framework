use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum RpcMessage {
    RpcRequest(RpcRequest),
    RpcResponse(RpcResponse),
    RpcErrorResponse(RpcErrorResponse),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RpcErrorResponse {
    pub jsonrpc: String,
    pub id: u64,
    pub error: RpcError,
}

#[non_exhaustive]
#[derive(Error, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    #[error("Parse error")]
    ParseError,
    #[error("Method not found")]
    MethodNotFound,
    #[error("Invalid params")]
    InvalidParams,
    /// Custom error with message. Messages are unstable and inconsistent across
    /// different plugins and versions. They should NEVER be relied upon for
    /// program logic.
    ///
    /// TODO: Consider poisoning equality checks on this variant to prevent
    /// accidental reliance on error message contents.
    #[error("{0}")]
    Custom(String),
}

pub trait RpcErrorContext<T> {
    fn context(self, msg: impl Into<String>) -> Result<T, RpcError>;
    fn with_context<F, S>(self, f: F) -> Result<T, RpcError>
    where
        F: FnOnce() -> S,
        S: Into<String>;
}

pub trait ToRpcResult<T> {
    fn rpc_err(self) -> Result<T, RpcError>;
}

impl RpcMessage {
    pub fn request(id: u64, method: String, params: Value) -> Self {
        RpcMessage::RpcRequest(RpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method,
            params,
        })
    }

    pub fn response(id: u64, result: Value) -> Self {
        RpcMessage::RpcResponse(RpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        })
    }

    pub fn error_response(id: u64, error: RpcError) -> Self {
        RpcMessage::RpcErrorResponse(RpcErrorResponse {
            jsonrpc: "2.0".to_string(),
            id,
            error,
        })
    }
}

impl RpcError {
    pub fn custom<S: Into<String>>(msg: S) -> Self {
        RpcError::Custom(msg.into())
    }
}

impl From<RpcErrorResponse> for RpcError {
    fn from(value: RpcErrorResponse) -> Self {
        value.error
    }
}

impl From<String> for RpcError {
    fn from(s: String) -> Self {
        RpcError::Custom(s)
    }
}

impl From<&str> for RpcError {
    fn from(s: &str) -> Self {
        RpcError::Custom(s.to_string())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for RpcError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RpcError::Custom(e.to_string())
    }
}

impl<T, E: std::fmt::Display> RpcErrorContext<T> for Result<T, E> {
    fn context(self, msg: impl Into<String>) -> Result<T, RpcError> {
        self.map_err(|e| RpcError::Custom(format!("{}: {}", msg.into(), e)))
    }

    fn with_context<F, S>(self, f: F) -> Result<T, RpcError>
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        self.map_err(|e| RpcError::Custom(format!("{}: {}", f().into(), e)))
    }
}

impl<T> RpcErrorContext<T> for Option<T> {
    fn context(self, msg: impl Into<String>) -> Result<T, RpcError> {
        self.ok_or_else(|| RpcError::Custom(msg.into()))
    }

    fn with_context<F, S>(self, f: F) -> Result<T, RpcError>
    where
        F: FnOnce() -> S,
        S: Into<String>,
    {
        self.ok_or_else(|| RpcError::Custom(f().into()))
    }
}

impl<T, E> ToRpcResult<T> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn rpc_err(self) -> Result<T, RpcError> {
        self.map_err(|e| RpcError::from(Box::new(e) as Box<dyn std::error::Error + Send + Sync>))
    }
}
