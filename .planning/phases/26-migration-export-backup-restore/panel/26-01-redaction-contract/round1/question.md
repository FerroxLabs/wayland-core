# PANEL QUESTION — is the redaction STRUCTURAL or COSMETIC?

You are one of four panel members. Answer from the evidence below only.

## The question

Phase 26 requires that `wayland-core migrate <peer> --json` emit a plan in which a
discovered credential is represented by its SOURCE REFERENCE only, such that NO downstream
printer, log line, error message or serializer can emit the credential VALUE.

Decide which of these three holds:

- `contract-holds`    — a secret value is UNREPRESENTABLE in the emitted type; representation
                        is by source reference alone.
- `contract-cosmetic` — values are withheld by the PRINTER rather than by the type, so a
                        downstream serializer, Debug rendering or error formatter could still
                        emit one.
- `contract-leaks`    — a value is representable AND observably present in an emitted document.

## Why the obvious measurement is not sufficient (this is why you are here)

Running the real binary against real homes and finding zero secrets can REFUTE
`contract-holds`, but can never confirm it: a purely cosmetic redaction that merely declines
to print produces exactly the same zero. So a multi-emitter probe was built. Judge whether it
actually closes the gap, and name the specific field, variant or accessor that decides it.

## EVIDENCE 1 — the redaction type (crates/wcore-config/src/portability/redact.rs)

//! The structural redaction boundary for portability plans (F26-01).
//!
//! # Why this type exists
//!
//! A secret that is redacted when PRINTED but still present in the typed value
//! has leaked to every consumer that serializes it — and `migrate --json`
//! creates exactly such a consumer. Withholding the value in one printer is
//! cosmetic: `Debug`, `serde`, a log line and an error formatter each get their
//! own chance to emit it, and every one of them has to remember.
//!
//! So the value is made **unrepresentable** instead. [`CredentialRef`] records
//! only where a credential came from — the variable or key name, and the file
//! relative to the source home. There is deliberately no field, no variant and
//! no accessor capable of carrying the secret itself, so `Debug`, `Display`,
//! `serde` and every error path inherit the redaction from the TYPE rather than
//! each having to implement it.
//!
//! This is a boundary type, not a container: a caller that holds a real secret
//! (the Hermes mapper does, when `--include-credentials` is passed) converts to
//! a `CredentialRef` and the value is dropped at the conversion. There is no
//! inverse — you cannot go from a `CredentialRef` back to a value.

use serde::{Deserialize, Serialize};

/// A discovered credential, represented by its SOURCE REFERENCE only.
///
/// # Invariant
///
/// This struct has exactly two fields, both of which name a LOCATION. Adding a
/// field that can hold a credential value — or a `From<…>` that stores one —
/// would silently convert every consumer of a portability plan into a secret
/// sink. The multi-emitter probe in `crates/wcore-cli/tests/migrate_typed_dryrun.rs`
/// exists to catch exactly that regression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CredentialRef {
    /// The environment variable or configuration key the credential was found
    /// under — e.g. `DEEPSEEK_API_KEY`, or `gateway.auth.token`.
    pub name: String,
    /// The file it was found in, relative to the source home — e.g.
    /// `profiles/fred/.env`. Relative so that an absolute path on the
    /// discovering machine never reaches an emitted document.
    pub source_file: String,
}

impl CredentialRef {
    /// Record a credential by reference.
    ///
    /// Note the signature: there is no parameter for the value. A caller that
    /// happens to be holding one cannot pass it in even by accident.
    pub fn new(name: impl Into<String>, source_file: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_file: source_file.into(),
        }
    }
}

impl std::fmt::Display for CredentialRef {
    /// Renders the reference. There is nothing secret to withhold here — the
    /// type cannot hold a value — so this is safe by construction rather than
    /// by remembering to elide something.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (from {})", self.name, self.source_file)
    }
}


## EVIDENCE 2 — the conversion that is claimed to be the boundary

The CLI's internal `MigrationPlan` CAN hold a real api_key (it must: --include-credentials
writes one). The emitted `PortabilityPlan` is produced by this projection:

    pub fn to_portability(&self) -> PortabilityPlan {
        let source = match self.source {
            "openclaw" => PeerSource::OpenClaw,
            _ => PeerSource::Hermes,
        };
        let mut out = PortabilityPlan::new(source, self.source_home.display().to_string());

        for p in &self.profiles {
            let kind = if is_root_profile_id(&p.name) {
                ItemKind::RootProfile
            } else {
                ItemKind::Profile
            };
            let mut details = BTreeMap::new();
            if let Some(v) = &p.config.provider {
                details.insert("provider".to_string(), v.clone());
            }
            if let Some(v) = &p.config.model {
                details.insert("model".to_string(), v.clone());
            }
            if let Some(v) = &p.config.base_url {
                details.insert("base_url".to_string(), v.clone());
            }
            if !p.mcp_refs.is_empty() {
                details.insert("mcp_refs".to_string(), p.mcp_refs.join(","));
            }
            out.items.push(DiscoveredItem {
                kind,
                id: p.name.clone(),
                source_path: p.source_path.clone(),
                target: format!("profiles.{}", p.name),
                conflict: p.conflict,
                // Reference only — by TYPE there is nowhere for a value to go.
                credential: p.credential_env_var.as_ref().map(|name| {
                    CredentialRef::new(name.clone(), p.credential_file.clone().unwrap_or_default())
                }),
                details,
            });
        }

        for (name, srv) in &self.mcp_servers {
            let mut details = BTreeMap::new();
            details.insert("transport".to_string(), format!("{:?}", srv.transport));
            if let Some(c) = &srv.command {
                details.insert("command".to_string(), c.clone());
            }
            if let Some(u) = &srv.url {
                details.insert("url".to_string(), u.clone());
            }
            out.items.push(DiscoveredItem {
                kind: ItemKind::McpServer,
                id: name.clone(),
                source_path: String::new(),
                target: format!("mcp.servers.{name}"),
                conflict: false,
                credential: None,
                details,
            });
        }


## EVIDENCE 3 — the emitted item type

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

## EVIDENCE 4 — the multi-emitter probe (the measurement), and its result

fn every_emitter_of_a_plan_is_free_of_canary_values() {
    let (_g, _h) = rooted();

    for (kind, plan) in [("hermes", hermes_plan()), ("openclaw", openclaw_plan())] {
        let json = plan.to_json().unwrap();
        let compact = serde_json::to_string(&plan).unwrap();
        let debug = format!("{plan:?}");
        let debug_alt = format!("{plan:#?}");
        // The error path: a plan reported as part of a failure. `anyhow`'s
        // Debug rendering is what a user actually sees on stderr.
        let err = format!("{:?}", anyhow::anyhow!("import failed: {plan:?}"));
        // Every credential the plan discovered, through its own emitters.
        let creds: String = plan
            .items
            .iter()
            .filter_map(|i| i.credential.as_ref())
            .map(|c| format!("{c} {c:?} {}", serde_json::to_string(c).unwrap()))
            .collect::<Vec<_>>()
            .join("\n");

        for (what, rendered) in [
            ("to_json", &json),
            ("serde compact", &compact),
            ("Debug", &debug),
            ("Debug alternate", &debug_alt),
            ("anyhow error path", &err),
            ("CredentialRef emitters", &creds),
        ] {
            assert_no_canaries(kind, what, rendered);
        }

        // POSITIVE half — without these, a plan that rendered to nothing would
        // satisfy every assertion above.
        assert!(
            !json.is_empty() && json.len() > 200,
            "{kind}: json too small"
        );
        assert!(
            debug.contains("PortabilityPlan"),
            "{kind}: Debug is not the plan"
        );
        assert!(
            !creds.is_empty(),
            "{kind}: NO credential was discovered, so the credential emitters were never exercised"
        );
        assert!(
            creds.contains("source_file") || creds.contains("from "),
            "{kind}: credential rendering lost its source reference: {creds}"
        );
    }
}

RESULT, measured on Linux at the plan's SHA:
    Starting 1 test across 43 binaries (2057 tests skipped)
        PASS [0.028s] (1/1) wcore-cli::migrate_typed_dryrun every_emitter_of_a_plan_is_free_of_canary_values
    Summary [0.033s] 1 test run: 1 passed, 2057 skipped

## EVIDENCE 5 — the emitted document from the canary corpus (excerpt)

The corpus manifest declares 36 canary tokens for hermes; 0 appear in the emitted document,
and 13 items carry a credential reference (so the absence check is not vacuous).

{
  "source": "hermes",
  "source_home": "crates/wcore-cli/tests/fixtures/portability/hermes",
  "items": [
    {
      "kind": "root_profile",
      "id": "hermes/root",
      "source_path": ".",
      "target": "profiles.hermes/root",
      "conflict": false,
      "credential": {
        "name": "OPENROUTER_API_KEY",
        "source_file": ".env"
      },
      "details": {
        "base_url": "https://openrouter.ai/api/v1",
        "mcp_refs": "ijfw-memory",
        "model": "anthropic/claude-opus-4.6",
        "provider": "openrouter"
      }
    },
    {
      "kind": "profile",
      "id": "flux-backend-eng",
      "source_path": "profiles/flux-backend-eng",
      "target": "profiles.flux-backend-eng",
      "conflict": false,
      "credential": {
        "name": "DEEPSEEK_API_KEY",
        "source_file": "profiles/flux-backend-eng/.env"
      },
      "details": {
        "base_url": "https://api.deepseek.com/v1",
        "model": "deepseek-v4-pro",
        "provider": "deepseek"
      }
    },
    {
      "kind": "profile",
      "id": "flux-ceo",
      "source_path": "profiles/flux-ceo",
      "target": "profiles.flux-ceo",
      "conflict": false,
      "credential": {
        "name": "DEEPSEEK_API_KEY",
        "source_file": "profiles/flux-ceo/.env"
      },
      "details": {
        "base_url": "https://api.deepseek.com/v1",
        "model": "deepseek-v4-pro",
        "provider": "deepseek"
      }
    },
    {
      "kind": "profile",
      "id": "flux-cro",
      "source_path": "profiles/flux-cro",
      "target": "profiles.flux-cro",
      "conflict": false,
      "credential": {
        "name": "DEEPSEEK_API_KEY",
        "source_file": "profiles/flux-cro/.env"
      },
      "details": {
        "base_url": "https://api.deepseek.com/v1",
        "mo
## Your answer

Name the specific field, variant or accessor through which a value could (or could not)
travel. If you can name a path the probe does NOT cover, say so explicitly — that is a
finding, not a vote.

End your answer with exactly two lines:
PANEL-VERDICT: <contract-holds|contract-cosmetic|contract-leaks>
PANEL-BASIS: <one sentence>
