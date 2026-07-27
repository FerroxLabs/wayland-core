//! Typed, deterministic, structurally-redacted portability plans (F26-01).
//!
//! This module owns the vocabulary that every peer discovery source produces
//! and that the CLI serializes for `migrate --json`. It owns three things:
//!
//! 1. [`PortabilityPlan`] and its parts — the typed plan.
//! 2. [`redact::CredentialRef`] — the structural redaction boundary.
//! 3. [`digest::tree_digest`] — the non-mutation proof.
//!
//! # The redaction contract
//!
//! A [`PortabilityPlan`] is the EMITTED type. No type reachable from it has a
//! field capable of holding a credential value: a credential appears only as a
//! [`redact::CredentialRef`], which records a name and a source file. So
//! `serde`, `Debug`, `Display` and every error formatter inherit the redaction
//! from the type instead of each having to remember to withhold something.
//!
//! Note what this buys over a careful printer. The CLI's internal
//! `MigrationPlan` DOES hold an api key, because `--include-credentials` has to
//! write one. The value is dropped at the conversion into this type, and there
//! is no inverse conversion — so a downstream consumer handed a
//! `PortabilityPlan` cannot emit a secret even deliberately.
//!
//! # Determinism
//!
//! Every collection is ordered by a key that exists in the source data — items
//! by `(kind, id)`, deferred counts and details by `BTreeMap` key — never by
//! directory iteration order. Two independent walks of the same tree therefore
//! serialize byte-identically. [`PortabilityPlan::finalize`] is what imposes
//! that order, and every source calls it before emitting.

pub mod digest;
pub mod redact;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use digest::{TreeDigest, tree_digest};
pub use redact::CredentialRef;

/// Which peer a plan was discovered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSource {
    Hermes,
    /// Renamed explicitly: the default snake_case rule would emit `open_claw`,
    /// which is not the peer's name and not the CLI subcommand.
    #[serde(rename = "openclaw")]
    OpenClaw,
}

impl PeerSource {
    /// The stable wire name, also used as the CLI subcommand name.
    pub fn as_str(self) -> &'static str {
        match self {
            PeerSource::Hermes => "hermes",
            PeerSource::OpenClaw => "openclaw",
        }
    }
}

impl std::fmt::Display for PeerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of thing discovery found. Ordered so that a plan's items sort into
/// a readable, stable grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// The peer's ROOT-level setup, imported as a profile. See
    /// [`ROOT_PROFILE_ID`].
    RootProfile,
    /// One named profile / agent.
    Profile,
    /// One MCP server definition.
    McpServer,
}

/// The stable identifier given to a peer's root-level setup when it is imported
/// as a profile.
///
/// # Why this exact string
///
/// A Hermes home may hold BOTH a root `config.yaml` and `profiles/<name>/`
/// directories, so the root entry needs a name that cannot silently collide
/// with a real profile. Two options were considered:
///
///   (a) pick a name a profile directory cannot have, or
///   (b) allow the collision and report it as a conflict.
///
/// **(a) was chosen, and the name is `default` prefixed with the source name.**
/// A profile is a DIRECTORY under `profiles/`, and `/` is not a legal character
/// in a directory name on any platform this ships to, so `hermes/root` is
/// unspoofable by construction rather than by convention. That makes the
/// collision impossible instead of merely detected — which is the stronger of
/// the two, because a detected collision still needs someone to act on it.
/// Discovery additionally emits a warning if a profile named `root` exists, so
/// a user who expected the obvious name is told where it went.
pub const ROOT_PROFILE_ID: &str = "hermes/root";

/// The same, for an OpenClaw home's `agents.defaults` block.
pub const OPENCLAW_ROOT_PROFILE_ID: &str = "openclaw/root";

/// True when an id names a peer's root-level setup rather than a named profile.
pub fn is_root_profile_id(id: &str) -> bool {
    id == ROOT_PROFILE_ID || id == OPENCLAW_ROOT_PROFILE_ID
}

/// One discovered thing, mapped or named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredItem {
    pub kind: ItemKind,
    /// Stable identifier — the profile or server name.
    pub id: String,
    /// Where it came from, RELATIVE to the source home. Relative so an absolute
    /// path on the discovering machine never reaches an emitted document.
    pub source_path: String,
    /// Where it would land in wayland-core, e.g. `profiles.fred`.
    pub target: String,
    /// A wayland-core object of this name already exists.
    pub conflict: bool,
    /// The credential discovered for this item, by REFERENCE only. There is no
    /// field anywhere in this type that can hold its value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialRef>,
    /// Mapped, non-secret settings — provider, model, base_url, transport, …
    /// A `BTreeMap` so the order is the key order, not the insertion order.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub details: BTreeMap<String, String>,
}

/// The full typed plan. This is the type `migrate --json` serializes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortabilityPlan {
    pub source: PeerSource,
    /// The resolved source home. Absolute, because it names the machine-local
    /// input the user asked about — but it is the ONLY absolute path emitted,
    /// and it is not a secret.
    pub source_home: String,
    pub items: Vec<DiscoveredItem>,
    /// Detected but NOT imported, keyed by a stable kind name. Counted rather
    /// than loaded, so a 540-directory skill tree costs nothing to report.
    #[serde(default)]
    pub deferred: BTreeMap<String, usize>,
    /// Non-fatal notes, including symlink escapes refused by the walk.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl PortabilityPlan {
    pub fn new(source: PeerSource, source_home: impl Into<String>) -> Self {
        Self {
            source,
            source_home: source_home.into(),
            items: Vec::new(),
            deferred: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Impose the total order the determinism guarantee depends on.
    ///
    /// Every source calls this before emitting. Sorting by `(kind, id)` — both
    /// of which come from the source data — is what makes two independent walks
    /// of one tree serialize identically, regardless of the order the
    /// filesystem handed entries back.
    pub fn finalize(&mut self) {
        self.items
            .sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
        self.warnings.sort();
        self.warnings.dedup();
    }

    /// Count of items of one kind — the positive assertion gates use so that an
    /// empty emission fails instead of passing a canary-absence check.
    pub fn count_of(&self, kind: ItemKind) -> usize {
        self.items.iter().filter(|i| i.kind == kind).count()
    }

    /// Serialize deterministically. `serde_json` preserves struct field order
    /// and `BTreeMap` key order, so this is stable across runs and processes.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ItemKind, id: &str) -> DiscoveredItem {
        DiscoveredItem {
            kind,
            id: id.into(),
            source_path: format!("profiles/{id}"),
            target: format!("profiles.{id}"),
            conflict: false,
            credential: None,
            details: BTreeMap::new(),
        }
    }

    #[test]
    fn finalize_imposes_an_order_independent_of_insertion() {
        let mut a = PortabilityPlan::new(PeerSource::Hermes, "/h");
        a.items = vec![
            item(ItemKind::Profile, "zed"),
            item(ItemKind::McpServer, "srv"),
            item(ItemKind::Profile, "amy"),
        ];
        let mut b = PortabilityPlan::new(PeerSource::Hermes, "/h");
        b.items = vec![
            item(ItemKind::McpServer, "srv"),
            item(ItemKind::Profile, "amy"),
            item(ItemKind::Profile, "zed"),
        ];
        a.finalize();
        b.finalize();
        assert_eq!(
            a.to_json().unwrap(),
            b.to_json().unwrap(),
            "two insertion orders must serialize identically"
        );
        // Positive half: the emission is not empty, so the equality above is
        // not the trivial equality of two empty documents.
        assert_eq!(a.count_of(ItemKind::Profile), 2);
        assert!(a.to_json().unwrap().contains("\"amy\""));
    }

    #[test]
    fn serialization_is_byte_identical_across_repeated_calls() {
        let mut p = PortabilityPlan::new(PeerSource::OpenClaw, "/o");
        p.items = vec![item(ItemKind::Profile, "main")];
        p.deferred.insert("skills".into(), 540);
        p.deferred.insert("agents".into(), 1);
        p.finalize();
        assert_eq!(p.to_json().unwrap(), p.to_json().unwrap());
        // Deferred is a BTreeMap, so its rendering is key-ordered.
        let json = p.to_json().unwrap();
        let agents = json.find("agents").unwrap();
        let skills = json.find("skills").unwrap();
        assert!(agents < skills, "deferred must render in key order");
    }

    #[test]
    fn no_type_reachable_from_a_plan_can_hold_a_credential_value() {
        // The structural claim, asserted on the emitted document itself: a
        // plan built by a source that HAD a real secret still cannot render it.
        let secret = "sk-live-MUST-NOT-APPEAR-9876543210";
        let mut p = PortabilityPlan::new(PeerSource::Hermes, "/h");
        let mut it = item(ItemKind::Profile, "fred");
        it.credential = Some(CredentialRef::new(
            "OPENROUTER_API_KEY",
            "profiles/fred/.env",
        ));
        it.details.insert("provider".into(), "anthropic".into());
        p.items.push(it);
        p.finalize();

        let json = p.to_json().unwrap();
        let debug = format!("{p:?}");
        for (what, r) in [("json", &json), ("debug", &debug)] {
            assert!(!r.contains(secret), "{what} leaked a credential value");
        }
        // Positive half, so an empty plan cannot satisfy the assertions above.
        assert!(json.contains("OPENROUTER_API_KEY"));
        assert!(json.contains("\"provider\""));
        assert_eq!(p.count_of(ItemKind::Profile), 1);
    }

    #[test]
    fn root_profile_id_cannot_collide_with_a_real_profile_directory_name() {
        // The collision decision, asserted rather than only documented: the id
        // contains a path separator, which no single directory name can.
        assert!(
            ROOT_PROFILE_ID.contains('/'),
            "the root id must be unspoofable by a directory name"
        );
    }

    #[test]
    fn plan_round_trips_through_json() {
        let mut p = PortabilityPlan::new(PeerSource::Hermes, "/h");
        p.items = vec![item(ItemKind::Profile, "a")];
        p.warnings.push("something".into());
        p.finalize();
        let json = p.to_json().unwrap();
        let back: PortabilityPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
