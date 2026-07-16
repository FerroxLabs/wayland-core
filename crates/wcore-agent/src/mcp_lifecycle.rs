//! Session-scoped MCP connection lifecycle and single-flight reservations.
//!
//! Hosts may submit the same live-add request concurrently while a configured
//! server is also connecting in the background.  This catalog is the one
//! authority that decides which caller may dial.  It intentionally does not
//! implement reconfiguration: a ready server keeps its current connection
//! until a future remove/restart contract exists.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use sha2::{Digest, Sha256};
use wcore_config::config::McpServerConfig;

/// Secret-safe identity of the configuration used for a connection attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct McpConfigIdentity([u8; 32]);

impl McpConfigIdentity {
    /// Identity used when adopting a boot connection whose source config is no
    /// longer available at the host handoff boundary.
    pub const UNKNOWN: Self = Self([0; 32]);

    /// Build a stable identity without retaining commands, headers, or secrets.
    /// Map entries are sorted before hashing so insertion order cannot create a
    /// second identity for the same effective configuration.
    pub fn for_server(config: &McpServerConfig) -> Self {
        #[derive(Serialize)]
        struct CanonicalConfig<'a> {
            transport: &'a wcore_config::config::TransportType,
            command: &'a Option<String>,
            args: &'a Option<Vec<String>>,
            env: Vec<(&'a String, &'a String)>,
            url: &'a Option<String>,
            headers: Vec<(&'a String, &'a String)>,
            deferred: &'a Option<bool>,
            allow_local: bool,
            only_for_assistant: &'a Option<Vec<String>>,
        }

        fn sorted_entries(map: &Option<HashMap<String, String>>) -> Vec<(&String, &String)> {
            let mut entries: Vec<_> = map
                .as_ref()
                .into_iter()
                .flat_map(|map| map.iter())
                .collect();
            entries.sort_by_key(|(key, _)| *key);
            entries
        }

        let canonical = CanonicalConfig {
            transport: &config.transport,
            command: &config.command,
            args: &config.args,
            env: sorted_entries(&config.env),
            url: &config.url,
            headers: sorted_entries(&config.headers),
            deferred: &config.deferred,
            allow_local: config.allow_local,
            only_for_assistant: &config.only_for_assistant,
        };
        let bytes = serde_json::to_vec(&canonical)
            .expect("MCP configuration contains only infallibly serializable values");
        Self(Sha256::digest(bytes).into())
    }
}

/// Current state of one server name in a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpLifecycleState {
    Connecting,
    Ready,
    Failed { reason: String },
    Stopping,
}

/// Immutable catalog observation returned to non-owning callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpLifecycleSnapshot {
    pub name: String,
    pub config_identity: McpConfigIdentity,
    pub generation: u64,
    pub state: McpLifecycleState,
}

#[derive(Default)]
struct CatalogInner {
    entries: HashMap<String, McpLifecycleSnapshot>,
}

/// One session's MCP lifecycle authority.
#[derive(Clone, Default)]
pub struct McpLifecycleCatalog {
    inner: Arc<Mutex<CatalogInner>>,
}

/// Result of attempting to reserve a server name.
pub enum McpReservationOutcome {
    /// This caller alone may dial and register the server.
    Acquired(McpConnectionReservation),
    /// Another attempt is active, or the existing connection must be kept.
    Existing(McpLifecycleSnapshot),
}

/// RAII ownership of one connection generation.
///
/// Dropping an uncompleted reservation changes only its own still-current
/// `Connecting` generation to `Failed`, making the name retryable.  A stale
/// owner can never overwrite a newer generation.
pub struct McpConnectionReservation {
    catalog: McpLifecycleCatalog,
    name: String,
    config_identity: McpConfigIdentity,
    generation: u64,
    armed: bool,
}

impl McpLifecycleCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, CatalogInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Atomically acquire the right to connect `name`.
    ///
    /// `Failed` is retryable and advances the generation. `Ready`,
    /// `Connecting`, and `Stopping` preserve the current connection/attempt,
    /// including when the submitted config identity differs.
    pub fn reserve(
        &self,
        name: impl Into<String>,
        config_identity: McpConfigIdentity,
    ) -> McpReservationOutcome {
        let name = name.into();
        let mut inner = self.lock();
        if let Some(current) = inner.entries.get(&name)
            && !matches!(current.state, McpLifecycleState::Failed { .. })
        {
            return McpReservationOutcome::Existing(current.clone());
        }

        let generation = inner
            .entries
            .get(&name)
            .map_or(1, |entry| entry.generation.saturating_add(1));
        inner.entries.insert(
            name.clone(),
            McpLifecycleSnapshot {
                name: name.clone(),
                config_identity,
                generation,
                state: McpLifecycleState::Connecting,
            },
        );
        McpReservationOutcome::Acquired(McpConnectionReservation {
            catalog: self.clone(),
            name,
            config_identity,
            generation,
            armed: true,
        })
    }

    /// Adopt an already-ready boot connection before live requests begin.
    pub fn seed_ready(&self, name: impl Into<String>, config_identity: McpConfigIdentity) {
        let name = name.into();
        let mut inner = self.lock();
        inner
            .entries
            .entry(name.clone())
            .or_insert(McpLifecycleSnapshot {
                name,
                config_identity,
                generation: 1,
                state: McpLifecycleState::Ready,
            });
    }

    /// Expose the current state for diagnostics and tests.
    pub fn snapshot(&self, name: &str) -> Option<McpLifecycleSnapshot> {
        self.lock().entries.get(name).cloned()
    }

    /// Record the beginning of a future remove/restart path without providing
    /// that protocol surface in this change.
    pub fn mark_stopping(&self, name: &str) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        entry.state = McpLifecycleState::Stopping;
        true
    }

    fn transition_if_current(
        &self,
        name: &str,
        config_identity: McpConfigIdentity,
        generation: u64,
        state: McpLifecycleState,
    ) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        if entry.generation != generation
            || entry.config_identity != config_identity
            || !matches!(entry.state, McpLifecycleState::Connecting)
        {
            return false;
        }
        entry.state = state;
        true
    }
}

impl McpConnectionReservation {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn complete_ready(mut self) -> bool {
        let changed = self.catalog.transition_if_current(
            &self.name,
            self.config_identity,
            self.generation,
            McpLifecycleState::Ready,
        );
        self.armed = false;
        changed
    }

    pub fn complete_failed(mut self, reason: impl Into<String>) -> bool {
        let changed = self.catalog.transition_if_current(
            &self.name,
            self.config_identity,
            self.generation,
            McpLifecycleState::Failed {
                reason: reason.into(),
            },
        );
        self.armed = false;
        changed
    }
}

impl Drop for McpConnectionReservation {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.catalog.transition_if_current(
                &self.name,
                self.config_identity,
                self.generation,
                McpLifecycleState::Failed {
                    reason: "connection attempt abandoned".to_string(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use wcore_config::config::TransportType;

    fn config() -> McpServerConfig {
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("server".to_string()),
            args: Some(vec!["--stdio".to_string()]),
            env: None,
            url: None,
            headers: None,
            deferred: Some(true),
            allow_local: false,
            only_for_assistant: None,
        }
    }

    #[test]
    fn concurrent_same_name_has_exactly_one_owner() {
        let catalog = McpLifecycleCatalog::new();
        let identity = McpConfigIdentity::for_server(&config());
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let catalog = catalog.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                match catalog.reserve("shared", identity) {
                    McpReservationOutcome::Acquired(reservation) => {
                        barrier.wait();
                        reservation.complete_ready();
                        true
                    }
                    McpReservationOutcome::Existing(_) => {
                        barrier.wait();
                        false
                    }
                }
            }));
        }
        barrier.wait();
        barrier.wait();
        let owners = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(owners, 1);
        assert_eq!(
            catalog.snapshot("shared").unwrap().state,
            McpLifecycleState::Ready
        );
    }

    #[test]
    fn abandoned_and_explicit_failure_are_retryable() {
        let catalog = McpLifecycleCatalog::new();
        let identity = McpConfigIdentity::for_server(&config());
        let first = match catalog.reserve("retry", identity) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("first reservation must win"),
        };
        assert_eq!(first.generation(), 1);
        drop(first);
        assert!(matches!(
            catalog.snapshot("retry").unwrap().state,
            McpLifecycleState::Failed { .. }
        ));

        let second = match catalog.reserve("retry", identity) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("retry must win"),
        };
        assert_eq!(second.generation(), 2);
        second.complete_failed("dial failed");
        let third = match catalog.reserve("retry", identity) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("failed retry must release"),
        };
        assert_eq!(third.generation(), 3);
        third.complete_ready();
    }

    #[test]
    fn ready_readd_keeps_existing_generation_even_when_config_changes() {
        let catalog = McpLifecycleCatalog::new();
        let first_identity = McpConfigIdentity::for_server(&config());
        let mut changed = config();
        changed.args = Some(vec!["--different".to_string()]);
        let changed_identity = McpConfigIdentity::for_server(&changed);
        assert_ne!(first_identity, changed_identity);

        let first = match catalog.reserve("stable", first_identity) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("first reservation must win"),
        };
        first.complete_ready();
        let existing = match catalog.reserve("stable", changed_identity) {
            McpReservationOutcome::Existing(snapshot) => snapshot,
            McpReservationOutcome::Acquired(_) => panic!("ready server must not reconfigure"),
        };
        assert_eq!(existing.generation, 1);
        assert_eq!(existing.config_identity, first_identity);
        assert_eq!(existing.state, McpLifecycleState::Ready);
    }

    #[test]
    fn config_identity_is_independent_of_map_insertion_order() {
        let mut left = config();
        left.env = Some(HashMap::from([
            ("B".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]));
        let mut right = config();
        right.env = Some(HashMap::from([
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ]));
        assert_eq!(
            McpConfigIdentity::for_server(&left),
            McpConfigIdentity::for_server(&right)
        );
    }

    #[test]
    fn stopping_blocks_new_reservations() {
        let catalog = McpLifecycleCatalog::new();
        catalog.seed_ready("server", McpConfigIdentity::UNKNOWN);
        assert!(catalog.mark_stopping("server"));
        let outcome = catalog.reserve("server", McpConfigIdentity::UNKNOWN);
        assert!(matches!(
            outcome,
            McpReservationOutcome::Existing(McpLifecycleSnapshot {
                state: McpLifecycleState::Stopping,
                ..
            })
        ));
    }
}
