//! Session-scoped MCP connection lifecycle and single-flight reservations.
//!
//! Hosts may submit the same live-add request concurrently while a configured
//! server is also connecting in the background.  This catalog is the one
//! authority that decides which caller may dial.  It intentionally does not
//! implement reconfiguration: a ready server keeps its current connection
//! until an explicit remove completes and releases the name for a new generation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use sha2::{Digest, Sha256};
use wcore_config::config::McpServerConfig;

pub const MAX_MCP_LIFECYCLE_NAMES: usize = 4096;
const MAX_MCP_LIFECYCLE_NAME_LEN: usize = 256;
const MAX_MCP_LIFECYCLE_REASON_LEN: usize = 512;

fn bounded_reason(reason: impl Into<String>) -> String {
    let mut reason = reason.into();
    if reason.len() <= MAX_MCP_LIFECYCLE_REASON_LEN {
        return reason;
    }
    let mut boundary = MAX_MCP_LIFECYCLE_REASON_LEN;
    while !reason.is_char_boundary(boundary) {
        boundary -= 1;
    }
    reason.truncate(boundary);
    reason
}

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
    Failed {
        reason: String,
    },
    Stopping,
    /// Transport/process cleanup could not be proven. This generation keeps
    /// the name reserved until a later remove retries and verifies cleanup.
    CleanupUnverified {
        reason: String,
    },
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
    /// Per-name high-water mark retained after removal so a new connection
    /// can never reuse a stale generation identity.
    generations: HashMap<String, u64>,
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
    /// The session already tracks the maximum number of distinct names.
    CapacityExceeded,
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
    /// `Connecting`, `Stopping`, and `CleanupUnverified` preserve the current
    /// connection/attempt, including when the submitted identity differs.
    pub fn reserve(
        &self,
        name: impl Into<String>,
        config_identity: McpConfigIdentity,
    ) -> McpReservationOutcome {
        let name = name.into();
        let mut inner = self.lock();
        if name.trim().is_empty() || name.len() > MAX_MCP_LIFECYCLE_NAME_LEN {
            return McpReservationOutcome::CapacityExceeded;
        }
        if let Some(current) = inner.entries.get(&name)
            && !matches!(current.state, McpLifecycleState::Failed { .. })
        {
            return McpReservationOutcome::Existing(current.clone());
        }
        if !inner.generations.contains_key(&name)
            && inner.generations.len() >= MAX_MCP_LIFECYCLE_NAMES
        {
            return McpReservationOutcome::CapacityExceeded;
        }

        let generation = inner
            .generations
            .get(&name)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        inner.generations.insert(name.clone(), generation);
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
    pub fn seed_ready(&self, name: impl Into<String>, config_identity: McpConfigIdentity) -> bool {
        let name = name.into();
        let mut inner = self.lock();
        if name.trim().is_empty()
            || name.len() > MAX_MCP_LIFECYCLE_NAME_LEN
            || (!inner.generations.contains_key(&name)
                && inner.generations.len() >= MAX_MCP_LIFECYCLE_NAMES)
        {
            return false;
        }
        if inner.entries.contains_key(&name) {
            return true;
        }
        let generation = inner
            .generations
            .get(&name)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        inner.generations.insert(name.clone(), generation);
        inner.entries.insert(
            name.clone(),
            McpLifecycleSnapshot {
                name,
                config_identity,
                generation,
                state: McpLifecycleState::Ready,
            },
        );
        true
    }

    /// Expose the current state for diagnostics and tests.
    pub fn snapshot(&self, name: &str) -> Option<McpLifecycleSnapshot> {
        self.lock().entries.get(name).cloned()
    }

    /// Record the beginning of an explicit remove/restart path.
    pub fn mark_stopping(&self, name: &str) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        if !matches!(
            entry.state,
            McpLifecycleState::Ready | McpLifecycleState::CleanupUnverified { .. }
        ) {
            return false;
        }
        entry.state = McpLifecycleState::Stopping;
        true
    }

    /// Cancel one exact in-flight connection generation. A stale remove can
    /// never stop a newer retry, and ordinary stop callers cannot silently
    /// rewrite `Connecting` through [`Self::mark_stopping`].
    pub fn cancel_connecting(&self, name: &str, generation: u64) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        if entry.generation != generation || !matches!(entry.state, McpLifecycleState::Connecting) {
            return false;
        }
        entry.state = McpLifecycleState::Stopping;
        true
    }

    /// Release a name only after its current entry reached `Stopping`.
    ///
    /// Repeating completion is harmless. A caller cannot accidentally erase a
    /// newer connecting/ready generation because those states are rejected.
    pub fn complete_stopping(&self, name: &str) -> bool {
        let Some(generation) = self.snapshot(name).map(|entry| entry.generation) else {
            return false;
        };
        self.complete_stopping_generation(name, generation)
    }

    /// Complete cleanup only for the generation the caller actually stopped.
    pub fn complete_stopping_generation(&self, name: &str, generation: u64) -> bool {
        let mut inner = self.lock();
        if !inner.entries.get(name).is_some_and(|entry| {
            entry.generation == generation && matches!(entry.state, McpLifecycleState::Stopping)
        }) {
            return false;
        }
        inner.entries.remove(name);
        true
    }

    /// Roll back a remove that could not acquire mutation authority. Only the
    /// current stopping entry can return to ready; absent/newer generations
    /// are never synthesized or overwritten.
    pub fn cancel_stopping(&self, name: &str) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        if !matches!(entry.state, McpLifecycleState::Stopping) {
            return false;
        }
        entry.state = McpLifecycleState::Ready;
        true
    }

    pub fn mark_cleanup_unverified(&self, name: &str, reason: impl Into<String>) -> bool {
        let Some(generation) = self.snapshot(name).map(|entry| entry.generation) else {
            return false;
        };
        self.mark_cleanup_unverified_generation(name, generation, reason)
    }

    /// Quarantine only the stopped generation whose cleanup actually failed.
    pub fn mark_cleanup_unverified_generation(
        &self,
        name: &str,
        generation: u64,
        reason: impl Into<String>,
    ) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(name) else {
            return false;
        };
        if entry.generation != generation || !matches!(entry.state, McpLifecycleState::Stopping) {
            return false;
        }
        entry.state = McpLifecycleState::CleanupUnverified {
            reason: bounded_reason(reason),
        };
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
                reason: bounded_reason(reason),
            },
        );
        self.armed = false;
        changed
    }

    /// Complete a generation-bound cancellation before any transport exists.
    /// Once a process/socket has been created, callers must prove its cleanup
    /// and use the catalog's generation-bound stopping APIs instead.
    pub fn complete_cancelled_before_transport(mut self) -> bool {
        let changed = self
            .catalog
            .complete_stopping_generation(&self.name, self.generation);
        self.armed = false;
        changed
    }
}

impl Drop for McpConnectionReservation {
    fn drop(&mut self) {
        if self.armed {
            let failed = self.catalog.transition_if_current(
                &self.name,
                self.config_identity,
                self.generation,
                McpLifecycleState::Failed {
                    reason: "connection attempt abandoned".to_string(),
                },
            );
            if !failed {
                let _ = self.catalog.mark_cleanup_unverified_generation(
                    &self.name,
                    self.generation,
                    "connection task abandoned after cancellation; cleanup unverified",
                );
            }
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
                    McpReservationOutcome::CapacityExceeded => {
                        panic!("concurrency fixture must stay below capacity")
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
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
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
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
        };
        assert_eq!(second.generation(), 2);
        second.complete_failed("dial failed");
        let third = match catalog.reserve("retry", identity) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("failed retry must release"),
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
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
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
        };
        first.complete_ready();
        let existing = match catalog.reserve("stable", changed_identity) {
            McpReservationOutcome::Existing(snapshot) => snapshot,
            McpReservationOutcome::Acquired(_) => panic!("ready server must not reconfigure"),
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
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

    #[test]
    fn connecting_cancellation_is_generation_bound_and_blocks_late_ready() {
        let catalog = McpLifecycleCatalog::new();
        let first = match catalog.reserve("server", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            _ => panic!("first reservation must acquire"),
        };
        assert!(!catalog.mark_stopping("server"));
        assert!(catalog.cancel_connecting("server", first.generation()));
        assert!(
            !first.complete_ready(),
            "cancelled owner must not publish Ready"
        );
        assert!(catalog.complete_stopping_generation("server", 1));

        let second = match catalog.reserve("server", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            _ => panic!("verified cancellation must allow a new generation"),
        };
        assert_eq!(second.generation(), 2);
        assert!(!catalog.cancel_connecting("server", 1));
        second.complete_ready();
        assert_eq!(
            catalog.snapshot("server").unwrap().state,
            McpLifecycleState::Ready
        );
    }

    #[test]
    fn abandoned_cancelled_connection_is_quarantined() {
        let catalog = McpLifecycleCatalog::new();
        let reservation = match catalog.reserve("server", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            _ => panic!("reservation must acquire"),
        };
        assert!(catalog.cancel_connecting("server", reservation.generation()));
        drop(reservation);
        assert!(matches!(
            catalog.snapshot("server").unwrap().state,
            McpLifecycleState::CleanupUnverified { .. }
        ));
    }

    #[test]
    fn cleanup_unverified_blocks_add_until_verified_retry_completes() {
        let catalog = McpLifecycleCatalog::new();
        catalog.seed_ready("server", McpConfigIdentity::UNKNOWN);
        assert!(catalog.mark_stopping("server"));
        assert!(catalog.mark_cleanup_unverified("server", "injected close failure"));
        let blocked = catalog.reserve("server", McpConfigIdentity::UNKNOWN);
        assert!(matches!(
            blocked,
            McpReservationOutcome::Existing(McpLifecycleSnapshot {
                generation: 1,
                state: McpLifecycleState::CleanupUnverified { .. },
                ..
            })
        ));

        // A later remove owns the cleanup retry. Only verified completion
        // releases the name for the next generation.
        assert!(catalog.mark_stopping("server"));
        assert!(catalog.complete_stopping("server"));
        let next = match catalog.reserve("server", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("verified cleanup must release name"),
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
        };
        assert_eq!(next.generation(), 2);
        next.complete_ready();
    }

    #[test]
    fn completed_stop_is_idempotent_and_allows_readd() {
        let catalog = McpLifecycleCatalog::new();
        catalog.seed_ready("server", McpConfigIdentity::UNKNOWN);
        assert!(catalog.mark_stopping("server"));
        assert!(catalog.complete_stopping("server"));
        assert!(!catalog.complete_stopping("server"));
        assert!(catalog.snapshot("server").is_none());

        let reservation = match catalog.reserve("server", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            McpReservationOutcome::Existing(_) => panic!("completed stop must release name"),
            McpReservationOutcome::CapacityExceeded => panic!("fixture below capacity"),
        };
        assert_eq!(reservation.generation(), 2);
        reservation.complete_ready();
        assert_eq!(
            catalog.snapshot("server").unwrap().state,
            McpLifecycleState::Ready
        );
    }

    #[test]
    fn seed_ready_advances_retained_generation_high_water() {
        let catalog = McpLifecycleCatalog::new();
        assert!(catalog.seed_ready("server", McpConfigIdentity::UNKNOWN));
        assert_eq!(catalog.snapshot("server").unwrap().generation, 1);
        assert!(catalog.mark_stopping("server"));
        assert!(catalog.complete_stopping("server"));
        assert!(catalog.seed_ready("server", McpConfigIdentity::UNKNOWN));
        assert_eq!(catalog.snapshot("server").unwrap().generation, 2);
    }

    #[test]
    fn stored_failure_reasons_are_byte_bounded() {
        let catalog = McpLifecycleCatalog::new();
        let first = match catalog.reserve("dial", McpConfigIdentity::UNKNOWN) {
            McpReservationOutcome::Acquired(reservation) => reservation,
            _ => panic!("first reservation must acquire"),
        };
        first.complete_failed("é".repeat(MAX_MCP_LIFECYCLE_REASON_LEN));
        let McpLifecycleState::Failed { reason } = catalog.snapshot("dial").unwrap().state else {
            panic!("dial must be failed");
        };
        assert!(reason.len() <= MAX_MCP_LIFECYCLE_REASON_LEN);
        assert!(reason.is_char_boundary(reason.len()));

        assert!(catalog.seed_ready("cleanup", McpConfigIdentity::UNKNOWN));
        assert!(catalog.mark_stopping("cleanup"));
        assert!(
            catalog.mark_cleanup_unverified("cleanup", "é".repeat(MAX_MCP_LIFECYCLE_REASON_LEN))
        );
        let McpLifecycleState::CleanupUnverified { reason } =
            catalog.snapshot("cleanup").unwrap().state
        else {
            panic!("cleanup must remain unverified");
        };
        assert!(reason.len() <= MAX_MCP_LIFECYCLE_REASON_LEN);
        assert!(reason.is_char_boundary(reason.len()));
    }

    #[test]
    fn lifecycle_catalog_rejects_unbounded_names_and_cardinality() {
        let catalog = McpLifecycleCatalog::new();
        assert!(matches!(
            catalog.reserve(
                "x".repeat(MAX_MCP_LIFECYCLE_NAME_LEN + 1),
                McpConfigIdentity::UNKNOWN
            ),
            McpReservationOutcome::CapacityExceeded
        ));
        assert!(
            catalog
                .snapshot(&"x".repeat(MAX_MCP_LIFECYCLE_NAME_LEN + 1))
                .is_none()
        );

        for index in 0..MAX_MCP_LIFECYCLE_NAMES {
            let reservation =
                match catalog.reserve(format!("bounded-{index}"), McpConfigIdentity::UNKNOWN) {
                    McpReservationOutcome::Acquired(reservation) => reservation,
                    McpReservationOutcome::Existing(_) => panic!("unique name must acquire"),
                    McpReservationOutcome::CapacityExceeded => panic!("capacity reached too early"),
                };
            reservation.complete_ready();
        }
        assert!(matches!(
            catalog.reserve("one-too-many", McpConfigIdentity::UNKNOWN),
            McpReservationOutcome::CapacityExceeded
        ));
        assert!(catalog.snapshot("one-too-many").is_none());
    }

    #[test]
    fn stop_completion_cannot_erase_a_non_stopping_entry() {
        let catalog = McpLifecycleCatalog::new();
        catalog.seed_ready("server", McpConfigIdentity::UNKNOWN);
        assert!(!catalog.complete_stopping("server"));
        assert_eq!(
            catalog.snapshot("server").unwrap().state,
            McpLifecycleState::Ready
        );
    }

    #[test]
    fn cancelled_stop_restores_ready_without_changing_generation() {
        let catalog = McpLifecycleCatalog::new();
        catalog.seed_ready("server", McpConfigIdentity::UNKNOWN);
        assert!(catalog.mark_stopping("server"));
        assert!(catalog.cancel_stopping("server"));
        let snapshot = catalog.snapshot("server").unwrap();
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.state, McpLifecycleState::Ready);
        assert!(!catalog.cancel_stopping("server"));
    }
}
