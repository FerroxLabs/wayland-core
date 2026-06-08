//! Stub `ConfigReader` + `MemoryHost` implementations used to build the
//! always-on observability / read views in `PluginContext`.
//!
//! For W2.5 these are intentionally inert — config reads return `None` for
//! every key; memory reads return empty; memory writes succeed but discard.
//! W4 + M3 land the real config-bridge and memory-bridge respectively; the
//! adapter signatures are stable so the host swap is internal.

use wcore_plugin_api::memory_spec::{MemoryItem, MemoryQuery, Partition};
use wcore_plugin_api::registry::config::ConfigReader;
use wcore_plugin_api::registry::memory::MemoryHost;

#[derive(Debug, Default)]
pub struct NullConfigReader;

impl ConfigReader for NullConfigReader {
    fn get_raw(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Debug, Default)]
pub struct NullMemoryHost {
    pub writes: Vec<(Partition, MemoryItem)>,
}

impl MemoryHost for NullMemoryHost {
    fn host_read(
        &self,
        _partition: Partition,
        _query: &MemoryQuery,
    ) -> Result<Vec<MemoryItem>, String> {
        Ok(Vec::new())
    }

    fn host_write(&mut self, partition: Partition, item: MemoryItem) -> Result<(), String> {
        self.writes.push((partition, item));
        Ok(())
    }
}
