use std::fmt::Display;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(Uuid);

impl Default for PluginId {
    fn default() -> Self {
        PluginId(Uuid::new_v4())
    }
}

impl PluginId {
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<u128> for PluginId {
    fn from(value: u128) -> Self {
        PluginId(Uuid::from_u128(value))
    }
}

impl Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{}", self.0) // full: {:#}
        } else {
            let uuid_str = self.0.as_simple().to_string();
            write!(f, "{}", &uuid_str[uuid_str.len() - 6..]) // short {}
        }
    }
}
