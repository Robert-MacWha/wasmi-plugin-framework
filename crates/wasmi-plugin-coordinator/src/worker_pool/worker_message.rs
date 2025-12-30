use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    Log { message: String },
    Idle,
}
