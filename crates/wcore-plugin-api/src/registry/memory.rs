//! `ScopedMemoryClient` — partition-gated memory access.
//!
//! The permission contract lives inside the client: `read`/`write` reject any
//! partition not in the plugin's manifest-declared lists. P5 (user model) is
//! never plugin-writable.

use std::collections::HashSet;

use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;
use crate::memory_spec::{MemoryItem, MemoryQuery, Partition};

pub trait MemoryHost: Send + Sync {
    fn host_read(
        &self,
        partition: Partition,
        query: &MemoryQuery,
    ) -> Result<Vec<MemoryItem>, String>;
    fn host_write(&mut self, partition: Partition, item: MemoryItem) -> Result<(), String>;
}

pub struct ScopedMemoryClient<'a> {
    plugin_name: String,
    readable: HashSet<Partition>,
    writable: HashSet<Partition>,
    host: &'a mut dyn MemoryHost,
}

impl<'a> ScopedMemoryClient<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn MemoryHost) -> Self {
        let readable = parse_set(&manifest.permissions.memory_partitions_readable);
        let writable = parse_set(&manifest.permissions.memory_partitions_writable);
        Self {
            plugin_name: manifest.plugin.name.clone(),
            readable,
            writable,
            host,
        }
    }

    pub fn read(&self, partition: Partition, query: &MemoryQuery) -> PluginResult<Vec<MemoryItem>> {
        if !self.readable.contains(&partition) {
            return Err(PluginError::PermissionDenied {
                plugin: self.plugin_name.clone(),
                operation: format!("memory.read({})", partition.as_str()),
            });
        }
        self.host
            .host_read(partition, query)
            .map_err(|e| PluginError::PermissionDenied {
                plugin: self.plugin_name.clone(),
                operation: format!("memory.read({}): {e}", partition.as_str()),
            })
    }

    pub fn write(&mut self, partition: Partition, item: MemoryItem) -> PluginResult<()> {
        if !self.writable.contains(&partition) {
            return Err(PluginError::PermissionDenied {
                plugin: self.plugin_name.clone(),
                operation: format!("memory.write({})", partition.as_str()),
            });
        }
        self.host
            .host_write(partition, item)
            .map_err(|e| PluginError::PermissionDenied {
                plugin: self.plugin_name.clone(),
                operation: format!("memory.write({}): {e}", partition.as_str()),
            })
    }
}

fn parse_set(list: &[String]) -> HashSet<Partition> {
    list.iter()
        .filter_map(|s| match s.as_str() {
            "P1" => Some(Partition::P1),
            "P2" => Some(Partition::P2),
            "P3" => Some(Partition::P3),
            "P4" => Some(Partition::P4),
            "P5" => Some(Partition::P5),
            _ => None,
        })
        .collect()
}
