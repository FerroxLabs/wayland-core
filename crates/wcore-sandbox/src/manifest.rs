//! `SandboxManifest` — the TOML schema a plugin declares to request
//! sandboxed execution of a tool. Parsed from the plugin manifest fragment
//! and passed into `SandboxBackend::execute` at run time.
//!
//! v0.6.3 schema (BREAKING — replaces the v0.6.2 schema):
//! - Path-based filesystem allowlists (`fs_read_allow` / `fs_write_allow`)
//!   replace the old `allow_mounts` + Docker-bind concept. Backends translate
//!   these to bind mounts (Docker), bwrap `--ro-bind` / `--bind` (Linux),
//!   sandbox-exec rules (macOS), or AppContainer ACLs (Windows).
//! - `network` now carries an inline `AllowHosts(Vec<String>)` variant that
//!   replaces the separate `allow_hosts` field; backends without a DNS gate
//!   return `PolicyNotSupported` rather than silently downgrading.
//! - `syscall_policy` (Linux-only) and `timeout` are first-class.
//! - Resource limits are advisory; see `ResourceLimitEnforcement` in the
//!   crate root for what each backend can actually enforce.

use crate::error::{Result, SandboxError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "hosts")]
pub enum NetworkPolicy {
    /// Inherit host network (default — pre-sandbox parity).
    #[default]
    Inherit,
    /// Block all network access.
    Deny,
    /// Egress to listed hostnames only. Backend may emit
    /// `PolicyNotSupported` if no DNS gate is available (e.g. Docker today).
    AllowHosts(Vec<String>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyscallPolicy {
    /// No syscall filter beyond what the host kernel enforces.
    #[default]
    Inherit,
    /// Strict seccomp filter (Linux-only via libseccomp; ignored elsewhere).
    Strict,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxManifest {
    /// Read-allowed paths (absolute, on host). Backend translates to
    /// bind/profile rules.
    #[serde(default)]
    pub fs_read_allow: Vec<PathBuf>,
    /// Write-allowed paths.
    #[serde(default)]
    pub fs_write_allow: Vec<PathBuf>,
    /// Read-DENIED paths (absolute, canonicalized). Backends deny reads even
    /// under an `fs_read_allow` subtree. Empty = today's behavior.
    #[serde(default)]
    pub fs_read_deny: Vec<PathBuf>,
    /// Paths the child may `stat` but may NEVER read the CONTENTS of.
    ///
    /// This exists because "denied" is not one behaviour across the three host
    /// sandboxes, and one caller — libgit2 — can survive one spelling of it and
    /// not the other. Measured, not assumed:
    ///
    /// * **bwrap** builds the child's mount namespace CONSTRUCTIVELY, so a path
    ///   that was never bound is simply **absent**: `stat("/root/.gitconfig")`
    ///   returns **ENOENT**. libgit2 reads that as "this user has no global
    ///   config" and carries on.
    /// * **seatbelt** overlays rules on the REAL filesystem, so an ungranted
    ///   path returns **EPERM** whether or not it exists. libgit2 treats EPERM
    ///   as a hard error — `failed to stat '<home>/.gitconfig'; class=Config
    ///   (7)` — and every `cargo new` on macOS died there, taking `cargo build`
    ///   with it.
    ///
    /// The narrowest repair is to let the child learn the file EXISTS without
    /// letting it read a byte, which is exactly `file-read-metadata` in SBPL.
    /// Redirecting the env instead does not work: libgit2 derives the global
    /// config path from `$HOME` and ignores `GIT_CONFIG_GLOBAL` (measured), and
    /// redirecting `$HOME` breaks the rustup shim (`Unable to proceed. Could
    /// not locate working directory`, also measured).
    ///
    /// **Backend contract.** A metadata grant is a grant of METADATA. No
    /// backend may widen it into a content read, and every backend must leave
    /// the contents unreadable:
    ///
    /// | Backend | Translation |
    /// |---|---|
    /// | `sandbox_exec` (macOS) | `(allow file-read-metadata (literal …))` — exact, and emitted before `fs_read_deny` so a deny still wins |
    /// | `bwrap` (Linux) | Not expressible, and NOT NEEDED: an unbound path already answers ENOENT, which is strictly less information than this grant asks for. Deliberately not bound — see the `bwrap` module docs |
    /// | `appcontainer` (Windows) | **Not yet honoured** — the ACL lease has no metadata-only mask, so the path stays Access Denied and Windows keeps the libgit2 failure. The fix is a `FILE_READ_ATTRIBUTES`-only mask in `acl_lease::canonical_intents`; it is deliberately not made here because it mutates an ACE on a file in the operator's own home and no Windows host was available to prove it |
    /// | `docker` | Same shape as bwrap — an unmounted path is absent |
    ///
    /// Entries are absolute host paths and are deliberately **NOT** gated on
    /// existence by the caller: under seatbelt a MISSING file is EPERM too, so
    /// gating on `exists()` would leave a host with no `~/.gitconfig` broken
    /// (measured).
    #[serde(default)]
    pub fs_metadata_read_allow: Vec<PathBuf>,
    /// Network policy.
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Syscall policy (Linux only, ignored elsewhere).
    #[serde(default)]
    pub syscall_policy: SyscallPolicy,
    /// Wall-clock timeout for the child process. `None` means the CALLER owns
    /// the bound: the host backends (bwrap, sandbox-exec) impose no cap of
    /// their own, so a caller that passes `None` must apply its own timeout.
    /// The container/AppContainer backends still fall back to 60 s. TOML
    /// encoding uses serde's default struct form (`{ secs = N, nanos = M }`);
    /// programmatic callers pass a `Duration` directly.
    #[serde(default)]
    pub timeout: Option<Duration>,
    /// Max RSS bytes for the child. Enforced where the backend can;
    /// best-effort elsewhere (see `ResourceLimitEnforcement`).
    #[serde(default)]
    pub max_memory_bytes: Option<u64>,
    /// Max CPU seconds.
    #[serde(default)]
    pub max_cpu_secs: Option<u64>,
    /// Explicit env injected into the child. Backends ALWAYS scrub host env
    /// first (`env -i` style).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Docker-only: container image. Default
    /// `ghcr.io/tradecanyon/wcore-sandbox:base`. Other backends ignore this
    /// field.
    #[serde(default = "default_image")]
    pub image: String,
}

pub(crate) fn default_image() -> String {
    "ghcr.io/tradecanyon/wcore-sandbox:base".into()
}

/// Hard-containment filesystem model.
///
/// Encodes the one filesystem shape a [`crate::HardContainmentAuthority`] will
/// accept: the candidate source is bound READ-ONLY, and the ONLY writable
/// mounts are parent-created, transaction-private roots (scratch / build /
/// cache). Global temp, the user home / config / credential directories, and
/// any network egress are structurally DENIED — there is no field through which
/// a caller can request them, so no "bypass mode" is expressible.
///
/// The type validates its inputs at construction; an out-of-policy candidate or
/// writable root fails closed rather than silently downgrading. It is the only
/// sanctioned way to describe the policy that mints hard containment, and the
/// derived [`ContainmentPolicyIdentity`] is what the authority binds and
/// re-checks for drift at spawn.
#[derive(Debug, Clone)]
pub struct HardContainmentFilesystem {
    read_only_candidate: PathBuf,
    private_writable_roots: Vec<PathBuf>,
    syscall_policy: SyscallPolicy,
}

impl HardContainmentFilesystem {
    /// Build a hard-containment filesystem policy.
    ///
    /// `read_only_candidate` is the untrusted source, bound read-only.
    /// `private_writable_roots` are parent-created, transaction-private roots
    /// and are the ONLY writable mounts. Every path must be absolute and must
    /// NOT be a denied location (global temp or a home / config / credential
    /// directory); each writable root must be disjoint from the candidate.
    /// Existence is deliberately NOT required — the mounts are created by the
    /// owning transaction and bound with the backend's "-try" semantics.
    pub fn new(read_only_candidate: PathBuf, private_writable_roots: Vec<PathBuf>) -> Result<Self> {
        if !read_only_candidate.is_absolute() {
            return Err(SandboxError::PathDenied(format!(
                "hard-containment candidate must be absolute: {}",
                read_only_candidate.display()
            )));
        }
        if path_has_traversal(&read_only_candidate) {
            return Err(SandboxError::PathDenied(format!(
                "hard-containment candidate must not contain '.'/'..' components: {}",
                read_only_candidate.display()
            )));
        }
        if let Some(reason) = denied_location(&read_only_candidate) {
            return Err(SandboxError::PathDenied(format!(
                "hard-containment candidate is a denied location ({reason}): {}",
                read_only_candidate.display()
            )));
        }
        let mut roots: Vec<PathBuf> = Vec::with_capacity(private_writable_roots.len());
        for root in private_writable_roots {
            if !root.is_absolute() {
                return Err(SandboxError::PathDenied(format!(
                    "hard-containment writable root must be absolute: {}",
                    root.display()
                )));
            }
            if path_has_traversal(&root) {
                return Err(SandboxError::PathDenied(format!(
                    "hard-containment writable root must not contain '.'/'..' components: {}",
                    root.display()
                )));
            }
            if let Some(reason) = denied_location(&root) {
                return Err(SandboxError::PathDenied(format!(
                    "hard-containment writable root is a denied location ({reason}): {}",
                    root.display()
                )));
            }
            if root == read_only_candidate
                || root.starts_with(&read_only_candidate)
                || read_only_candidate.starts_with(&root)
            {
                return Err(SandboxError::PathDenied(format!(
                    "hard-containment writable root overlaps the read-only candidate: {}",
                    root.display()
                )));
            }
            roots.push(root);
        }
        roots.sort();
        roots.dedup();
        Ok(Self {
            read_only_candidate,
            private_writable_roots: roots,
            // Syscall policy is inherited by default so the read-only-candidate /
            // private-writable-root + network-deny model is the load-bearing
            // containment here; a strict seccomp filter can be layered later
            // without changing this authority's shape.
            syscall_policy: SyscallPolicy::Inherit,
        })
    }

    /// The read-only candidate source path.
    pub fn candidate(&self) -> &Path {
        &self.read_only_candidate
    }

    /// The parent-created, transaction-private writable roots.
    pub fn private_writable_roots(&self) -> &[PathBuf] {
        &self.private_writable_roots
    }

    /// The normalized, order-independent identity of this policy. The authority
    /// binds this at mint and re-derives it at spawn; any change refuses.
    pub fn policy_identity(&self) -> ContainmentPolicyIdentity {
        ContainmentPolicyIdentity {
            read_only_candidate: self.read_only_candidate.clone(),
            private_writable_roots: self.private_writable_roots.clone(),
            network: NetworkPolicy::Deny,
            syscall_policy: self.syscall_policy,
        }
    }

    /// Lower the policy to a concrete [`SandboxManifest`]. Network is always
    /// `Deny`; the candidate is the sole read mount; the private roots are the
    /// sole write mounts. Every field is set explicitly so a future
    /// `SandboxManifest` field cannot silently widen the policy.
    pub fn to_manifest(&self) -> SandboxManifest {
        SandboxManifest {
            fs_read_allow: vec![self.read_only_candidate.clone()],
            fs_write_allow: self.private_writable_roots.clone(),
            fs_read_deny: Vec::new(),
            // Hard containment names its whole filesystem shape up front and
            // has no channel through which a caller could request a stat on a
            // host path, so there is nothing to lower here.
            fs_metadata_read_allow: Vec::new(),
            network: NetworkPolicy::Deny,
            syscall_policy: self.syscall_policy,
            timeout: None,
            max_memory_bytes: None,
            max_cpu_secs: None,
            env: Vec::new(),
            image: default_image(),
        }
    }
}

/// Order-independent identity of a [`HardContainmentFilesystem`] policy.
///
/// Held privately inside [`crate::HardContainmentAuthority`] and compared by
/// value at spawn. Structural equality (not a hash) is used so there is no
/// collision surface: two policies are the same iff their normalized fields are
/// byte-identical. Not serializable — it never crosses a trust boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct ContainmentPolicyIdentity {
    read_only_candidate: PathBuf,
    private_writable_roots: Vec<PathBuf>,
    network: NetworkPolicy,
    syscall_policy: SyscallPolicy,
}

impl std::fmt::Debug for ContainmentPolicyIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted: the candidate + writable-root paths are the contained
        // execution's filesystem plan. Only shapes/policies are shown so the
        // plan never leaks into logs.
        f.debug_struct("ContainmentPolicyIdentity")
            .field("read_only_candidate", &"<redacted>")
            .field("private_writable_roots", &self.private_writable_roots.len())
            .field("network", &self.network)
            .field("syscall_policy", &self.syscall_policy)
            .finish()
    }
}

/// True if `path` contains any `.`/`..` (non-`Normal`) component. The lexical
/// `starts_with` denial and candidate-overlap checks in
/// [`HardContainmentFilesystem::new`] only hold for normalized paths — a `..`
/// segment slips past both and is then resolved by the kernel at bind time.
/// Mirrors the `Component::Normal` discipline enforced elsewhere in this crate
/// (e.g. `directory_authority`, `docker`, `appcontainer`).
fn path_has_traversal(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    })
}

/// Global temp roots that may never host a hard-containment mount.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
    ];
    for var in ["TMPDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(var) {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                roots.push(path);
            }
        }
    }
    roots
}

/// The current user's home directory, if the environment exposes one.
fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(var) {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                return Some(path);
            }
        }
    }
    None
}

/// Return a reason string when `path` is a location that hard containment must
/// never bind (global temp, the home directory itself, or a known credential /
/// config directory). Comparisons are component-aware (`Path::starts_with`), so
/// `/tmpfoo` does not match `/tmp`.
fn denied_location(path: &Path) -> Option<&'static str> {
    for temp in temp_roots() {
        if path == temp || path.starts_with(&temp) {
            return Some("global temp");
        }
    }
    if let Some(home) = home_dir() {
        if path == home {
            return Some("home directory");
        }
        // Credential / config directories under the home root. A
        // transaction-private root deliberately lives OUTSIDE these; the parent
        // must not hand a worker write access to the user's secrets.
        const CREDENTIAL_SUBDIRS: [&str; 9] = [
            ".ssh", ".aws", ".gnupg", ".kube", ".docker", ".config", ".netrc", ".azure", ".gcloud",
        ];
        for sub in CREDENTIAL_SUBDIRS {
            let denied = home.join(sub);
            if path == denied || path.starts_with(&denied) {
                return Some("home config/credentials");
            }
        }
    }
    None
}

/// Absolute fixture root for hard-containment policy tests, in this crate's one
/// place so no test re-derives it.
///
/// `HardContainmentFilesystem::new` requires an ABSOLUTE candidate, and
/// absoluteness is platform-defined: `/srv/wl-hard` is absolute on unix but NOT
/// on Windows, where `Path::is_absolute` demands a drive or UNC prefix. The
/// literal was unix-shaped because this crate's test targets had never compiled
/// on Windows, so these cases had never executed there. The guard under test is
/// correct on both platforms — only the fixture was wrong — so this fixes the
/// fixture and leaves every assertion intact. The path is never created on disk;
/// all of these cases are pure policy validation.
#[cfg(test)]
pub(crate) fn hard_fixture_root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\srv\wl-hard")
    } else {
        PathBuf::from("/srv/wl-hard")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_read_deny_defaults_empty() {
        let m = SandboxManifest::default();
        assert!(
            m.fs_read_deny.is_empty(),
            "fs_read_deny must default to empty (today's behavior)"
        );
    }

    #[test]
    fn fs_read_deny_roundtrips_toml() {
        let m = SandboxManifest {
            fs_read_deny: vec![
                PathBuf::from("/tmp/secret.env"),
                PathBuf::from("/home/user/.ssh/id_rsa"),
            ],
            ..Default::default()
        };
        let serialized = toml::to_string(&m).expect("serialize");
        let back: SandboxManifest = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(m.fs_read_deny, back.fs_read_deny);
    }

    #[test]
    fn fs_read_deny_missing_from_toml_defaults_empty() {
        // A serialized manifest without the field (old schema) must still
        // deserialize cleanly with an empty deny list.
        let toml_str = "";
        let m: SandboxManifest = toml::from_str(toml_str).expect("deserialize");
        assert!(
            m.fs_read_deny.is_empty(),
            "missing fs_read_deny in old schema must default to empty"
        );
    }

    #[test]
    fn hard_containment_filesystem_is_read_only_candidate_plus_private_writes() {
        let root = hard_fixture_root();
        let fs = HardContainmentFilesystem::new(
            root.join("candidate"),
            vec![root.join("scratch"), root.join("cache")],
        )
        .expect("policy must validate");
        let m = fs.to_manifest();
        // Candidate is the sole read mount; it is NOT writable.
        assert_eq!(m.fs_read_allow, vec![root.join("candidate")]);
        assert!(!m.fs_write_allow.contains(&root.join("candidate")));
        // Only the private roots are writable, and network is denied — there is
        // no bypass field to widen this.
        assert!(m.fs_write_allow.contains(&root.join("scratch")));
        assert!(m.fs_write_allow.contains(&root.join("cache")));
        assert_eq!(m.network, NetworkPolicy::Deny);
    }

    #[test]
    fn hard_containment_rejects_relative_candidate() {
        let err = HardContainmentFilesystem::new(PathBuf::from("relative/candidate"), Vec::new())
            .expect_err("relative candidate must be refused");
        assert!(matches!(err, SandboxError::PathDenied(_)));
    }

    #[test]
    fn hard_containment_denies_candidate_under_global_temp() {
        let temp = std::env::temp_dir().join("wl-hard-should-be-denied");
        assert!(
            matches!(
                HardContainmentFilesystem::new(temp, Vec::new()),
                Err(SandboxError::PathDenied(_))
            ),
            "a candidate under global temp must be denied"
        );
    }

    #[test]
    fn denied_location_flags_temp_and_credentials() {
        // Read-only assertions on the denial predicate — no environment
        // mutation, so this is race-free under parallel test runners.
        assert!(denied_location(&std::env::temp_dir()).is_some());
        assert!(denied_location(&std::env::temp_dir().join("nested")).is_some());
        assert!(denied_location(Path::new("/srv/wl-hard/candidate")).is_none());
        if let Some(home) = home_dir() {
            assert!(denied_location(&home).is_some(), "home root must be denied");
            assert!(
                denied_location(&home.join(".ssh").join("id_rsa")).is_some(),
                "~/.ssh must be denied"
            );
            assert!(
                denied_location(&home.join(".aws").join("credentials")).is_some(),
                "~/.aws must be denied"
            );
        }
    }

    #[test]
    fn hard_containment_rejects_writable_root_overlapping_candidate() {
        let err = HardContainmentFilesystem::new(
            PathBuf::from("/srv/wl-hard/candidate"),
            vec![PathBuf::from("/srv/wl-hard/candidate/inner")],
        )
        .expect_err("an overlapping writable root must be refused");
        assert!(matches!(err, SandboxError::PathDenied(_)));
    }

    #[test]
    fn hard_containment_rejects_traversal_components() {
        // A `..` component slips past the lexical denial + overlap checks and is
        // then resolved by the kernel at bind time (e.g. escaping to ~/.ssh or
        // writing inside the read-only candidate). Both the candidate and every
        // writable root must reject non-`Normal` components.
        assert!(
            matches!(
                HardContainmentFilesystem::new(
                    PathBuf::from("/srv/wl-hard/../../home/user/.ssh"),
                    Vec::new()
                ),
                Err(SandboxError::PathDenied(_))
            ),
            "a `..`-traversal candidate must be refused"
        );
        assert!(
            matches!(
                HardContainmentFilesystem::new(
                    PathBuf::from("/srv/wl-hard/candidate"),
                    vec![PathBuf::from("/srv/wl-hard/candidate/../scratch")]
                ),
                Err(SandboxError::PathDenied(_))
            ),
            "a `..`-traversal writable root must be refused"
        );
    }

    #[test]
    fn policy_identity_is_order_independent() {
        let root = hard_fixture_root();
        let a = HardContainmentFilesystem::new(
            root.join("candidate"),
            vec![root.join("scratch"), root.join("cache")],
        )
        .unwrap();
        let b = HardContainmentFilesystem::new(
            root.join("candidate"),
            vec![root.join("cache"), root.join("scratch")],
        )
        .unwrap();
        assert_eq!(a.policy_identity(), b.policy_identity());
        let c = HardContainmentFilesystem::new(root.join("candidate"), vec![root.join("scratch")])
            .unwrap();
        assert_ne!(a.policy_identity(), c.policy_identity());
    }
}
