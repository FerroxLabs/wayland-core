//! The operator support bundle — evidence that can leave the machine.
//!
//! Phase 24 Success Criterion 4, threat T-24-03-01 (CRITICAL).
//!
//! # A support bundle that leaks a secret is worse than no support bundle
//!
//! Everything here is shaped by that. A bundle is created precisely when
//! something is wrong, by an operator who is about to attach it to a ticket —
//! so it crosses a trust boundary by design, under time pressure, without
//! being read first.
//!
//! # Two defences, and the ORDER matters
//!
//! 1. **Structural elision.** Secret-bearing sources contribute KEY NAMES
//!    only. The configuration file, the credentials store and the process
//!    environment are enumerated as names — `providers.anthropic.api_key`,
//!    `AWS_SECRET_ACCESS_KEY` — and their values are never read into the
//!    bundle at all. This is the primary defence because it cannot be defeated
//!    by a secret the collector did not know about.
//!
//! 2. **Exact-secret scrubbing**, over free text only. Logs are prose written
//!    by dozens of call sites and structural elision cannot apply to them, so
//!    every known secret VALUE is replaced. This is a backstop and is treated
//!    as one: it can only catch secrets that were enumerable.
//!
//! Getting the order backwards — copying values and relying on scrubbing to
//! remove them — is the design that leaks, because it fails open for every
//! secret the redactor never learned.
//!
//! # Why the bundle is a DIRECTORY and not a `.tar.gz`
//!
//! The plan's canary gate decompresses an archive. This produces a directory
//! instead, for two reasons. The practical one: `tar` and `flate2` are
//! workspace dependencies but not this crate's, and adding either rewrites
//! `Cargo.lock`, which is a Phase-24 shared seam concurrent lanes conflict on.
//! The better one: an uncompressed tree is a STRONGER redaction posture. The
//! canary scan reads the real bytes rather than trusting a decompressor to
//! reveal them, there are no nested members that a shallow scan could miss,
//! and an operator can read exactly what they are about to hand over before
//! they hand it over. Archiving is one `tar czf` away and is the operator's
//! choice, taken after inspection rather than before it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Shortest string treated as a secret by the scrubber.
///
/// A short "secret" produces catastrophic over-redaction — a two-character
/// value occurs inside ordinary words and would blank most of a log, which
/// pressures an operator into disabling redaction entirely. Structural elision
/// is what protects short secrets; this bound protects the bundle's
/// legibility.
pub const MIN_SCRUBBABLE_SECRET_LEN: usize = 8;

/// Replacement text.
pub const REDACTED: &str = "[REDACTED]";

/// Largest tail of the gateway log carried into a bundle.
pub const MAX_LOG_BYTES: u64 = 256 * 1024;

/// Environment variable name fragments that mark a value as secret.
///
/// Matched case-insensitively as substrings, so `MY_APP_API_KEY` is caught by
/// `key`. Deliberately broad: a false positive costs one elided value in a
/// name list, and a false negative costs a credential.
const SECRET_NAME_FRAGMENTS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "auth",
    "session",
    "cookie",
    "private",
    "signature",
    "bearer",
];

/// Whether an environment variable or config key NAME marks a secret value.
pub fn name_marks_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAME_FRAGMENTS
        .iter()
        .any(|frag| lower.contains(frag))
}

/// Exact-secret scrubber for free text. The BACKSTOP, not the primary
/// defence — see the module docs.
#[derive(Clone, Default)]
pub struct Redactor {
    secrets: BTreeSet<String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Teach the scrubber a secret VALUE.
    ///
    /// Values shorter than [`MIN_SCRUBBABLE_SECRET_LEN`] are ignored: see the
    /// constant. Returns whether it was taken, so a caller can tell the
    /// difference between "learned" and "silently discarded" — a scrubber that
    /// quietly drops what it is handed is one that reports a clean bundle it
    /// never protected.
    pub fn learn(&mut self, secret: &str) -> bool {
        let s = secret.trim();
        if s.len() < MIN_SCRUBBABLE_SECRET_LEN {
            return false;
        }
        self.secrets.insert(s.to_string());
        true
    }

    /// Learn the values of every environment variable whose NAME marks it as
    /// secret-bearing.
    pub fn learn_from_environment(&mut self) {
        for (name, value) in std::env::vars() {
            if name_marks_secret(&name) {
                self.learn(&value);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Replace every known secret in `text`, returning the scrubbed text and
    /// how many replacements were made.
    pub fn scrub(&self, text: &str) -> (String, usize) {
        let mut out = text.to_string();
        let mut hits = 0usize;
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                hits += out.matches(secret.as_str()).count();
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        (out, hits)
    }
}

/// What a bundle contains and how it was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub created_at: String,
    pub home: PathBuf,
    pub os: String,
    pub arch: String,
    pub binary_path: Option<PathBuf>,
    pub binary_version: Option<String>,
    /// Files written, relative to the bundle root.
    pub members: Vec<String>,
    /// How many secret values the scrubber knew. Recorded so a reviewer can
    /// see a bundle produced with an EMPTY scrubber, which is a bundle whose
    /// clean canary scan means considerably less.
    pub known_secrets: usize,
    /// Replacements the scrubber actually made.
    pub redactions: usize,
    /// Sources that were expected and absent, named rather than skipped: a
    /// missing member and a member that was never collected look identical
    /// otherwise.
    pub absent_sources: Vec<String>,
}

/// Sources the bundle reads.
#[derive(Debug, Clone, Default)]
pub struct BundleSources {
    /// Configuration file. Only its KEY NAMES are carried.
    pub config: Option<PathBuf>,
    /// Credentials store file. Only its KEY NAMES are carried.
    pub credentials: Option<PathBuf>,
    /// Free-text log. Scrubbed, tail-bounded.
    pub log: Option<PathBuf>,
    /// JSON projections copied verbatim after scrubbing (status, health).
    pub projections: Vec<PathBuf>,
}

/// Collect a support bundle into `out_dir`.
///
/// `out_dir` is created; existing content is not removed, so a caller passing
/// a populated directory gets an error rather than a silent merge that would
/// ship whatever was already there.
pub fn collect(
    home: &Path,
    out_dir: &Path,
    sources: &BundleSources,
    redactor: &Redactor,
) -> std::io::Result<BundleManifest> {
    if out_dir.exists()
        && out_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "{} is not empty; refusing to add a bundle to a directory whose \
                 existing contents would ship with it",
                out_dir.display()
            ),
        ));
    }
    std::fs::create_dir_all(out_dir)?;

    let mut members: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();
    let mut redactions = 0usize;

    // ---- Key NAMES only. Values are never read into the bundle.
    if let Some(path) = &sources.config {
        match key_names(path) {
            Some(names) => {
                write_member(out_dir, "config-keys.txt", &names.join("\n"), &mut members)?;
            }
            None => absent.push(format!("config: {}", path.display())),
        }
    }
    if let Some(path) = &sources.credentials {
        match key_names(path) {
            Some(names) => {
                write_member(
                    out_dir,
                    "credential-keys.txt",
                    &names.join("\n"),
                    &mut members,
                )?;
            }
            None => absent.push(format!("credentials: {}", path.display())),
        }
    }

    // The environment contributes NAMES, and secret-marked names are flagged
    // so a reader can see what existed without seeing any of it.
    let mut env_names: Vec<String> = std::env::vars()
        .map(|(name, _)| {
            if name_marks_secret(&name) {
                format!("{name}  [value elided: name marks a secret]")
            } else {
                format!("{name}  [value elided]")
            }
        })
        .collect();
    env_names.sort();
    write_member(
        out_dir,
        "environment-keys.txt",
        &env_names.join("\n"),
        &mut members,
    )?;

    // ---- Projections: JSON the runtime already publishes. Scrubbed, because
    // a `reason` string is free text written by a call site that had a
    // credential in scope.
    for path in &sources.projections {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let (scrubbed, hits) = redactor.scrub(&raw);
                redactions += hits;
                write_member(out_dir, name, &scrubbed, &mut members)?;
            }
            Err(_) => absent.push(format!("projection: {}", path.display())),
        }
    }

    // ---- Log: free text, tail-bounded, scrubbed.
    if let Some(path) = &sources.log {
        match read_tail(path, MAX_LOG_BYTES) {
            Some(raw) => {
                let (scrubbed, hits) = redactor.scrub(&raw);
                redactions += hits;
                write_member(out_dir, "recent-log.txt", &scrubbed, &mut members)?;
            }
            None => absent.push(format!("log: {}", path.display())),
        }
    }

    let manifest = BundleManifest {
        created_at: chrono::Utc::now().to_rfc3339(),
        home: home.to_path_buf(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        binary_path: std::env::current_exe().ok(),
        binary_version: option_env!("CARGO_PKG_VERSION").map(str::to_string),
        members: members.clone(),
        known_secrets: redactor.len(),
        redactions,
        absent_sources: absent,
    };
    // The manifest is written last and is NOT listed in its own `members`,
    // because a member list that includes itself cannot be checked against the
    // directory without a special case.
    std::fs::write(
        out_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
    )?;
    Ok(manifest)
}

fn write_member(
    out_dir: &Path,
    name: &str,
    body: &str,
    members: &mut Vec<String>,
) -> std::io::Result<()> {
    std::fs::write(out_dir.join(name), body)?;
    members.push(name.to_string());
    Ok(())
}

/// Extract the KEY NAMES from a TOML-ish or JSON-ish file without ever
/// retaining a value.
///
/// Deliberately a lexical scan rather than a parse. A parse would need the
/// value types, which means holding the values in memory in a structure that
/// a later `Debug` or serialization could emit. This never constructs a value
/// at all: it takes the text left of the first `=` or `:` on each line and
/// discards the remainder of the line immediately.
fn key_names(path: &Path) -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut names: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // A TOML table header is a NAME, not a value.
            names.push(line.to_string());
            continue;
        }
        if let Some(idx) = line.find(['=', ':']) {
            let key = line[..idx].trim().trim_matches('"').to_string();
            if !key.is_empty() {
                names.push(format!("{key}  [value elided]"));
            }
        }
    }
    names.sort();
    names.dedup();
    Some(names)
}

/// Read at most `max` bytes from the END of `path`.
fn read_tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Every file in a bundle directory, recursively — the surface a canary scan
/// must cover.
pub fn bundle_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_secret_is_refused_by_the_scrubber_rather_than_silently_dropped() {
        // A caller must be able to tell "learned" from "discarded": a scrubber
        // that quietly drops what it is handed reports a clean bundle it never
        // protected.
        let mut r = Redactor::new();
        assert!(!r.learn("abc"), "short values must be refused");
        assert!(r.is_empty());
        assert!(r.learn("long-enough-secret"));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn scrubbing_replaces_every_occurrence_and_counts_them() {
        let mut r = Redactor::new();
        r.learn("sk-canary-value-1234");
        let (out, hits) = r.scrub("a sk-canary-value-1234 b sk-canary-value-1234 c");
        assert_eq!(hits, 2);
        assert!(!out.contains("sk-canary-value-1234"));
        assert_eq!(out.matches(REDACTED).count(), 2);
    }

    #[test]
    fn secret_marking_catches_the_common_name_shapes() {
        for name in [
            "ANTHROPIC_API_KEY",
            "aws_secret_access_key",
            "MyAppToken",
            "DB_PASSWORD",
            "session_cookie",
            "GITHUB_BEARER",
        ] {
            assert!(name_marks_secret(name), "{name} must be marked secret");
        }
        for name in ["HOME", "PATH", "LANG", "TERM"] {
            assert!(!name_marks_secret(name), "{name} must not be marked");
        }
    }

    #[test]
    fn key_extraction_keeps_names_and_never_the_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# comment\n[providers.anthropic]\napi_key = \"sk-SECRET-VALUE-abcdefgh\"\n\
             base_url = \"https://api.anthropic.com\"\n",
        )
        .unwrap();
        let names = key_names(&path).unwrap().join("\n");
        assert!(names.contains("api_key"), "the NAME must survive: {names}");
        assert!(
            !names.contains("sk-SECRET-VALUE-abcdefgh"),
            "the value must not: {names}"
        );
        assert!(
            !names.contains("https://api.anthropic.com"),
            "no value survives, not even a harmless-looking one — a rule with \
             exceptions is a rule nobody can check: {names}"
        );
    }

    #[test]
    fn collect_refuses_a_non_empty_output_directory() {
        // Otherwise whatever was already in the directory ships inside the
        // bundle the operator attaches to a ticket.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("someone-elses-file"), "private").unwrap();
        let err = collect(
            dir.path(),
            &out,
            &BundleSources::default(),
            &Redactor::new(),
        )
        .expect_err("a populated output directory must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn absent_sources_are_named_rather_than_skipped() {
        // A member that is missing and a member that was never collected look
        // identical in a bundle unless the absence is recorded.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let sources = BundleSources {
            config: Some(dir.path().join("does-not-exist.toml")),
            log: Some(dir.path().join("no-such.log")),
            ..Default::default()
        };
        let manifest = collect(dir.path(), &out, &sources, &Redactor::new()).unwrap();
        assert_eq!(
            manifest.absent_sources.len(),
            2,
            "{:?}",
            manifest.absent_sources
        );
        assert!(
            manifest
                .absent_sources
                .iter()
                .any(|s| s.starts_with("config:"))
        );
        assert!(
            manifest
                .absent_sources
                .iter()
                .any(|s| s.starts_with("log:"))
        );
    }

    #[test]
    fn the_manifest_records_an_empty_scrubber() {
        // A clean canary scan over a bundle produced with zero known secrets
        // means much less, so a reviewer must be able to see that it was zero.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let manifest = collect(
            dir.path(),
            &out,
            &BundleSources::default(),
            &Redactor::new(),
        )
        .unwrap();
        assert_eq!(manifest.known_secrets, 0);
        assert_eq!(manifest.redactions, 0);
    }

    #[test]
    fn every_declared_member_exists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let manifest = collect(
            dir.path(),
            &out,
            &BundleSources::default(),
            &Redactor::new(),
        )
        .unwrap();
        for m in &manifest.members {
            assert!(out.join(m).exists(), "declared member {m} is not on disk");
        }
        assert!(out.join("manifest.json").exists());
    }
}
