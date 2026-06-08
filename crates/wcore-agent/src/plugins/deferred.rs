//! `DeferredPluginRegistry` — storage for plugins whose manifest sets
//! `deferred = true`. The first-use dispatch trigger that wakes them lands
//! in W7/W8 alongside the tool-dispatch loop; W2.5 ships the storage so the
//! shape is locked in and consumers can plug in.

use super::loader::DiscoveredPlugin;

#[derive(Default)]
pub struct DeferredPluginRegistry {
    pub pending: Vec<DiscoveredPlugin>,
}

impl DeferredPluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move every deferred plugin out of `discovered` into the deferred
    /// store. Returns the non-deferred subset for immediate initialization.
    pub fn split_off(&mut self, discovered: Vec<DiscoveredPlugin>) -> Vec<DiscoveredPlugin> {
        let mut immediate = Vec::with_capacity(discovered.len());
        for d in discovered {
            if d.manifest.plugin.deferred {
                self.pending.push(d);
            } else {
                immediate.push(d);
            }
        }
        immediate
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
