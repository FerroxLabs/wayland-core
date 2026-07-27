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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_ref_carries_no_value_through_any_emitter() {
        // The canonical failure this type prevents: a caller holds a real
        // secret and records the credential. The value must not survive into
        // any rendering, and the type must give it nowhere to live.
        let secret = "sk-live-THIS-MUST-NEVER-APPEAR-0123456789";
        let c = CredentialRef::new("DEEPSEEK_API_KEY", "profiles/fred/.env");

        let json = serde_json::to_string(&c).unwrap();
        let debug = format!("{c:?}");
        let display = format!("{c}");
        // The error path is a real emitter: a plan is often reported as part of
        // a failure, and `anyhow`'s Debug rendering is what a user sees.
        let err = format!("{:?}", anyhow::anyhow!("import failed for {c}"));

        for (what, rendered) in [
            ("json", &json),
            ("debug", &debug),
            ("display", &display),
            ("error", &err),
        ] {
            assert!(
                !rendered.contains(secret),
                "credential value leaked through the {what} emitter: {rendered}"
            );
        }

        // Positive half — without it a type that rendered to the empty string
        // would pass the assertions above vacuously.
        assert!(
            json.contains("DEEPSEEK_API_KEY"),
            "json lost the name: {json}"
        );
        assert!(
            json.contains("profiles/fred/.env"),
            "json lost the source file: {json}"
        );
        assert!(
            debug.contains("DEEPSEEK_API_KEY"),
            "debug is empty: {debug}"
        );
    }

    #[test]
    fn credential_ref_json_shape_has_exactly_two_location_fields() {
        // Guards the invariant directly: if someone adds a value-bearing field,
        // this fails rather than waiting for a leak to be observed downstream.
        let c = CredentialRef::new("OPENROUTER_API_KEY", ".env");
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        let obj = v
            .as_object()
            .expect("CredentialRef must serialize as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["name", "source_file"],
            "CredentialRef gained a field; if it can hold a credential value the \
             structural redaction contract is broken"
        );
    }

    #[test]
    fn credential_ref_ordering_is_total_and_by_location() {
        // Discovery sorts by this ordering, so it must be total and derived
        // from the data rather than from walk order.
        let a = CredentialRef::new("A_KEY", "a/.env");
        let b = CredentialRef::new("B_KEY", "a/.env");
        let c = CredentialRef::new("A_KEY", "b/.env");
        assert!(a < b, "same file, name orders");
        assert!(a < c, "same name, source_file orders");
        // `name` is the FIRST field, so it dominates: c (A_KEY) sorts before
        // b (B_KEY) even though c's file sorts later.
        assert!(c < b, "name must dominate source_file in the ordering");
    }
}
