//! T3-3.4: Office-side skill enable/disable runtime — ported from the
//! prior Wayland Python engine (Stage 5 Task 7).
//!
//! HELPER module. The authoritative source of truth for which optional
//! skills are enabled lives in the Wayland Desktop's `office.db`; the
//! agent consumes the enabled list via the `/office/sync` JSON endpoint
//! and uses this module to materialize the chosen skill bundles under
//! the user's skills directory (typically `~/.wayland-core/skills/<slug>/`)
//! so the regular skill-discovery loader picks them up.
//!
//! ## Divergence from the Python source
//!
//! The Python source imports three cross-module symbols at import time:
//! - `wayland_constants.get_wayland_home()` — global Wayland home path.
//! - `tools.skills_hub.OptionalSkillSource` — Python class with a
//!   `fetch(slug) -> SkillBundle | None` method that returns a
//!   `SkillBundle` whose `files` is a `dict[str, bytes | str]`.
//! - `wayland_cli.office_sync.{emit_skill_authored, set_enabled}` —
//!   side-channel callbacks that fan events out to the `/office/events`
//!   long-poll subscribers and mirror the in-memory enabled set.
//!
//! None of these have a 1:1 Rust counterpart in the wcore stack today
//! (the office-sync HTTP layer is a Desktop-side concern, and the
//! skills-hub bundler is in the host process). To keep this crate's
//! dependency footprint tight, the Rust port introduces three trait
//! seams that callers wire up at runtime:
//!
//! - [`SkillSource`] — produces a [`SkillBundle`] for a given slug.
//! - [`EnabledMirror`] — receives the post-sync enabled set so the
//!   higher-level office-sync handler can mirror it.
//! - [`SkillAuthoredEmitter`] — receives `skill-authored` events for
//!   broadcast to long-poll subscribers.
//!
//! The skills root (equivalent to `get_wayland_home() / "skills"`) is
//! passed explicitly into [`OfficeRuntime::new`]; callers typically use
//! `wcore_skills::paths::user_skills_dir()` (which already handles
//! `dirs::home_dir()` cross-platform) but we don't depend on that crate
//! here to avoid pulling `wcore-skills` into `wcore-tools`.
//!
//! Concurrency: the Python source runs all four `office_runtime` entry
//! points from the same `office_sync` async handler, but the in-memory sets are
//! plain module globals. The Rust port wraps them in [`Mutex`] guards
//! so a multi-threaded host (e.g. `wcore-agent` on Tokio's multi-thread
//! runtime) can hit `is_enabled`/`is_office_origin` concurrently without
//! tearing.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Content of a single file inside a [`SkillBundle`]. Mirrors the
/// `bytes | str` dispatch in the Python source: text content is written
/// as UTF-8, binary content is written verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillFileContent {
    Text(String),
    Bytes(Vec<u8>),
}

/// A materializable skill bundle. Roughly equivalent to the
/// `SkillBundle` returned by the Python source's
/// `OptionalSkillSource.fetch()` — just the `files` mapping is required
/// here (other Python attributes like `version` / `description` are
/// loader concerns that don't affect on-disk materialization).
#[derive(Debug, Clone, Default)]
pub struct SkillBundle {
    /// Relative path inside the skill dir → file content.
    /// Using `BTreeMap` so iteration order is deterministic — tests
    /// otherwise hit map-iteration nondeterminism on identical input.
    pub files: BTreeMap<String, SkillFileContent>,
}

/// Source-of-bundles seam — mirrors the Python `OptionalSkillSource.fetch`.
pub trait SkillSource: Send + Sync {
    fn fetch(&self, slug: &str) -> Option<SkillBundle>;
}

/// Mirror-the-enabled-set seam — the Python source calls
/// `office_sync.set_enabled(_enabled)` so the `/office/events` snapshot
/// stays in sync with what the runtime knows about. The Rust port
/// defaults this to a no-op when the host doesn't wire one in.
pub trait EnabledMirror: Send + Sync {
    fn mirror_enabled(&self, enabled: &HashSet<String>);
}

/// Authored-skill broadcast seam — the Python source calls
/// `office_sync.emit_skill_authored(slug)` to fan a `skill-authored`
/// event out to the Desktop Settings pane via the long-poll bridge.
pub trait SkillAuthoredEmitter: Send + Sync {
    fn emit_skill_authored(&self, slug: &str);
}

/// Convenience no-op mirror so callers without a mirror callback don't
/// need to define an empty type.
pub struct NoopEnabledMirror;
impl EnabledMirror for NoopEnabledMirror {
    fn mirror_enabled(&self, _enabled: &HashSet<String>) {}
}

/// Convenience no-op emitter for callers that don't care about events.
pub struct NoopSkillAuthoredEmitter;
impl SkillAuthoredEmitter for NoopSkillAuthoredEmitter {
    fn emit_skill_authored(&self, _slug: &str) {}
}

/// Outcome of [`OfficeRuntime::load_skill`]. Mirrors the Python tuple
/// `(ok, err)`: callers treat `LoadOutcome::Failure { .. }` as a signal
/// to auto-disable the slug (prevents crash loops).
#[derive(Debug)]
pub enum LoadOutcome {
    Success,
    Failure(String),
}

impl LoadOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, LoadOutcome::Success)
    }
}

/// Per-instance office runtime state. The Python source uses
/// module-level globals; the Rust port encapsulates them so tests don't
/// need to share state.
pub struct OfficeRuntime {
    /// Equivalent to the Python `get_wayland_home() / "skills"` — root
    /// directory under which `<slug>/` directories get materialized.
    skills_root: PathBuf,
    source: Box<dyn SkillSource>,
    mirror: Box<dyn EnabledMirror>,
    emitter: Box<dyn SkillAuthoredEmitter>,
    /// Currently-enabled slugs (mirror of Desktop's `office.db` truth).
    enabled: Mutex<HashSet<String>>,
    /// Slugs materialized by this runtime — used to gate unload so a
    /// user-authored skill with a colliding name is never nuked.
    office_origin: Mutex<HashSet<String>>,
}

impl OfficeRuntime {
    pub fn new(
        skills_root: PathBuf,
        source: Box<dyn SkillSource>,
        mirror: Box<dyn EnabledMirror>,
        emitter: Box<dyn SkillAuthoredEmitter>,
    ) -> Self {
        Self {
            skills_root,
            source,
            mirror,
            emitter,
            enabled: Mutex::new(HashSet::new()),
            office_origin: Mutex::new(HashSet::new()),
        }
    }

    /// Convenience constructor for callers that don't need the mirror /
    /// emitter side-channels (tests, single-binary embedders).
    pub fn with_noop_callbacks(skills_root: PathBuf, source: Box<dyn SkillSource>) -> Self {
        Self::new(
            skills_root,
            source,
            Box::new(NoopEnabledMirror),
            Box::new(NoopSkillAuthoredEmitter),
        )
    }

    /// Replace the in-memory enabled set (called from the `/office/sync`
    /// handler). Mirrors into the [`EnabledMirror`] callback so the
    /// `/office/events` snapshot stays consistent.
    pub fn set_enabled(&self, slugs: HashSet<String>) {
        {
            let mut g = self.enabled.lock().expect("enabled mutex poisoned");
            *g = slugs;
            self.mirror.mirror_enabled(&g);
        }
    }

    pub fn is_enabled(&self, slug: &str) -> bool {
        self.enabled
            .lock()
            .expect("enabled mutex poisoned")
            .contains(slug)
    }

    pub fn is_office_origin(&self, slug: &str) -> bool {
        self.office_origin
            .lock()
            .expect("office_origin mutex poisoned")
            .contains(slug)
    }

    /// Materialize a bundled optional-skill into
    /// `<skills_root>/<slug>/`. Returns `LoadOutcome::Failure` on any
    /// I/O or fetch error; callers should treat that as a signal to
    /// auto-disable the slug to prevent crash loops (matching the
    /// Python contract).
    pub fn load_skill(&self, slug: &str) -> LoadOutcome {
        let bundle = match self.source.fetch(slug) {
            Some(b) => b,
            None => {
                return LoadOutcome::Failure(format!("skill {slug:?} not found in SkillSource"));
            }
        };

        let target = self.skills_root.join(slug);
        if let Err(e) = fs::create_dir_all(&target) {
            return LoadOutcome::Failure(format!(
                "create_dir_all({}) failed: {e}",
                target.display()
            ));
        }

        for (rel, content) in bundle.files.iter() {
            // Defense in depth: reject relative paths that try to
            // escape the skill directory via `..` or absolute prefixes.
            // The Python source trusts the bundle implicitly; the Rust
            // port keeps that posture but adds a containment check
            // because Rust's `Path::join` happily follows `..`.
            if !is_safe_relative(rel) {
                return LoadOutcome::Failure(format!(
                    "unsafe relative path {rel:?} in bundle for {slug:?}"
                ));
            }
            let dst = target.join(rel);
            if let Some(parent) = dst.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return LoadOutcome::Failure(format!(
                    "create_dir_all({}) failed: {e}",
                    parent.display()
                ));
            }
            let write_result = match content {
                SkillFileContent::Text(s) => fs::write(&dst, s.as_bytes()),
                SkillFileContent::Bytes(b) => fs::write(&dst, b),
            };
            if let Err(e) = write_result {
                return LoadOutcome::Failure(format!("write({}) failed: {e}", dst.display()));
            }
        }

        self.office_origin
            .lock()
            .expect("office_origin mutex poisoned")
            .insert(slug.to_string());
        LoadOutcome::Success
    }

    /// Remove a materialized skill from `<skills_root>/<slug>/`.
    /// Idempotent: missing dir is a silent no-op. Only acts on slugs
    /// that this runtime materialized (i.e. tracked as office-origin)
    /// so user-authored skills with colliding names are protected.
    pub fn unload_skill(&self, slug: &str) {
        let was_office_origin = self
            .office_origin
            .lock()
            .expect("office_origin mutex poisoned")
            .contains(slug);
        if !was_office_origin {
            // Matches the Python `_office_origin` gate — refuse to unload.
            return;
        }
        let target = self.skills_root.join(slug);
        if target.exists() {
            // `ignore_errors=True` equivalent — best-effort removal.
            let _ = fs::remove_dir_all(&target);
        }
        self.office_origin
            .lock()
            .expect("office_origin mutex poisoned")
            .remove(slug);
    }

    /// Called by the self-authoring code path (e.g. a future
    /// `skill_creator` tool) once a new `SKILL.md` has been written to
    /// `<skills_root>/<slug>/`. Marks the slug as office-origin (so it
    /// becomes eligible for `unload_skill`) and broadcasts a
    /// `skill-authored` event via the registered emitter.
    pub fn notify_skill_authored(&self, slug: &str) {
        self.office_origin
            .lock()
            .expect("office_origin mutex poisoned")
            .insert(slug.to_string());
        self.emitter.emit_skill_authored(slug);
    }
}

/// Reject relative paths that contain `..`, are absolute, or are
/// rooted (Windows drive letters / UNC). Component-based check so it's
/// agnostic to the separator the bundle uses.
fn is_safe_relative(rel: &str) -> bool {
    let p = Path::new(rel);
    if p.is_absolute() {
        return false;
    }
    for c in p.components() {
        use std::path::Component;
        match c {
            Component::Normal(_) | Component::CurDir => {}
            // RootDir, Prefix, ParentDir → unsafe.
            _ => return false,
        }
    }
    !rel.is_empty()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use tempfile::TempDir;

    /// In-memory test source.
    struct MapSource(BTreeMap<String, SkillBundle>);
    impl SkillSource for MapSource {
        fn fetch(&self, slug: &str) -> Option<SkillBundle> {
            self.0.get(slug).cloned()
        }
    }

    /// Records every `mirror_enabled` invocation.
    #[derive(Default, Clone)]
    struct RecordingMirror(Arc<StdMutex<Vec<HashSet<String>>>>);
    impl EnabledMirror for RecordingMirror {
        fn mirror_enabled(&self, enabled: &HashSet<String>) {
            self.0.lock().unwrap().push(enabled.clone());
        }
    }

    /// Records every `emit_skill_authored` invocation.
    #[derive(Default, Clone)]
    struct RecordingEmitter(Arc<StdMutex<Vec<String>>>);
    impl SkillAuthoredEmitter for RecordingEmitter {
        fn emit_skill_authored(&self, slug: &str) {
            self.0.lock().unwrap().push(slug.to_string());
        }
    }

    fn bundle_with(files: &[(&str, &str)]) -> SkillBundle {
        let mut b = SkillBundle::default();
        for (k, v) in files {
            b.files
                .insert((*k).to_string(), SkillFileContent::Text((*v).to_string()));
        }
        b
    }

    fn binary_bundle(rel: &str, bytes: Vec<u8>) -> SkillBundle {
        let mut b = SkillBundle::default();
        b.files
            .insert(rel.to_string(), SkillFileContent::Bytes(bytes));
        b
    }

    fn rt_with(
        skills_root: PathBuf,
        bundles: Vec<(&str, SkillBundle)>,
    ) -> (OfficeRuntime, RecordingMirror, RecordingEmitter) {
        let mut map = BTreeMap::new();
        for (slug, bundle) in bundles {
            map.insert(slug.to_string(), bundle);
        }
        let mirror = RecordingMirror::default();
        let emitter = RecordingEmitter::default();
        let rt = OfficeRuntime::new(
            skills_root,
            Box::new(MapSource(map)),
            Box::new(mirror.clone()),
            Box::new(emitter.clone()),
        );
        (rt, mirror, emitter)
    }

    #[test]
    fn set_enabled_and_is_enabled_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let (rt, mirror, _) = rt_with(tmp.path().to_path_buf(), vec![]);

        assert!(!rt.is_enabled("alpha"));

        let set: HashSet<String> = ["alpha", "beta"].iter().map(|s| s.to_string()).collect();
        rt.set_enabled(set);

        assert!(rt.is_enabled("alpha"));
        assert!(rt.is_enabled("beta"));
        assert!(!rt.is_enabled("gamma"));

        // Replacing the set should reflect immediately (matches the
        // Python `_enabled = set(slugs)` reassignment).
        let set2: HashSet<String> = ["gamma"].iter().map(|s| s.to_string()).collect();
        rt.set_enabled(set2);
        assert!(!rt.is_enabled("alpha"));
        assert!(rt.is_enabled("gamma"));

        // Mirror was invoked once per set_enabled call.
        let history = mirror.0.lock().unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].contains("alpha"));
        assert!(history[1].contains("gamma"));
    }

    #[test]
    fn load_skill_materializes_text_and_binary_files() {
        let tmp = TempDir::new().unwrap();
        let bundle = {
            let mut b = SkillBundle::default();
            b.files.insert(
                "SKILL.md".to_string(),
                SkillFileContent::Text("---\nname: foo\n---\nhi".to_string()),
            );
            b.files.insert(
                "assets/icon.bin".to_string(),
                SkillFileContent::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            );
            b
        };
        let (rt, _, _) = rt_with(tmp.path().to_path_buf(), vec![("foo", bundle)]);

        let outcome = rt.load_skill("foo");
        assert!(outcome.is_success(), "load_skill: {outcome:?}");
        assert!(rt.is_office_origin("foo"));

        let skill_md = tmp.path().join("foo").join("SKILL.md");
        assert_eq!(
            fs::read_to_string(&skill_md).unwrap(),
            "---\nname: foo\n---\nhi"
        );
        let icon = tmp.path().join("foo").join("assets").join("icon.bin");
        assert_eq!(fs::read(&icon).unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn load_skill_missing_in_source_returns_failure() {
        let tmp = TempDir::new().unwrap();
        let (rt, _, _) = rt_with(tmp.path().to_path_buf(), vec![]);
        let outcome = rt.load_skill("nope");
        match outcome {
            LoadOutcome::Failure(msg) => assert!(
                msg.contains("nope") && msg.contains("not found"),
                "unexpected msg: {msg}"
            ),
            LoadOutcome::Success => panic!("expected failure"),
        }
        // Failure must NOT mark the slug as office-origin (else
        // unload_skill would later try to remove a non-existent dir).
        assert!(!rt.is_office_origin("nope"));
    }

    #[test]
    fn load_skill_rejects_path_traversal_in_bundle() {
        let tmp = TempDir::new().unwrap();
        // The Python source is implicitly trusting; the Rust port adds containment.
        let bundle = binary_bundle("../escape.txt", vec![1, 2, 3]);
        let (rt, _, _) = rt_with(tmp.path().to_path_buf(), vec![("evil", bundle)]);
        let outcome = rt.load_skill("evil");
        match outcome {
            LoadOutcome::Failure(msg) => assert!(msg.contains("unsafe relative path")),
            LoadOutcome::Success => panic!("traversal should be rejected"),
        }
        // The escape target must not exist.
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[test]
    fn unload_skill_is_idempotent_and_office_origin_gated() {
        let tmp = TempDir::new().unwrap();
        let bundle = bundle_with(&[("SKILL.md", "x")]);
        let (rt, _, _) = rt_with(tmp.path().to_path_buf(), vec![("foo", bundle)]);

        // Unload before load = no-op (foo isn't office-origin yet).
        rt.unload_skill("foo");
        assert!(!rt.is_office_origin("foo"));

        // Load, then unload should remove the dir + drop the slug.
        assert!(rt.load_skill("foo").is_success());
        let skill_dir = tmp.path().join("foo");
        assert!(skill_dir.exists());
        rt.unload_skill("foo");
        assert!(!skill_dir.exists());
        assert!(!rt.is_office_origin("foo"));

        // Second unload is a no-op.
        rt.unload_skill("foo");
        assert!(!rt.is_office_origin("foo"));

        // A user-authored dir with the same slug (NOT tracked by the
        // runtime) must survive unload — matches the Python guard.
        let user_skill = tmp.path().join("user-skill");
        fs::create_dir_all(&user_skill).unwrap();
        fs::write(user_skill.join("SKILL.md"), "user").unwrap();
        rt.unload_skill("user-skill");
        assert!(user_skill.exists(), "user-authored skill must survive");
    }

    #[test]
    fn notify_skill_authored_marks_origin_and_emits() {
        let tmp = TempDir::new().unwrap();
        let (rt, _, emitter) = rt_with(tmp.path().to_path_buf(), vec![]);

        assert!(!rt.is_office_origin("self-made"));
        rt.notify_skill_authored("self-made");
        assert!(rt.is_office_origin("self-made"));

        let events = emitter.0.lock().unwrap();
        assert_eq!(events.as_slice(), &["self-made".to_string()]);

        // Subsequent unload should now succeed because the slug was
        // promoted to office-origin via the authored notification.
        let dir = tmp.path().join("self-made");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "x").unwrap();
        rt.unload_skill("self-made");
        assert!(!dir.exists());
    }

    #[test]
    fn is_safe_relative_accepts_nested_paths_and_rejects_escapes() {
        assert!(is_safe_relative("SKILL.md"));
        assert!(is_safe_relative("a/b/c.txt"));
        assert!(is_safe_relative("./SKILL.md"));
        assert!(!is_safe_relative(""));
        assert!(!is_safe_relative("../escape"));
        assert!(!is_safe_relative("a/../b"));
        // Absolute paths blocked on both unix and windows.
        #[cfg(unix)]
        assert!(!is_safe_relative("/etc/passwd"));
    }
}
