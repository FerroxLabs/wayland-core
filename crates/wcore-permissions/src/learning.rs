//! v0.8.1 U11 — building block reserved for the future sub-agent ACL
//! pre-filter wave. Today no production caller constructs a LearnedPolicy
//! or runs its evaluate() in the dispatch path; the module is kept
//! self-contained so a future wave can wire it without re-inventing the
//! types. See node_executor::dispatch_once for the removed pre-filter
//! site.
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
        std::fs::write(path, toml).map_err(|e| LearningError::Write {
            path: path.to_path_buf(),
            source: e,
        })
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
        let tool = tool.into();
        self.rules
            .retain(|r| !(r.tool == tool && r.arg_pattern == arg_pattern));
        self.rules.push(StoredRule {
            tool,
            arg_pattern,
            decision,
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

    /// Total rule count (mostly useful for tests).
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
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
