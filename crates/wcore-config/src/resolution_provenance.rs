//! Provenance for the configuration sources used to build one runtime config.
//!
//! These types deliberately carry source identity and disposition only. They
//! never retain configuration contents or environment values, so callers can
//! safely project them into local diagnostics after applying their own path
//! disclosure policy.
//!
//! This is diagnostic evidence, not a tamper-resistant attestation. Callers
//! must not treat it as proof that local files or process memory were unmodified.

use std::path::PathBuf;

/// The role a source plays in the configuration precedence chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSourceRole {
    Global,
    Project,
    Profile,
    CliOverrides,
    EnvironmentOverride { variable: String },
}

/// What happened to a source while resolving the effective configuration.
///
/// A source can have more than one disposition: for example, a loaded project
/// file can also be restricted by workspace trust, and a legacy project layout
/// can be overridden by the canonical file-form layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSourceDisposition {
    Loaded,
    Absent,
    Ignored,
    Unreadable,
    Invalid,
    Overridden,
    Restricted,
}

/// Provenance for one source without any of the source's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSourceEvidence {
    pub role: ConfigSourceRole,
    pub path: Option<PathBuf>,
    pub precedence: u16,
    pub dispositions: Vec<ConfigSourceDisposition>,
}

impl ConfigSourceEvidence {
    #[must_use]
    pub fn new(
        role: ConfigSourceRole,
        path: Option<PathBuf>,
        precedence: u16,
        disposition: ConfigSourceDisposition,
    ) -> Self {
        Self {
            role,
            path,
            precedence,
            dispositions: vec![disposition],
        }
    }

    pub(crate) fn add_disposition(&mut self, disposition: ConfigSourceDisposition) {
        if !self.dispositions.contains(&disposition) {
            self.dispositions.push(disposition);
        }
    }
}

/// Which launch-time authority selected the process's effective profile home.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LaunchBindingEvidence {
    /// `activate_for_launch` has not run and no explicit home is visible.
    #[default]
    Unavailable,
    /// No explicit home or isolated profile was selected.
    DefaultHome,
    /// `WAYLAND_HOME` was already present at process entry. Its value is never
    /// retained here.
    ExplicitWaylandHome,
    /// A valid isolated profile was selected and bound at process entry.
    BoundProfile {
        name: String,
        source: ProfileSelectionSource,
    },
    /// A profile was selected but could not be bound. Host modes must not
    /// impersonate it by falling through to the default home.
    UnboundProfile {
        name: String,
        source: ProfileSelectionSource,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSelectionSource {
    CommandLine,
    ActivePointer,
}

/// Complete diagnostic source evidence for one resolution attempt.
///
/// This records what the resolver observed; it is not cryptographically bound
/// to source files and must not be used as an integrity attestation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigResolutionProvenance {
    pub sources: Vec<ConfigSourceEvidence>,
    pub launch_binding: LaunchBindingEvidence,
}

impl ConfigResolutionProvenance {
    #[must_use]
    pub fn source(&self, role: &ConfigSourceRole) -> Option<&ConfigSourceEvidence> {
        self.sources.iter().find(|source| &source.role == role)
    }
}

/// A successfully produced value and the source evidence used to produce it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithConfigProvenance<T> {
    pub value: T,
    pub provenance: ConfigResolutionProvenance,
}

/// A failed resolution that still carries the evidence collected before the
/// failure. Existing compatibility APIs flatten this back to `anyhow::Error`.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct ConfigResolutionError {
    pub provenance: ConfigResolutionProvenance,
    #[source]
    pub source: anyhow::Error,
}

impl ConfigResolutionError {
    #[must_use]
    pub fn new(provenance: ConfigResolutionProvenance, source: impl Into<anyhow::Error>) -> Self {
        Self {
            provenance,
            source: source.into(),
        }
    }
}
