//! Wave B4 — in-RAM mock for `HonchoClient`.
//!
//! Deterministic, no network, no I/O. Matches the live surface so the
//! same `HonchoClient` API works in tests and prod.

use crate::api::{DialecticInference, UserProfile};
use crate::error::Result;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct MockClient {
    // Mutex over the profile map. Poisoning is unreachable in practice
    // (no panics inside the critical section); matches the engine-wide
    // `lock().unwrap()` convention (see e.g. wcore-skills/src/mcp.rs).
    profiles: Mutex<HashMap<String, UserProfile>>,
    // v0.8.1 U3 — per-user dialectic inferences. Empty by default so a
    // mock client behaves identically to a fresh Honcho deployment with
    // no observations folded in yet.
    representations: Mutex<HashMap<String, Vec<DialecticInference>>>,
}

impl MockClient {
    pub async fn learn_preference(&self, user_id: &str, key: &str, value: &str) -> Result<()> {
        let mut p = self.profiles.lock().unwrap();
        let entry = p.entry(user_id.to_string()).or_insert_with(|| UserProfile {
            user_id: user_id.to_string(),
            ..Default::default()
        });
        entry.preferences.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub async fn recall_user(&self, user_id: &str) -> Result<UserProfile> {
        let p = self.profiles.lock().unwrap();
        Ok(p.get(user_id).cloned().unwrap_or_else(|| UserProfile {
            user_id: user_id.to_string(),
            ..Default::default()
        }))
    }

    /// v0.8.1 U3 — mirror of `LiveClient::representations`. Returns
    /// whatever was seeded via [`Self::seed_representations`]; unknown
    /// users yield an empty Vec (matches the live 404 semantics).
    pub async fn representations(&self, user_id: &str) -> Result<Vec<DialecticInference>> {
        let r = self.representations.lock().unwrap();
        Ok(r.get(user_id).cloned().unwrap_or_default())
    }

    /// Test-only seed for the dialectic store. Used by adapter +
    /// engine tests to assert the brief() merge path without touching
    /// the network.
    pub fn seed_representations(&self, user_id: &str, inferences: Vec<DialecticInference>) {
        let mut r = self.representations.lock().unwrap();
        r.insert(user_id.to_string(), inferences);
    }
}
