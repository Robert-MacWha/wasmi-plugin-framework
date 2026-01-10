use std::fmt::Display;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::plugin_id::PluginId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstanceId {
    pub plugin: PluginId,
    pub id: Uuid,
}

impl InstanceId {
    pub fn new(plugin: PluginId) -> Self {
        InstanceId {
            plugin,
            id: Uuid::new_v4(),
        }
    }
}

impl Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{}:{}", self.plugin, self.id) // full: {:#}
        } else {
            let uuid_str = self.id.as_simple().to_string();
            write!(f, "{},{}", self.plugin, &uuid_str[uuid_str.len() - 6..]) // short {}
        }
    }
}
