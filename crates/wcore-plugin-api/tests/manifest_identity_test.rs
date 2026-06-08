//! Wave SC SECURITY MAJOR fix — `PluginIdentity` enforces verified
//! origin so a malicious crate cannot impersonate `wayland-browser` /
//! `wayland-cua` by setting matching `name` in its `PluginManifest`.
//!
//! Closes the audit finding: identity is now anchored to either the
//! engine's compile-time inventory registry (Static) or a path-prefix
//! check against the host's plugin root (PathPrefix). A manifest from
//! `~/Downloads/evil-browser/` claiming the canonical name is refused
//! by `PluginIdentity::from_path_prefix`.

use std::fs;
#[cfg(unix)]
use std::path::PathBuf;

use tempfile::TempDir;
use wcore_plugin_api::PluginIdentity;

#[test]
fn static_identity_is_anchored_to_symbol() {
    let id = PluginIdentity::from_static("wayland-browser");
    match &id {
        PluginIdentity::Static { symbol } => assert_eq!(symbol, "wayland-browser"),
        other => panic!("expected Static, got {other:?}"),
    }
    assert!(
        id.is_static(),
        "Static identity must report is_static = true"
    );
}

#[test]
fn path_prefix_accepts_manifest_under_allowed_root() {
    let tmp = TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("wayland-browser");
    fs::create_dir_all(&plugin_dir).unwrap();
    let manifest_path = plugin_dir.join("manifest.toml");
    fs::write(&manifest_path, b"[plugin]\nname = \"wayland-browser\"\n").unwrap();

    let allowed_roots = vec![tmp.path().to_path_buf()];
    let id = PluginIdentity::from_path_prefix(&manifest_path, &allowed_roots)
        .expect("manifest under allowed root must be accepted");
    match &id {
        PluginIdentity::PathPrefix { manifest_path: p } => {
            assert!(p.starts_with(tmp.path().canonicalize().unwrap()));
        }
        other => panic!("expected PathPrefix, got {other:?}"),
    }
}

#[test]
fn path_prefix_refuses_manifest_outside_allowed_root() {
    let allowed_tmp = TempDir::new().unwrap();
    let evil_tmp = TempDir::new().unwrap();
    let evil_path = evil_tmp.path().join("manifest.toml");
    fs::write(&evil_path, b"[plugin]\nname = \"wayland-browser\"\n").unwrap();

    let allowed_roots = vec![allowed_tmp.path().to_path_buf()];
    let r = PluginIdentity::from_path_prefix(&evil_path, &allowed_roots);
    assert!(
        r.is_err(),
        "manifest outside allowed roots must be refused, got {r:?}"
    );
    let err_msg = r.unwrap_err().to_string();
    assert!(
        err_msg.contains("outside the host's allowed plugin roots"),
        "error message should explain the rejection: {err_msg}"
    );
}

#[test]
fn path_prefix_refuses_nonexistent_path() {
    // A manifest path that can't be canonicalized (doesn't exist) is
    // refused — even with a permissive allowlist. Catches typo'd
    // path-prefix attacks where the attacker writes a "look-alike"
    // path that doesn't yet exist on disk.
    let tmp = TempDir::new().unwrap();
    let allowed_roots = vec![tmp.path().to_path_buf()];
    let r =
        PluginIdentity::from_path_prefix(tmp.path().join("does-not-exist.toml"), &allowed_roots);
    assert!(r.is_err(), "nonexistent path must be refused");
}

#[test]
fn default_plugin_root_is_under_data_dir() {
    let root = PluginIdentity::default_plugin_root();
    let s = root.to_string_lossy();
    assert!(
        s.contains("wayland"),
        "default root should contain 'wayland': {s}"
    );
    assert!(
        s.contains("plugins"),
        "default root should end with 'plugins': {s}"
    );
}

#[test]
fn signed_identity_is_a_distinct_variant() {
    // The Signed variant is reserved for v0.3.0 — pin its shape so
    // future refactors don't accidentally fold it into a non-typed
    // string field.
    let id = PluginIdentity::Signed {
        public_key: "abc".into(),
        signature: "def".into(),
    };
    match id {
        PluginIdentity::Signed {
            public_key,
            signature,
        } => {
            assert_eq!(public_key, "abc");
            assert_eq!(signature, "def");
        }
        other => panic!("expected Signed, got {other:?}"),
    }
}

// Windows symlinks need elevated privileges in CI — skip the
// canonicalization-bypass test on that target. The pure-path check in
// the sibling test `from_path_prefix_*` already exercises the prefix
// logic; this test only adds the realpath-resolution layer, which is
// unix-only by design.
#[cfg(unix)]
#[test]
fn path_prefix_canonicalizes_symlinks() {
    // A symlink that resolves OUTSIDE the allowed root must be
    // refused. Catches `~/Downloads/symlink-to-evil → /etc/passwd`
    // attacks where the path itself looks valid but the realpath
    // is outside the trust boundary.
    let allowed_tmp = TempDir::new().unwrap();
    let evil_tmp = TempDir::new().unwrap();

    // Real manifest lives in evil_tmp.
    let real_manifest = evil_tmp.path().join("manifest.toml");
    fs::write(&real_manifest, b"[plugin]\nname = \"wayland-browser\"\n").unwrap();

    // Symlink in allowed_tmp pointing at the evil manifest.
    let symlink_path: PathBuf = allowed_tmp.path().join("manifest.toml");
    std::os::unix::fs::symlink(&real_manifest, &symlink_path).unwrap();

    let allowed_roots = vec![allowed_tmp.path().to_path_buf()];
    let r = PluginIdentity::from_path_prefix(&symlink_path, &allowed_roots);
    assert!(
        r.is_err(),
        "symlink resolving outside allowed root must be refused, got {r:?}"
    );
}
