//! v0.8.1 U11 — originally a building block reserved for the future sub-agent
//! ACL pre-filter wave. That reservation is stale, and this note said so for
//! longer than it was true: the module is wired in production on both halves.
//! `wcore_agent::orchestration` calls `evaluate()` in the dispatch path
//! (`orchestration/mod.rs:546`), and #693 made the store durable —
//! `wcore_cli::tui::engine_bridge::persist_always_allow` writes it when the
//! user grants "always allow", and `wcore_cli::main` replays it at startup
//! through `restore_always_allows`. Because that file IS the agent's standing
//! authority, `wcore_config::command_floor` refuses any shell command that
//! names it. See node_executor::dispatch_once for the removed pre-filter site
//! the original note referred to.
//!
//! ## Original v0.7.0 Task 3.C.3 spec
//!
//! Records user decisions about whether a given tool invocation should
//! be allowed. The runtime calls [`LearnedPolicy::evaluate`] for each
//! tool dispatch; if no rule matches, the caller (the TUI in 3.C.4)
//! prompts the user and feeds the answer back via [`LearnedPolicy::record`].
//! Rules can be `AllowOnce` / `AllowAlways` / `DenyOnce` / `DenyAlways`;
//! the *-Once variants are evaluated then dropped.
//!
//! Pattern matching is glob-like: an arg_pattern of `git *` matches any
//! invocation whose joined argv begins with `git `; `*` matches anything;
//! a missing arg_pattern matches the tool with no argument matching at
//! all (most permissive). Specific patterns beat wildcard patterns.
//!
//! Persistence is TOML at `~/.wayland/permissions.toml` (path is
//! injectable for tests).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LearnedDecision {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalResult {
    /// Persisted rule matched. The caller should honour `allow` and the
    /// matched pattern is returned for audit.
    Match { allow: bool, pattern: String },
    /// No rule found. The caller should prompt the user and feed the
    /// answer back via `record`.
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRule {
    tool: String,
    /// `None` matches any invocation of the tool; `Some("*")` is the
    /// explicit wildcard; `Some("git *")` matches argv starting with
    /// `git `.
    arg_pattern: Option<String>,
    decision: LearnedDecision,
    /// The workspace this rule was granted in, as a canonical absolute path.
    /// `None` means "every workspace".
    ///
    /// #693 — this file is USER-global (`~/.wayland/permissions.toml`), so
    /// without this field a grant made by pressing `[a]` at an approval
    /// prompt in one checkout would authorise that tool in every other
    /// checkout the user ever opens. [`LearnedPolicy::record_in`] stamps the
    /// workspace and [`LearnedPolicy::snapshot_in`] filters on it, so an
    /// interactive grant carries exactly the authority its prompt names.
    ///
    /// `None` is reachable only from a file written before this field
    /// existed or hand-edited by the operator. Both are deliberate
    /// statements of "everywhere", so they are honoured rather than dropped
    /// — nothing in this codebase writes an unscoped rule.
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoredPolicy {
    #[serde(default)]
    rules: Vec<StoredRule>,
}

#[derive(Debug, thiserror::Error)]
pub enum LearningError {
    #[error("could not resolve user permissions directory (HOME unset?)")]
    NoHomeDir,
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to lock {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("gave up after {waited:?} waiting for the lock on {path}")]
    LockTimeout { path: PathBuf, waited: Duration },
    #[error("failed to parse permissions TOML: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("failed to serialise permissions TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Default)]
pub struct LearnedPolicy {
    rules: Vec<StoredRule>,
}

impl LearnedPolicy {
    /// Empty policy (no rules).
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the default on-disk path (`~/.wayland/permissions.toml`).
    pub fn default_path() -> Result<PathBuf, LearningError> {
        dirs::home_dir()
            .map(|h| h.join(".wayland").join("permissions.toml"))
            .ok_or(LearningError::NoHomeDir)
    }

    /// The workspace key for the process's current directory: its canonical
    /// absolute path, as a string. `None` when the directory cannot be read
    /// (it was deleted out from under the process).
    ///
    /// #693 — canonical so that two paths reaching the same directory (a
    /// symlinked checkout, a `..` hop, a trailing `.`) produce ONE key
    /// instead of several that never match each other on restore. The write
    /// and restore sides both call this, so they cannot disagree.
    pub fn current_workspace() -> Option<String> {
        Some(Self::workspace_key(&std::env::current_dir().ok()?))
    }

    /// The workspace key for an explicitly chosen directory.
    ///
    /// #693 — the process CWD is only the right identity when the session was
    /// not pointed somewhere else. `--project-dir` moves the workspace the
    /// session operates against (its config, its skills, its MCP servers, and
    /// its entry in the workspace trust store) WITHOUT moving the CWD, so a
    /// grant keyed off the CWD alone is shared by two sessions aimed at two
    /// different projects. The CLI resolves `--project-dir` or the CWD once
    /// and passes the answer here, so the learned policy and the trust store
    /// agree on what "this workspace" means.
    pub fn workspace_key(dir: &Path) -> String {
        // A canonicalize failure (a permission-denied ancestor) is not fatal:
        // the raw path is still a stable key for this machine, it just will
        // not unify with an alias of the same directory.
        let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        canonical.to_string_lossy().into_owned()
    }

    /// Load from a specific path. Missing file = empty policy (not an error).
    pub fn load_from(path: &Path) -> Result<Self, LearningError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(e) => {
                return Err(LearningError::Read {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };
        let stored: StoredPolicy = toml::from_str(&raw)?;
        Ok(Self {
            rules: stored.rules,
        })
    }

    /// Persist to a specific path (creates parent dir if absent).
    pub fn save_to(&self, path: &Path) -> Result<(), LearningError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LearningError::Write {
                path: path.to_path_buf(),
                source: e,
            })?;
        }
        let stored = StoredPolicy {
            rules: self.rules.clone(),
        };
        let toml = toml::to_string_pretty(&stored)?;
        // #693 — `std::fs::write` truncates in place, so a crash (or a full
        // disk) mid-write publishes a half-file under the live name and the
        // next launch reads a policy that parses to something the operator
        // never wrote. `atomic_write` is the workspace's existing
        // tempfile+fsync+rename helper; the rename target is this file, never
        // the sidecar lock, so a concurrent lock holder cannot make the
        // rename fail with a sharing violation on Windows.
        wcore_config::atomic_write(path, toml.as_bytes()).map_err(|e| LearningError::Write {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Run `mutate` against the policy stored at `path` while holding that
    /// file's exclusive cross-process advisory lock, then publish the result
    /// atomically.
    ///
    /// #693 — a bare load / mutate / save is a read-modify-write with no
    /// mutual exclusion: two sessions that grant a tool at the same time both
    /// read the same "before" file and the later `save_to` overwrites the
    /// earlier one's rule, so one user decision is silently lost. The lock
    /// must span the READ as well as the write — publishing atomically only
    /// guarantees no torn file, not that the file being written was derived
    /// from the current one.
    ///
    /// ## What releases the lock, and why the wait is bounded
    ///
    /// `fd_lock` takes a `flock(2)`-style lock, which belongs to the open file
    /// description and is therefore DUPLICATED into a forked child rather than
    /// re-acquired by it. That is exactly how the session-journal lock in this
    /// project came to be held by a process nobody was looking at.
    ///
    /// What prevents it here is neither of the two properties this comment
    /// used to name. "The critical section spawns nothing" is irrelevant: the
    /// hazard is a `fork` on ANOTHER thread, which duplicates every descriptor
    /// this one has open — that is the shape
    /// `wcore-agent/tests/snapshot_lock_probe.rs` models, naming the Bash
    /// tool, `git status`, the spawner and the forge. And `O_CLOEXEC` (which
    /// Rust does set on this file) is consulted only by `exec`, so it does
    /// nothing for a `fork` WITHOUT one. The actual protection is that
    /// `fd_lock`'s write guard issues an explicit `FlockOperation::Unlock` in
    /// its `Drop`: the unlock is SYMMETRIC with the lock, so it lands the
    /// moment the guard dies — on an early return or a panic inside `mutate`
    /// too — instead of waiting for the last duplicate of the descriptor to be
    /// closed.
    ///
    /// One shape survives that, and it is why the wait below is bounded: if
    /// this process is killed outright while a forked child still holds a
    /// duplicate of the descriptor, no `Drop` runs, the kernel keeps the flock
    /// until that child exits, and `/proc/locks` attributes it to the dead
    /// parent. A blocking `flock` behind such a holder waits forever, and a
    /// blocking `flock` behind a merely SUSPENDED peer (Ctrl-Z inside its
    /// critical section) was measured at 17.9 s. The only caller runs INLINE
    /// on the TUI's synchronous event thread, so an unbounded wait there is a
    /// frozen UI with no message. `try_write` is polled to [`LOCK_WAIT`] and
    /// then gives up with [`LearningError::LockTimeout`], which the TUI already
    /// renders as "this grant applies to this session only". Losing one grant
    /// with an explanation beats wedging the session without one.
    ///
    /// The `Drop`-unlock claim above is read from the vendored source
    /// (`fd-lock-4.0.4/src/sys/unix/write_guard.rs`), NOT pinned by a test,
    /// and a test in this crate cannot pin it: `lock` is a local, so the
    /// `File` is closed when this function returns and the flock would be
    /// released by that close even if the guard were leaked. Inside one
    /// process the two mechanisms are indistinguishable through the public
    /// API. Telling them apart needs `/proc/locks` plus a fork-without-exec
    /// child outliving its parent — the shape
    /// `wcore-agent/tests/snapshot_lock_probe.rs` already builds. Do not add
    /// a `mem::forget`-the-guard test here; it passes either way.
    pub fn update_at(path: &Path, mutate: impl FnOnce(&mut Self)) -> Result<(), LearningError> {
        let mut lock = open_policy_lock(path)?;
        // `fd_lock`'s guard borrows the `RwLock` mutably, so the retry loop
        // cannot return the guard across a function boundary under NLL — the
        // same closure shape `wcore-config`'s credential marker lock and
        // `wcore-budget`'s daily ledger already use.
        let deadline = Instant::now() + LOCK_WAIT;
        let _guard = loop {
            match lock.try_write() {
                Ok(guard) => break guard,
                Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
                // `fd_lock` normalises contention to `WouldBlock` on BOTH
                // platforms: `EWOULDBLOCK` from `flock(LOCK_NB)` on unix and
                // `ERROR_LOCK_VIOLATION` from `LOCKFILE_FAIL_IMMEDIATELY` on
                // Windows. Anything else is a real open/lock failure.
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(LearningError::LockTimeout {
                            path: policy_lock_path(path),
                            waited: LOCK_WAIT,
                        });
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(source) => {
                    return Err(LearningError::Lock {
                        path: policy_lock_path(path),
                        source,
                    });
                }
            }
        };
        let mut policy = Self::load_from(path)?;
        mutate(&mut policy);
        policy.save_to(path)
    }

    /// Evaluate whether `tool` invoked with `argv` is currently allowed.
    /// `argv` is the joined argument list (already shell-quoted by the caller).
    ///
    /// Specificity rules: an exact pattern match beats `*` beats no
    /// pattern. Within equal specificity, the first matching rule wins
    /// (preserving insertion order). `*-Once` rules are NOT consumed
    /// here — call `record_once_consumed` after honouring an `Ask` →
    /// user-chose-AllowOnce / DenyOnce path so the rule disappears.
    pub fn evaluate(&self, tool: &str, argv: &str) -> EvalResult {
        let mut best: Option<(usize, &StoredRule)> = None;
        for r in &self.rules {
            if r.tool != tool {
                continue;
            }
            let specificity = match r.arg_pattern.as_deref() {
                None => 0,
                Some("*") => 1,
                Some(pat) => {
                    if !pattern_matches(pat, argv) {
                        continue;
                    }
                    2
                }
            };
            match best {
                None => best = Some((specificity, r)),
                Some((s, _)) if specificity > s => best = Some((specificity, r)),
                _ => {}
            }
        }
        match best {
            Some((_, r)) => {
                let allow = matches!(
                    r.decision,
                    LearnedDecision::AllowOnce | LearnedDecision::AllowAlways
                );
                EvalResult::Match {
                    allow,
                    pattern: r.arg_pattern.clone().unwrap_or_else(|| "*".to_string()),
                }
            }
            None => EvalResult::Ask,
        }
    }

    /// Record the user's decision for a `tool` + optional `arg_pattern`.
    /// Pass `None` for arg_pattern to match the tool with any args.
    /// Replaces any existing rule with the same (tool, arg_pattern) key.
    pub fn record(
        &mut self,
        tool: impl Into<String>,
        arg_pattern: Option<String>,
        decision: LearnedDecision,
    ) {
        self.record_rule(tool, arg_pattern, decision, None);
    }

    /// [`record`](Self::record), scoped to the `workspace` the decision was
    /// made in.
    ///
    /// #693 — this is the shape an interactive grant takes. Rules are keyed
    /// by (tool, arg_pattern, workspace), so granting a tool in one checkout
    /// neither replaces nor widens the same tool's rule in another.
    pub fn record_in(
        &mut self,
        tool: impl Into<String>,
        arg_pattern: Option<String>,
        decision: LearnedDecision,
        workspace: &str,
    ) {
        self.record_rule(tool, arg_pattern, decision, Some(workspace.to_string()));
    }

    fn record_rule(
        &mut self,
        tool: impl Into<String>,
        arg_pattern: Option<String>,
        decision: LearnedDecision,
        workspace: Option<String>,
    ) {
        let tool = tool.into();
        self.rules.retain(|r| {
            !(r.tool == tool && r.arg_pattern == arg_pattern && r.workspace == workspace)
        });
        self.rules.push(StoredRule {
            tool,
            arg_pattern,
            decision,
            workspace,
        });
    }

    /// After an Ask → user chose AllowOnce / DenyOnce path, the runtime
    /// must record() the *Once decision (so the next evaluate() in this
    /// session also returns Match), then call this to clear it once the
    /// invocation has happened.
    pub fn record_once_consumed(&mut self, tool: &str, arg_pattern: Option<&str>) {
        self.rules.retain(|r| {
            !(r.tool == tool
                && r.arg_pattern.as_deref() == arg_pattern
                && matches!(
                    r.decision,
                    LearnedDecision::AllowOnce | LearnedDecision::DenyOnce
                ))
        });
    }

    /// All persisted rules, grouped by tool, for inspection / TUI listing.
    pub fn snapshot(&self) -> HashMap<String, Vec<(Option<String>, LearnedDecision)>> {
        let mut map: HashMap<String, Vec<(Option<String>, LearnedDecision)>> = HashMap::new();
        for r in &self.rules {
            map.entry(r.tool.clone())
                .or_default()
                .push((r.arg_pattern.clone(), r.decision.clone()));
        }
        map
    }

    /// [`snapshot`](Self::snapshot), narrowed to the rules that apply in
    /// `workspace`: those granted in it, plus the unscoped rules that apply
    /// everywhere.
    ///
    /// #693 — the restore path reads through here so a grant made in one
    /// checkout cannot be replayed as authority in another.
    pub fn snapshot_in(
        &self,
        workspace: &str,
    ) -> HashMap<String, Vec<(Option<String>, LearnedDecision)>> {
        let mut map: HashMap<String, Vec<(Option<String>, LearnedDecision)>> = HashMap::new();
        for r in &self.rules {
            if r.workspace.as_deref().is_some_and(|ws| ws != workspace) {
                continue;
            }
            map.entry(r.tool.clone())
                .or_default()
                .push((r.arg_pattern.clone(), r.decision.clone()));
        }
        map
    }

    /// Total rule count (mostly useful for tests).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// How long [`LearnedPolicy::update_at`] waits for the policy lock before
/// giving up. This is a UI freeze budget, not an I/O budget: the caller runs
/// inline on the TUI's synchronous event thread. An honest concurrent grant
/// holds the lock for a small file read, an in-memory edit and an atomic
/// write — sub-millisecond — so a legitimate contender always wins this
/// comfortably, while a wedged or orphaned holder cannot hang the session.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// Poll interval while waiting. `flock` has no timed variant, so a bounded
/// wait has to be a `try_write` poll.
const LOCK_POLL: Duration = Duration::from_millis(10);

/// The sidecar advisory-lock file for the policy at `path`.
///
/// A SEPARATE file on purpose: [`LearnedPolicy::save_to`] publishes by
/// renaming over `path`, and on Windows a rename over a file another process
/// holds open fails with a sharing violation. Locking a sibling leaves the
/// rename target closed by every participant.
fn policy_lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}

fn open_policy_lock(path: &Path) -> Result<fd_lock::RwLock<std::fs::File>, LearningError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| LearningError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let lock_path = policy_lock_path(path);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        // Never truncate: the lock file's CONTENT is irrelevant, but
        // truncating it is a write that a concurrent holder would see.
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| LearningError::Lock {
            path: lock_path,
            source,
        })?;
    Ok(fd_lock::RwLock::new(file))
}

fn pattern_matches(pattern: &str, argv: &str) -> bool {
    // Two simple cases: a literal pattern (no `*`) matches argv as a prefix
    // exactly; a trailing-`*` pattern matches argv where the non-`*` part
    // is a prefix. Leading `*` and middle `*` are intentionally
    // unsupported — keep the language tight so users can predict matches.
    if pattern == argv {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return argv.starts_with(prefix);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_policy_asks() {
        let p = LearnedPolicy::new();
        assert_eq!(p.evaluate("Bash", "git status"), EvalResult::Ask);
    }

    #[test]
    fn allow_always_for_tool_with_no_pattern() {
        let mut p = LearnedPolicy::new();
        p.record("Read", None, LearnedDecision::AllowAlways);
        match p.evaluate("Read", "src/main.rs") {
            EvalResult::Match { allow: true, .. } => {}
            other => panic!("expected allow-Match, got {other:?}"),
        }
    }

    #[test]
    fn wildcard_pattern_matches_anything() {
        let mut p = LearnedPolicy::new();
        p.record("Bash", Some("*".to_string()), LearnedDecision::DenyAlways);
        match p.evaluate("Bash", "rm -rf /") {
            EvalResult::Match {
                allow: false,
                pattern,
            } => assert_eq!(pattern, "*"),
            other => panic!("expected deny-Match, got {other:?}"),
        }
    }

    #[test]
    fn specific_beats_wildcard() {
        let mut p = LearnedPolicy::new();
        p.record("Bash", Some("*".to_string()), LearnedDecision::DenyAlways);
        p.record(
            "Bash",
            Some("git *".to_string()),
            LearnedDecision::AllowAlways,
        );
        match p.evaluate("Bash", "git status") {
            EvalResult::Match {
                allow: true,
                pattern,
            } => assert_eq!(pattern, "git *"),
            other => panic!("expected allow-Match for git, got {other:?}"),
        }
        // non-git Bash invocations should still hit the wildcard deny
        match p.evaluate("Bash", "rm -rf /") {
            EvalResult::Match {
                allow: false,
                pattern,
            } => assert_eq!(pattern, "*"),
            other => panic!("expected deny-Match for rm, got {other:?}"),
        }
    }

    #[test]
    fn pattern_matches_prefix_only() {
        let mut p = LearnedPolicy::new();
        p.record(
            "Bash",
            Some("git *".to_string()),
            LearnedDecision::AllowAlways,
        );
        assert!(matches!(
            p.evaluate("Bash", "git status"),
            EvalResult::Match { allow: true, .. }
        ));
        assert_eq!(p.evaluate("Bash", "kubectl get pods"), EvalResult::Ask);
    }

    #[test]
    fn exact_literal_match() {
        let mut p = LearnedPolicy::new();
        p.record(
            "Bash",
            Some("ls -la".to_string()),
            LearnedDecision::AllowAlways,
        );
        assert!(matches!(
            p.evaluate("Bash", "ls -la"),
            EvalResult::Match { allow: true, .. }
        ));
        assert_eq!(p.evaluate("Bash", "ls"), EvalResult::Ask);
    }

    #[test]
    fn record_overwrites_same_key() {
        let mut p = LearnedPolicy::new();
        p.record("Bash", Some("*".to_string()), LearnedDecision::AllowAlways);
        p.record("Bash", Some("*".to_string()), LearnedDecision::DenyAlways);
        assert_eq!(p.len(), 1);
        match p.evaluate("Bash", "anything") {
            EvalResult::Match { allow: false, .. } => {}
            other => panic!("expected deny after overwrite, got {other:?}"),
        }
    }

    #[test]
    fn once_decisions_clear_after_consume() {
        let mut p = LearnedPolicy::new();
        p.record("Bash", Some("*".to_string()), LearnedDecision::AllowOnce);
        assert!(matches!(
            p.evaluate("Bash", "git status"),
            EvalResult::Match { allow: true, .. }
        ));
        p.record_once_consumed("Bash", Some("*"));
        assert_eq!(p.evaluate("Bash", "git status"), EvalResult::Ask);
    }

    #[test]
    fn always_decisions_survive_consume() {
        let mut p = LearnedPolicy::new();
        p.record("Bash", Some("*".to_string()), LearnedDecision::AllowAlways);
        p.record_once_consumed("Bash", Some("*"));
        assert!(matches!(
            p.evaluate("Bash", "git status"),
            EvalResult::Match { allow: true, .. }
        ));
    }

    #[test]
    fn round_trips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("permissions.toml");

        let mut p = LearnedPolicy::new();
        p.record("Read", None, LearnedDecision::AllowAlways);
        p.record(
            "Bash",
            Some("git *".to_string()),
            LearnedDecision::AllowAlways,
        );
        p.record("Bash", Some("*".to_string()), LearnedDecision::DenyAlways);
        p.save_to(&path).unwrap();

        let loaded = LearnedPolicy::load_from(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(matches!(
            loaded.evaluate("Bash", "git push"),
            EvalResult::Match { allow: true, .. }
        ));
        assert!(matches!(
            loaded.evaluate("Bash", "rm -rf /"),
            EvalResult::Match { allow: false, .. }
        ));
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist.toml");
        let p = LearnedPolicy::load_from(&missing).expect("missing file = empty");
        assert!(p.is_empty());
    }

    #[test]
    fn snapshot_groups_by_tool() {
        let mut p = LearnedPolicy::new();
        p.record("Read", None, LearnedDecision::AllowAlways);
        p.record("Bash", Some("*".to_string()), LearnedDecision::DenyAlways);
        p.record(
            "Bash",
            Some("git *".to_string()),
            LearnedDecision::AllowAlways,
        );
        let s = p.snapshot();
        assert_eq!(s.get("Read").unwrap().len(), 1);
        assert_eq!(s.get("Bash").unwrap().len(), 2);
    }
}
