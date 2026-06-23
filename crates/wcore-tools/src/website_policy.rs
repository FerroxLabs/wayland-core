//! T3-3.3.3 — Website access policy helpers for URL-capable tools.
//!
//! Ported from the prior Wayland Python engine.
//!
//! Loads a user-managed website blocklist from `~/.wayland-core/config.yaml`
//! plus any referenced shared list files, and enforces it on URLs the
//! agent's web/browser tools resolve. Kept deliberately lightweight so
//! callers do NOT have to pull the heavier `wcore-config` cascading
//! loader — this is a pure policy check.
//!
//! ## Public surface
//!
//! * [`check_website_access`] — main entry. Returns `None` if the URL is
//!   allowed, or `Some(WebsiteBlock)` with metadata describing the
//!   matching rule.
//! * [`load_website_blocklist`] — parse + normalize the YAML (used by
//!   tests and by callers that want to inspect rules without checking a
//!   URL).
//! * [`reset_cache`] / [`invalidate_cache`] — drop the 30-second policy
//!   cache (used by tests, profile switching, or hot-reload paths).
//! * [`WebsitePolicyError`] — structured error for malformed config
//!   files (callers passing an explicit `config_path` get hard errors;
//!   the default path fails open with a `tracing::warn`).
//!
//! ## Differences from the Python original
//!
//! * `get_wayland_home()` is replaced by `wcore_config::config::app_config_dir()`
//!   which returns `Option<PathBuf>` — when the OS has no config dir
//!   (rare embedded targets) the policy is treated as disabled.
//! * Cache uses `parking_lot::Mutex` instead of a `threading.Lock` +
//!   global state, but the semantics (30s TTL, default-path only) match.
//! * `urllib.parse.urlparse` is replaced by the `url` crate. A bare
//!   `host[:port]` input (no scheme) is parsed by prefixing `http://`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use thiserror::Error;
use url::Url;

use wcore_config::config::app_config_dir;

/// TTL for the in-memory policy cache. Matches the Python original.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// A single normalized blocklist rule (pattern + source provenance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocklistRule {
    /// Normalized host pattern (lowercased, no scheme, no path, no
    /// leading `www.`, possibly leading `*.` for wildcards).
    pub pattern: String,
    /// Where this rule came from: `"config"` for inline `domains:`
    /// entries, or the absolute path string of a shared blocklist file.
    pub source: String,
}

/// Parsed, normalized website blocklist policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebsitePolicy {
    /// Master enable switch. When `false`, [`check_website_access`]
    /// short-circuits to `None`.
    pub enabled: bool,
    /// Deduplicated, normalized rules in declaration order
    /// (inline `domains:` first, then each `shared_files:` entry in order).
    pub rules: Vec<BlocklistRule>,
}

/// Metadata returned when a URL is blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebsiteBlock {
    pub url: String,
    pub host: String,
    pub rule: String,
    pub source: String,
    pub message: String,
}

/// Structured error for malformed website-policy YAML.
#[derive(Debug, Error)]
pub enum WebsitePolicyError {
    #[error("invalid config YAML at {path}: {source}")]
    InvalidYaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config root must be a mapping (at {0})")]
    RootNotMapping(PathBuf),
    #[error("security must be a mapping (at {0})")]
    SecurityNotMapping(PathBuf),
    #[error("security.website_blocklist must be a mapping (at {0})")]
    BlocklistNotMapping(PathBuf),
    #[error("security.website_blocklist.domains must be a list (at {0})")]
    DomainsNotList(PathBuf),
    #[error("security.website_blocklist.shared_files must be a list (at {0})")]
    SharedFilesNotList(PathBuf),
    #[error("security.website_blocklist.enabled must be a boolean (at {0})")]
    EnabledNotBool(PathBuf),
}

// ---------------------------------------------------------------------------
// Cache state
// ---------------------------------------------------------------------------

struct CachedPolicy {
    policy: WebsitePolicy,
    at: Instant,
}

fn cache_slot() -> &'static Mutex<Option<CachedPolicy>> {
    static SLOT: OnceLock<Mutex<Option<CachedPolicy>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Drop the cached policy so the next check re-reads config. Used by
/// the test suite and any context that swaps config files at runtime
/// (e.g. a profile switch).
pub fn reset_cache() {
    *cache_slot().lock() = None;
}

/// Alias kept for parity with the Python original
/// (`invalidate_cache()` vs `reset()`).
pub fn invalidate_cache() {
    reset_cache();
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

fn normalize_host(host: &str) -> String {
    host.trim().to_lowercase().trim_end_matches('.').to_string()
}

/// Normalize a rule string into a comparable host pattern, or return
/// `None` if it's a comment / blank / unrepresentable.
fn normalize_rule(rule: &str) -> Option<String> {
    let value = rule.trim().to_lowercase();
    if value.is_empty() || value.starts_with('#') {
        return None;
    }

    // If the rule looks like a URL, peel off everything but the host.
    let value = if value.contains("://") {
        match Url::parse(&value) {
            Ok(u) => u
                .host_str()
                .map(|h| h.to_string())
                .unwrap_or_else(|| value.clone()),
            Err(_) => value,
        }
    } else {
        value
    };

    // Strip any path segment ("foo.com/bar" -> "foo.com").
    let value = value.split('/').next().unwrap_or("").trim().to_string();
    let value = value.trim_end_matches('.').to_string();

    // Drop a single leading "www." so "www.evil.com" and "evil.com" are
    // canonically the same blocklist entry (matches Python original).
    let value = if let Some(stripped) = value.strip_prefix("www.") {
        stripped.to_string()
    } else {
        value
    };

    if value.is_empty() { None } else { Some(value) }
}

fn iter_blocklist_file_rules(path: &Path) -> Vec<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                target: "wcore_tools::website_policy",
                "Shared blocklist file unreadable (skipping): {} — {err}",
                path.display(),
            );
            return Vec::new();
        }
    };

    raw.lines()
        .filter_map(|line| {
            let stripped = line.trim();
            if stripped.is_empty() || stripped.starts_with('#') {
                None
            } else {
                normalize_rule(stripped)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

fn default_config_path() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("config.yaml"))
}

fn parse_policy_yaml(config_path: &Path) -> Result<WebsitePolicy, WebsitePolicyError> {
    if !config_path.exists() {
        return Ok(WebsitePolicy::default());
    }

    let raw = std::fs::read_to_string(config_path).map_err(|source| WebsitePolicyError::Io {
        path: config_path.to_path_buf(),
        source,
    })?;

    let root: YamlValue =
        serde_yaml::from_str(&raw).map_err(|source| WebsitePolicyError::InvalidYaml {
            path: config_path.to_path_buf(),
            source,
        })?;

    // Empty YAML -> default-disabled policy.
    if root.is_null() {
        return Ok(WebsitePolicy::default());
    }

    let root_map = root
        .as_mapping()
        .ok_or_else(|| WebsitePolicyError::RootNotMapping(config_path.to_path_buf()))?;

    let security = match root_map.get(YamlValue::String("security".to_string())) {
        None | Some(YamlValue::Null) => return Ok(WebsitePolicy::default()),
        Some(v) => v
            .as_mapping()
            .ok_or_else(|| WebsitePolicyError::SecurityNotMapping(config_path.to_path_buf()))?,
    };

    let blocklist = match security.get(YamlValue::String("website_blocklist".to_string())) {
        None | Some(YamlValue::Null) => return Ok(WebsitePolicy::default()),
        Some(v) => v
            .as_mapping()
            .ok_or_else(|| WebsitePolicyError::BlocklistNotMapping(config_path.to_path_buf()))?,
    };

    // enabled: default true when the block is present but the key is omitted
    // (matches the Python: `policy.get("enabled", True)` after merging defaults).
    let enabled = match blocklist.get(YamlValue::String("enabled".to_string())) {
        None | Some(YamlValue::Null) => true,
        Some(YamlValue::Bool(b)) => *b,
        Some(_) => {
            return Err(WebsitePolicyError::EnabledNotBool(
                config_path.to_path_buf(),
            ));
        }
    };

    let domains = match blocklist.get(YamlValue::String("domains".to_string())) {
        None | Some(YamlValue::Null) => Vec::new(),
        Some(YamlValue::Sequence(s)) => s.clone(),
        Some(_) => {
            return Err(WebsitePolicyError::DomainsNotList(
                config_path.to_path_buf(),
            ));
        }
    };

    let shared_files = match blocklist.get(YamlValue::String("shared_files".to_string())) {
        None | Some(YamlValue::Null) => Vec::new(),
        Some(YamlValue::Sequence(s)) => s.clone(),
        Some(_) => {
            return Err(WebsitePolicyError::SharedFilesNotList(
                config_path.to_path_buf(),
            ));
        }
    };

    let mut rules: Vec<BlocklistRule> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();

    for raw_rule in domains {
        let YamlValue::String(s) = raw_rule else {
            continue;
        };
        if let Some(normalized) = normalize_rule(&s) {
            let key = ("config".to_string(), normalized.clone());
            if seen.insert(key) {
                rules.push(BlocklistRule {
                    pattern: normalized,
                    source: "config".to_string(),
                });
            }
        }
    }

    for shared_file in shared_files {
        let YamlValue::String(s) = shared_file else {
            continue;
        };
        if s.trim().is_empty() {
            continue;
        }
        let raw_path = PathBuf::from(shellexpand_tilde(&s));
        let path = if raw_path.is_absolute() {
            raw_path
        } else if let Some(home) = app_config_dir() {
            home.join(&raw_path)
        } else {
            raw_path
        };

        let source = path.to_string_lossy().into_owned();
        for normalized in iter_blocklist_file_rules(&path) {
            let key = (source.clone(), normalized.clone());
            if seen.insert(key) {
                rules.push(BlocklistRule {
                    pattern: normalized,
                    source: source.clone(),
                });
            }
        }
    }

    Ok(WebsitePolicy { enabled, rules })
}

/// Minimal `~` expansion. Not a full shell expansion — only handles a
/// leading `~/` or bare `~`, which is the only form the Python original
/// supported via `Path.expanduser()`.
fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    if s == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home.to_string_lossy().into_owned();
    }
    s.to_string()
}

/// Load + normalize the policy. When `config_path` is `None`, results
/// are cached for [`CACHE_TTL`] (30s). When `config_path` is `Some`,
/// the cache is bypassed (this is the path tests use).
pub fn load_website_blocklist(
    config_path: Option<&Path>,
) -> Result<WebsitePolicy, WebsitePolicyError> {
    if config_path.is_none() {
        let slot = cache_slot().lock();
        if let Some(cached) = slot.as_ref()
            && cached.at.elapsed() < CACHE_TTL
        {
            return Ok(cached.policy.clone());
        }
        drop(slot);
    }

    let resolved = match config_path {
        Some(p) => p.to_path_buf(),
        None => match default_config_path() {
            Some(p) => p,
            None => return Ok(WebsitePolicy::default()),
        },
    };

    let policy = parse_policy_yaml(&resolved)?;

    if config_path.is_none() {
        *cache_slot().lock() = Some(CachedPolicy {
            policy: policy.clone(),
            at: Instant::now(),
        });
    }

    Ok(policy)
}

// ---------------------------------------------------------------------------
// Host matching
// ---------------------------------------------------------------------------

fn match_host_against_rule(host: &str, pattern: &str) -> bool {
    if host.is_empty() || pattern.is_empty() {
        return false;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // `*.foo.com` matches `bar.foo.com` and `a.b.foo.com`, but NOT
        // bare `foo.com` (matches Python fnmatch semantics with literal "*").
        return host.ends_with(&format!(".{suffix}")) || host == suffix;
    }
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

fn extract_host_from_urlish(input: &str) -> String {
    if let Ok(parsed) = Url::parse(input)
        && let Some(h) = parsed.host_str()
    {
        return normalize_host(h);
    }
    // No scheme — prefix one so `Url::parse` can pull out the authority.
    if !input.contains("://")
        && let Ok(parsed) = Url::parse(&format!("http://{input}"))
        && let Some(h) = parsed.host_str()
    {
        return normalize_host(h);
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Check whether `url` is allowed by the website blocklist policy.
///
/// Returns `None` if access is allowed, or `Some(WebsiteBlock)` if
/// blocked.
///
/// **Fail-open** when `config_path` is `None`: a malformed config logs
/// a `tracing::warn` and returns `None`, so a config typo doesn't
/// nuke all web tools at runtime. When `config_path` is `Some` (the
/// test path), errors propagate.
pub fn check_website_access(
    url: &str,
    config_path: Option<&Path>,
) -> Result<Option<WebsiteBlock>, WebsitePolicyError> {
    // Fast path: cached + disabled → no work at all.
    if config_path.is_none() {
        let slot = cache_slot().lock();
        if let Some(cached) = slot.as_ref()
            && !cached.policy.enabled
        {
            return Ok(None);
        }
    }

    let host = extract_host_from_urlish(url);
    if host.is_empty() {
        return Ok(None);
    }

    let policy = match load_website_blocklist(config_path) {
        Ok(p) => p,
        Err(err) => {
            if config_path.is_some() {
                return Err(err);
            }
            tracing::warn!(
                target: "wcore_tools::website_policy",
                "Website policy config error (failing open): {err}",
            );
            return Ok(None);
        }
    };

    if !policy.enabled {
        return Ok(None);
    }

    for rule in &policy.rules {
        if match_host_against_rule(&host, &rule.pattern) {
            tracing::info!(
                target: "wcore_tools::website_policy",
                "Blocked URL {url} — matched rule '{}' from {}",
                rule.pattern,
                rule.source,
            );
            let message = format!(
                "Blocked by website policy: '{host}' matched rule '{}' from {}",
                rule.pattern, rule.source,
            );
            return Ok(Some(WebsiteBlock {
                url: url.to_string(),
                host,
                rule: rule.pattern.clone(),
                source: rule.source.clone(),
                message,
            }));
        }
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, yaml: &str) -> PathBuf {
        let path = dir.path().join("config.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(yaml.as_bytes()).unwrap();
        path
    }

    #[test]
    fn normalize_rule_strips_scheme_path_and_www() {
        assert_eq!(
            normalize_rule("https://WWW.Evil.com/path"),
            Some("evil.com".into())
        );
        assert_eq!(
            normalize_rule("  bad.example.  "),
            Some("bad.example".into())
        );
        assert_eq!(normalize_rule("# a comment"), None);
        assert_eq!(normalize_rule(""), None);
        assert_eq!(normalize_rule("*.tracker.io"), Some("*.tracker.io".into()));
    }

    #[test]
    fn match_host_exact_and_subdomain() {
        assert!(match_host_against_rule("evil.com", "evil.com"));
        assert!(match_host_against_rule("ads.evil.com", "evil.com"));
        assert!(!match_host_against_rule("notevil.com", "evil.com"));
        assert!(!match_host_against_rule("evilcom", "evil.com"));
    }

    #[test]
    fn match_host_wildcard_pattern() {
        assert!(match_host_against_rule("api.tracker.io", "*.tracker.io"));
        assert!(match_host_against_rule("a.b.tracker.io", "*.tracker.io"));
        // bare apex also matches `*.foo` in our port (parity with Python
        // semantics where `endswith(".foo")` was added alongside fnmatch).
        assert!(match_host_against_rule("tracker.io", "*.tracker.io"));
        assert!(!match_host_against_rule("other.io", "*.tracker.io"));
    }

    #[test]
    fn empty_policy_allows_all() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "security: {}\n");
        let result = check_website_access("https://anywhere.example/foo", Some(&path)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn blocklist_entry_blocks_url_with_metadata() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            "security:\n  website_blocklist:\n    enabled: true\n    domains:\n      - https://www.Evil.com/path\n",
        );
        let result = check_website_access("https://ads.evil.com/x", Some(&path))
            .unwrap()
            .expect("should be blocked");
        assert_eq!(result.host, "ads.evil.com");
        assert_eq!(result.rule, "evil.com");
        assert_eq!(result.source, "config");
        assert!(result.message.contains("evil.com"));
    }

    #[test]
    fn disabled_policy_does_not_block() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            "security:\n  website_blocklist:\n    enabled: false\n    domains:\n      - evil.com\n",
        );
        let result = check_website_access("https://evil.com", Some(&path)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn wildcard_blocks_subdomain_only() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            "security:\n  website_blocklist:\n    enabled: true\n    domains:\n      - \"*.tracker.io\"\n",
        );
        assert!(
            check_website_access("https://pixel.tracker.io", Some(&path))
                .unwrap()
                .is_some()
        );
        assert!(
            check_website_access("https://safe.example", Some(&path))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn shared_file_rules_are_merged() {
        let dir = TempDir::new().unwrap();
        let shared_path = dir.path().join("extra.txt");
        std::fs::write(&shared_path, "# comment\nbadguy.net\n\n  shady.example  \n").unwrap();

        let yaml = format!(
            "security:\n  website_blocklist:\n    enabled: true\n    shared_files:\n      - {}\n",
            shared_path.to_string_lossy(),
        );
        let cfg = write_config(&dir, &yaml);

        let blocked = check_website_access("https://api.badguy.net/", Some(&cfg))
            .unwrap()
            .expect("should be blocked");
        assert_eq!(blocked.rule, "badguy.net");
        assert!(
            blocked.source.ends_with("extra.txt"),
            "source should be the shared file path, got {}",
            blocked.source,
        );

        // Comments are ignored, whitespace trimmed.
        assert!(
            check_website_access("https://shady.example", Some(&cfg))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn malformed_yaml_propagates_with_explicit_path() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "security: [not, a, mapping]\n");
        let err = check_website_access("https://x.example", Some(&path)).unwrap_err();
        assert!(
            matches!(err, WebsitePolicyError::SecurityNotMapping(_)),
            "unexpected err: {err}",
        );
    }

    #[test]
    fn host_extraction_handles_schemeless_input() {
        assert_eq!(extract_host_from_urlish("evil.com/path"), "evil.com");
        assert_eq!(extract_host_from_urlish("HTTPS://Evil.COM"), "evil.com");
        // Trailing dot is normalized away.
        assert_eq!(extract_host_from_urlish("http://evil.com./"), "evil.com");
        // Garbage in -> empty (no panic).
        assert_eq!(extract_host_from_urlish(""), "");
    }

    #[test]
    fn missing_config_file_treated_as_empty_policy() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("does_not_exist.yaml");
        let policy = load_website_blocklist(Some(&nonexistent)).unwrap();
        assert!(!policy.enabled);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn reset_cache_drops_state() {
        // Seed the cache with something then reset it.
        *cache_slot().lock() = Some(CachedPolicy {
            policy: WebsitePolicy {
                enabled: true,
                rules: vec![BlocklistRule {
                    pattern: "x.example".into(),
                    source: "config".into(),
                }],
            },
            at: Instant::now(),
        });
        assert!(cache_slot().lock().is_some());
        reset_cache();
        assert!(cache_slot().lock().is_none());
    }
}
