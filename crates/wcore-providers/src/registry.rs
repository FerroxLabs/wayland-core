//! Open ProviderRegistry trait — replaces closed ProviderType enum dispatch.
//!
//! New providers register via `ProviderRegistry::register` at runtime instead
//! of requiring a new enum variant. The registry and the legacy enum coexist
//! unconditionally.

use std::collections::HashMap;
use std::sync::Arc;

use crate::LlmProvider;

/// Type alias for a provider factory function.
pub type ProviderFactory = Arc<dyn Fn() -> Arc<dyn LlmProvider> + Send + Sync>;

/// Open registry of LLM providers by string id.
pub trait ProviderRegistry: Send + Sync {
    /// Register a provider factory under the given id. Returns Err if id is empty
    /// or already registered.
    fn register(&mut self, id: &str, factory: ProviderFactory) -> Result<(), RegistryError>;
    /// Look up a registered provider by id and construct an instance.
    fn get(&self, id: &str) -> Option<Arc<dyn LlmProvider>>;
    /// List all registered provider ids.
    fn list_ids(&self) -> Vec<String>;
    /// Remove a provider by id. Returns true if removed, false if not found.
    fn remove(&mut self, id: &str) -> bool;
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("provider id is empty")]
    EmptyId,
    #[error("provider id '{0}' already registered")]
    DuplicateId(String),
}

/// Default in-memory implementation of ProviderRegistry.
#[derive(Default)]
pub struct WaylandProviderRegistry {
    providers: HashMap<String, ProviderFactory>,
}

impl WaylandProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl ProviderRegistry for WaylandProviderRegistry {
    fn register(&mut self, id: &str, factory: ProviderFactory) -> Result<(), RegistryError> {
        if id.trim().is_empty() {
            return Err(RegistryError::EmptyId);
        }
        if self.providers.contains_key(id) {
            return Err(RegistryError::DuplicateId(id.to_string()));
        }
        self.providers.insert(id.to_string(), factory);
        Ok(())
    }

    fn get(&self, id: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(id).map(|f| f())
    }

    fn list_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.providers.keys().cloned().collect();
        v.sort();
        v
    }

    fn remove(&mut self, id: &str) -> bool {
        self.providers.remove(id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_types::llm::{LlmEvent, LlmRequest};

    struct DummyProvider;
    #[async_trait]
    impl LlmProvider for DummyProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, crate::ProviderError> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
    }

    fn dummy_factory() -> ProviderFactory {
        Arc::new(|| Arc::new(DummyProvider) as Arc<dyn LlmProvider>)
    }

    #[test]
    fn register_then_get() {
        let mut r = WaylandProviderRegistry::new();
        r.register("dummy", dummy_factory()).unwrap();
        assert!(r.get("dummy").is_some());
    }

    #[test]
    fn empty_id_rejected() {
        let mut r = WaylandProviderRegistry::new();
        assert_eq!(r.register("", dummy_factory()), Err(RegistryError::EmptyId));
        assert_eq!(
            r.register("   ", dummy_factory()),
            Err(RegistryError::EmptyId)
        );
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut r = WaylandProviderRegistry::new();
        r.register("dup", dummy_factory()).unwrap();
        assert_eq!(
            r.register("dup", dummy_factory()),
            Err(RegistryError::DuplicateId("dup".into()))
        );
    }

    #[test]
    fn list_ids_sorted() {
        let mut r = WaylandProviderRegistry::new();
        r.register("zeta", dummy_factory()).unwrap();
        r.register("alpha", dummy_factory()).unwrap();
        r.register("mu", dummy_factory()).unwrap();
        assert_eq!(r.list_ids(), vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn remove_returns_true_if_present() {
        let mut r = WaylandProviderRegistry::new();
        r.register("rem", dummy_factory()).unwrap();
        assert!(r.remove("rem"));
        assert!(!r.remove("rem"));
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let r = WaylandProviderRegistry::new();
        assert!(r.get("never_registered").is_none());
    }

    #[test]
    fn len_tracks_registrations() {
        let mut r = WaylandProviderRegistry::new();
        assert_eq!(r.len(), 0);
        r.register("a", dummy_factory()).unwrap();
        r.register("b", dummy_factory()).unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn factory_called_per_get() {
        // Counter shared across factory invocations
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = count.clone();
        let factory: ProviderFactory = Arc::new(move || {
            count_clone.fetch_add(1, Ordering::SeqCst);
            Arc::new(DummyProvider) as Arc<dyn LlmProvider>
        });
        let mut r = WaylandProviderRegistry::new();
        r.register("counted", factory).unwrap();
        let _ = r.get("counted");
        let _ = r.get("counted");
        let _ = r.get("counted");
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
