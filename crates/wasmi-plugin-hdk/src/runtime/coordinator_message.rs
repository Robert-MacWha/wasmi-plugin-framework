use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InstanceId(u32);

impl InstanceId {
    pub fn new() -> Self {
        Self(getrandom::u32().expect("Could not get random for InstanceId"))
    }
}

impl From<u64> for InstanceId {
    fn from(value: u64) -> Self {
        Self(value as u32)
    }
}

impl From<InstanceId> for u64 {
    fn from(id: InstanceId) -> Self {
        id.0 as u64
    }
}

impl Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CoordinatorMessage {
    /// WASM module attached as external bytes.
    RunPlugin {
        id: InstanceId,
    },
    RunPluginResp {
        id: InstanceId,
        status: Result<(), String>,
    },
    Stdin {
        id: InstanceId,
    },
    Stdout {
        id: InstanceId,
    },
    Stderr {
        id: InstanceId,
    },
    Log {
        message: String,
    },
    Terminate {
        id: InstanceId,
    },

    Ready,
}
