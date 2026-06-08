//! Memory partitions (design spec §4.3). Mirror so the api crate stays
//! independent of `wcore-memory`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
pub enum Partition {
    P1,
    P2,
    P3,
    P4,
    P5,
}

impl Partition {
    pub fn as_str(self) -> &'static str {
        match self {
            Partition::P1 => "P1",
            Partition::P2 => "P2",
            Partition::P3 => "P3",
            Partition::P4 => "P4",
            Partition::P5 => "P5",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryItem {
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryQuery {
    pub q: String,
    pub limit: Option<u32>,
}
