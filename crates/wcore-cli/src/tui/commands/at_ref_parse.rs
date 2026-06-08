//! Parsing for `@`-references — the [`AtRef`] type, the [`AtRefError`]
//! error enum, and [`AtRef::parse`].
//!
//! This module is pure syntax: it turns a `@…` composer token into a
//! typed [`AtRef`] and never touches the filesystem or the network.
//! Resolution (`at_ref_resolve`) and completion (`at_ref_complete`) build
//! on the types defined here. Split out of `at_refs.rs` (W3-B).

use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────

/// A failure resolving an `@`-reference.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AtRefError {
    /// The token did not start with `@` or was otherwise unparseable.
    #[error("not a valid @-reference: {0:?}")]
    Malformed(String),

    /// The referenced path / target does not exist.
    #[error("@-reference target not found: {0}")]
    NotFound(String),

    /// The path resolved to a credential/secret file on the denylist and
    /// was refused. Surfaced loudly — never a silent omission.
    #[error("refused to attach a secret file: {0} (matches the secret denylist)")]
    SecretBlocked(String),

    /// The path is excluded by a `.gitignore` rule.
    #[error("@-reference target is git-ignored: {0}")]
    GitIgnored(String),

    /// An `@url` did not parse as a valid `http(s)` URL.
    #[error("invalid @url: {0}")]
    BadUrl(String),

    /// The filesystem could not be read.
    #[error("filesystem error reading {path}: {message}")]
    Io {
        /// The path the failed read targeted.
        path: String,
        /// The underlying I/O error message.
        message: String,
    },
}

// ─────────────────────────────────────────────────────────────────────────
// AtRef — a parsed @-reference
// ─────────────────────────────────────────────────────────────────────────

/// The kind of thing an `@`-reference attaches. UX doc §3b — the seven
/// grounded forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtRef {
    /// `@path/to/file` — one file's contents.
    File(PathBuf),
    /// `@path/to/dir/` — a directory tree.
    Dir(PathBuf),
    /// `@SymbolName` — a function/type definition + call sites, from the
    /// repomap symbol index (wired in Wave 2).
    Symbol(String),
    /// `@diff` (working tree) or `@diff <ref>` (vs a git ref).
    Diff {
        /// An optional git ref to diff against; `None` is the working tree.
        base: Option<String>,
    },
    /// `@url <https://…>` — a fetched, readable-extracted web page.
    Url(String),
    /// `@session <id>` — a past session as reference context.
    Session(String),
    /// `@output` — the last shell command's stdout/stderr.
    Output,
}

impl AtRef {
    /// The bare token kind keyword (`file`, `dir`, `symbol`, …), used for
    /// the completion popup's right-hand "surfaced as" label.
    pub fn kind_label(&self) -> &'static str {
        match self {
            AtRef::File(_) => "file",
            AtRef::Dir(_) => "directory",
            AtRef::Symbol(_) => "symbol",
            AtRef::Diff { .. } => "diff",
            AtRef::Url(_) => "url",
            AtRef::Session(_) => "session",
            AtRef::Output => "output",
        }
    }

    /// Parse one whitespace-delimited `@…` token (plus any trailing
    /// argument the static kinds need, e.g. `@diff main`, `@url https://…`).
    ///
    /// `raw` is the text *including* the leading `@`. A path token ending
    /// in `/` parses as [`AtRef::Dir`], otherwise as [`AtRef::File`] — the
    /// composer disambiguates `@symbol` from `@file` by whether the body
    /// contains a path separator or a known static keyword.
    pub fn parse(raw: &str) -> Result<Self, AtRefError> {
        let body = raw
            .strip_prefix('@')
            .ok_or_else(|| AtRefError::Malformed(raw.to_string()))?;
        if body.is_empty() {
            return Err(AtRefError::Malformed(raw.to_string()));
        }

        // Split off an optional argument: `@diff main`, `@url https://…`,
        // `@session abc`. The keyword is the first whitespace-delimited
        // word; everything after is the argument.
        let mut parts = body.splitn(2, char::is_whitespace);
        let keyword = parts.next().unwrap_or("");
        let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());

        match keyword {
            "diff" => Ok(AtRef::Diff {
                base: arg.map(str::to_string),
            }),
            "output" => Ok(AtRef::Output),
            "url" => {
                let url = arg.ok_or_else(|| AtRefError::BadUrl(raw.to_string()))?;
                if is_http_url(url) {
                    Ok(AtRef::Url(url.to_string()))
                } else {
                    Err(AtRefError::BadUrl(url.to_string()))
                }
            }
            "session" => {
                let id = arg.ok_or_else(|| AtRefError::Malformed(raw.to_string()))?;
                Ok(AtRef::Session(id.to_string()))
            }
            // Path-or-symbol: a token with a separator (or a trailing `/`)
            // is a path; a bare identifier is a symbol.
            _ => {
                if looks_like_path(keyword) {
                    let path = PathBuf::from(keyword);
                    if keyword.ends_with('/') {
                        Ok(AtRef::Dir(path))
                    } else {
                        Ok(AtRef::File(path))
                    }
                } else {
                    Ok(AtRef::Symbol(keyword.to_string()))
                }
            }
        }
    }
}

/// True if a token body should be treated as a filesystem path rather than
/// a symbol name. A path has a separator, a `.` extension, or a leading
/// `.` (a dotfile); a bare CamelCase / snake_case identifier is a symbol.
fn looks_like_path(body: &str) -> bool {
    body.ends_with('/')
        || body.contains('/')
        || body.contains('\\')
        || body.starts_with("./")
        || body.starts_with("../")
        // A leading dot marks a dotfile (`.env`, `.gitignore`) — always a
        // path, never a symbol name (no identifier starts with a dot).
        || body.starts_with('.')
        // A dotted name with no separator and a short extension-like tail
        // (`compat.rs`, `main.py`) reads as a file, not a symbol.
        || body
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=4).contains(&ext.len()))
}

/// True for a syntactically valid `http`/`https` URL. Deliberately strict:
/// `@url` is the one form with a network seam, so a malformed URL must
/// fail fast at parse time rather than at fetch time.
fn is_http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    match rest {
        Some(host_and_path) => {
            let host = host_and_path
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default();
            // A host needs a dot (`example.com`) or is `localhost`, and no
            // whitespace anywhere in the URL.
            !s.contains(char::is_whitespace)
                && !host.is_empty()
                && (host.contains('.') || host.starts_with("localhost"))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_a_token_without_an_at_sign() {
        assert!(matches!(
            AtRef::parse("file.rs"),
            Err(AtRefError::Malformed(_))
        ));
        assert!(matches!(AtRef::parse("@"), Err(AtRefError::Malformed(_))));
    }

    #[test]
    fn parse_file_vs_dir_by_trailing_slash() {
        assert_eq!(
            AtRef::parse("@src/main.rs").unwrap(),
            AtRef::File(PathBuf::from("src/main.rs"))
        );
        assert_eq!(
            AtRef::parse("@src/").unwrap(),
            AtRef::Dir(PathBuf::from("src/"))
        );
    }

    #[test]
    fn parse_bare_identifier_is_a_symbol() {
        assert_eq!(
            AtRef::parse("@ProviderCompat").unwrap(),
            AtRef::Symbol("ProviderCompat".to_string())
        );
        assert_eq!(
            AtRef::parse("@resolve_field").unwrap(),
            AtRef::Symbol("resolve_field".to_string())
        );
    }

    #[test]
    fn parse_dotted_filename_is_a_file_not_a_symbol() {
        // `@compat.rs` reads as a file (short extension), `@foo.bar.baz`
        // with a long tail stays a symbol.
        assert_eq!(
            AtRef::parse("@compat.rs").unwrap(),
            AtRef::File(PathBuf::from("compat.rs"))
        );
        assert!(matches!(
            AtRef::parse("@some_module").unwrap(),
            AtRef::Symbol(_)
        ));
    }

    #[test]
    fn parse_diff_with_and_without_a_base_ref() {
        assert_eq!(AtRef::parse("@diff").unwrap(), AtRef::Diff { base: None });
        assert_eq!(
            AtRef::parse("@diff main").unwrap(),
            AtRef::Diff {
                base: Some("main".to_string())
            }
        );
    }

    #[test]
    fn parse_output_and_session() {
        assert_eq!(AtRef::parse("@output").unwrap(), AtRef::Output);
        assert_eq!(
            AtRef::parse("@session abc123").unwrap(),
            AtRef::Session("abc123".to_string())
        );
        assert!(matches!(
            AtRef::parse("@session"),
            Err(AtRefError::Malformed(_))
        ));
    }

    #[test]
    fn parse_url_validates_the_scheme_and_host() {
        assert_eq!(
            AtRef::parse("@url https://example.com/page").unwrap(),
            AtRef::Url("https://example.com/page".to_string())
        );
        assert!(matches!(
            AtRef::parse("@url not-a-url"),
            Err(AtRefError::BadUrl(_))
        ));
        assert!(matches!(
            AtRef::parse("@url ftp://example.com"),
            Err(AtRefError::BadUrl(_))
        ));
        assert!(matches!(AtRef::parse("@url"), Err(AtRefError::BadUrl(_))));
    }

    #[test]
    fn http_url_validation_edge_cases() {
        assert!(is_http_url("http://localhost:8080/x"));
        assert!(is_http_url("https://a.b.c/path?q=1#frag"));
        assert!(!is_http_url("https://nodot/path")); // host needs a dot
        assert!(!is_http_url("https://exa mple.com")); // whitespace
        assert!(!is_http_url("https://")); // empty host
    }

    #[test]
    fn kind_label_covers_every_at_ref_variant() {
        assert_eq!(AtRef::File(PathBuf::new()).kind_label(), "file");
        assert_eq!(AtRef::Dir(PathBuf::new()).kind_label(), "directory");
        assert_eq!(AtRef::Symbol(String::new()).kind_label(), "symbol");
        assert_eq!(AtRef::Diff { base: None }.kind_label(), "diff");
        assert_eq!(AtRef::Url(String::new()).kind_label(), "url");
        assert_eq!(AtRef::Session(String::new()).kind_label(), "session");
        assert_eq!(AtRef::Output.kind_label(), "output");
    }
}
