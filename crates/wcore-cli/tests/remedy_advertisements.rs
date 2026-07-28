//! A remediation string that names something the product cannot honour must
//! fail CI, not ship.
//!
//! # Why this file exists
//!
//! Five separate defects on this program share exactly one shape: **the product
//! tells the operator to do something that does not work.** Each was found by a
//! human driving the real binary; none was visible from source review, because
//! the string and its consumer are in different crates and nothing ever compared
//! them.
//!
//! | # | ledger | advertised | why it was dead |
//! |---|---|---|---|
//! | 1 | `27-C2(a)` | `[browser] allowed_origins = [..]` | loader reads `browser.policy.*`; the key parsed cleanly and was **silently discarded** |
//! | 2 | `23A-C1` | `--skills-promote` in `--help` | handler is an unconditional `bail!` — always exits 1 |
//! | 3 | `24-C2` | `--trigger webhook:` / `poll:` | accepted at add, never fired |
//! | 4 | ollama hint | "select a model id prefixed `ollama:` — no API key is needed" | credential resolution returns `MissingApiKey` before the model string is read |
//! | 5 | headless keyring | `credentials.backend = "encrypted-file"` | wrong section, unparseable value, struct variant, and a passphrase mechanism named in 0 docs / 0 help / 0 errors |
//!
//! `wcore-agent/src/recovery_confidential.rs` already carries the countermeasure
//! *in miniature* for case 5: it re-parses the value its own error message
//! advertises through the real `CredentialsStorageConfig`. This file generalises
//! that from one hand-written pair to a sweep of the whole workspace.
//!
//! # What makes case 1 the hard one
//!
//! `BrowserConfig` and `BrowserPolicyConfig` are both `#[serde(default)]` with no
//! `deny_unknown_fields`, so `[browser] allowed_origins` **parses without error**
//! and is dropped on the floor. A gate that only asked "does this parse?" — which
//! is what the miniature does — passes on case 1. So the check here is
//! **retention**, not parseability: the advertised key/value must still be there
//! after a round trip through the real loader. Retention subsumes parseability
//! (an unparseable value cannot be retained) and additionally catches the silent
//! drop, which is the strictly nastier defect because the operator gets no
//! diagnostic at all.
//!
//! # What this file does NOT check
//!
//! Stated plainly, because a gate that silently covers a fraction while reading
//! as complete is worse than one that declares its coverage. See
//! `.planning/REMEDY-GATE.md` for the measured figures.
//!
//! * **Prose remedies.** "Configure an OS keyring", "run it in a terminal" name
//!   no token and cannot be mechanically checked. They are the majority.
//! * **Flag *values*.** Case 3 (`--trigger webhook:`) advertises a value prefix
//!   whose consumer is a dispatcher, not a parser. There is no one type to
//!   re-parse it through; it needs a per-consumer binding.
//! * **Ordering defects.** Case 4 is not a wrong token — `ollama:` really is a
//!   valid model prefix. The defect was that credential resolution ran *before*
//!   the model string was read, so the advertised route was unreachable. That is
//!   a control-flow property, not a naming one.
//! * **Config assignments whose root is not a real `ConfigFile` field and whose
//!   leaf name is unknown to the schema.** Deliberate: `core.bare = false` (git
//!   config) and `group = [projects]` (gcloud) appear in operator-facing strings
//!   in this workspace and are not this product's config. Admitting them would
//!   red the gate on correct text, and a gate that reds on correct text gets
//!   deleted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use wcore_config::config::ConfigFile;

// ---------------------------------------------------------------- workspace

fn workspace_root() -> PathBuf {
    // crates/wcore-cli -> crates -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Production Rust sources only.
///
/// `tests/`, `benches/` and `examples/` are excluded, and so is every
/// `#[cfg(test)] mod .. { .. }` block (see [`strip_test_modules`]): a string only
/// a test ever sees is not advertised to anybody, and including them would let
/// fixture TOML inflate the gate's apparent coverage.
fn production_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(
                    name.as_ref(),
                    "target" | ".git" | "tests" | "benches" | "examples"
                ) {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    let crates = workspace_root().join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        panic!("cannot read {}", crates.display());
    };
    for entry in entries.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out.sort();
    out
}

/// Blank out `#[cfg(test)] mod .. { .. }` blocks, preserving byte offsets.
///
/// Offsets are preserved on purpose: section-header-to-key binding below is
/// positional, and shortening the text silently re-points headers at keys
/// further down. That exact bug mis-bound five `[tools]`/`[session]` keys to
/// `[default]` in an earlier revision of the sweeper.
fn strip_test_modules(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut search = 0usize;
    while let Some(rel) = src[search..].find("#[cfg(test)]") {
        let at = search + rel;
        let Some(open_rel) = src[at..].find('{') else {
            break;
        };
        let open = at + open_rel;
        let mut depth = 0i32;
        let mut i = open;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let end = (i + 1).min(bytes.len());
        for b in out.iter_mut().take(end).skip(at) {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        search = end;
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

/// Every Rust string literal in `src`, with its 1-based line number.
///
/// Hand-lexed rather than regexed because this codebase's remediation text uses
/// both raw strings (`r"..."`, `r#"..."#`) and backslash-continued multi-line
/// literals, and a line-oriented regex mangles both.
fn string_literals(src: &str) -> Vec<(usize, String)> {
    let b: Vec<char> = src.chars().collect();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < n {
        let c = b[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < n && b[i + 1] == '/' {
            while i < n && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && b[i + 1] == '*' {
            let mut j = i + 2;
            while j + 1 < n && !(b[j] == '*' && b[j + 1] == '/') {
                if b[j] == '\n' {
                    line += 1;
                }
                j += 1;
            }
            i = (j + 2).min(n);
            continue;
        }
        // char literal / lifetime -- skip, so `'"'` cannot open a string
        if c == '\'' {
            if i + 2 < n && b[i + 1] == '\\' {
                i += 4;
            } else if i + 2 < n && b[i + 2] == '\'' {
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        // raw string
        if c == 'r' && i + 1 < n && (b[i + 1] == '"' || b[i + 1] == '#') {
            let mut h = 0usize;
            while i + 1 + h < n && b[i + 1 + h] == '#' {
                h += 1;
            }
            if i + 1 + h < n && b[i + 1 + h] == '"' {
                let start = i + 2 + h;
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', h))
                    .collect();
                let rest: String = b[start..].iter().collect();
                if let Some(rel) = rest.find(&close) {
                    let text: String = rest[..rel].to_string();
                    let started = line;
                    line += text.matches('\n').count();
                    out.push((started, text));
                    i = start + rest[..rel].chars().count() + close.chars().count();
                    continue;
                }
            }
        }
        if c == '"' {
            let started = line;
            let mut j = i + 1;
            let mut buf = String::new();
            while j < n {
                if b[j] == '\\' && j + 1 < n {
                    let nx = b[j + 1];
                    if nx == '\n' {
                        line += 1;
                        let mut k = j + 2;
                        while k < n && (b[k] == ' ' || b[k] == '\t') {
                            k += 1;
                        }
                        j = k;
                        continue;
                    }
                    buf.push(match nx {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    j += 2;
                    continue;
                }
                if b[j] == '"' {
                    break;
                }
                if b[j] == '\n' {
                    line += 1;
                }
                buf.push(b[j]);
                j += 1;
            }
            out.push((started, buf));
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- extraction

fn section_re() -> Regex {
    Regex::new(r"\[([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)\]").unwrap()
}

fn keyval_re() -> Regex {
    // `key = value` and the prose form `key to "value"`, both of which appear
    // verbatim in shipped remediation text. Case 5's pre-fix string used the
    // prose form ("set `credentials.backend` to \"encrypted-file\""), so an
    // extractor that only understood `=` would have been blind to it.
    Regex::new(
        r#"(?:^|[^\w.\-])([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)\s*(?:=|\sto\s)\s*`?("[^"]*"|\[[^\]]*\]|true|false|-?\d+)"#,
    )
    .unwrap()
}

/// A `section.key = value` an operator is told to write.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Advertised {
    path: String,
    value: String,
    file: String,
    line: usize,
    context: String,
}

/// Bind each assignment to a section header POSITIONALLY.
///
/// Preference order, both measured against real shipped strings:
///   1. the nearest header BEFORE the assignment (pasted TOML snippets:
///      `[browser.policy]\nallowed_origins = [..]`);
///   2. failing that, a header AFTER it **on the same line** (prose remedies:
///      "set `enabled = true` under `[anvil]`" — five shipped strings phrase it
///      this way, and binding them to nothing would drop them silently).
///
/// A "last header on the line wins" rule was tried first and was wrong: the live
/// headless-keyring string names two sections on one line and that rule bound
/// `backend` to `[session]`, inventing a key that does not exist. A gate that
/// reds on correct text gets deleted, so a false positive is worse than a miss.
fn advertised_assignments(text: &str, file: &str, line: usize, context: &str) -> Vec<Advertised> {
    // comment lines are illustration, not instruction -- blanked, length-preserving
    let masked: String = text
        .split('\n')
        .map(|l| {
            if l.trim_start().starts_with('#') {
                " ".repeat(l.chars().count())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let sec = section_re();
    let heads: Vec<(usize, usize, String)> = sec
        .captures_iter(&masked)
        .map(|c| {
            let m = c.get(0).unwrap();
            (m.start(), m.end(), c[1].to_string())
        })
        .collect();

    let line_start = |pos: usize| masked[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);

    let mut out = Vec::new();
    for cap in keyval_re().captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        let key = cap[1].to_string();
        let value = cap[2].to_string();
        let at = cap.get(1).unwrap().start();

        let before = heads.iter().filter(|(s, _, _)| *s < at).next_back();
        let same_line_after = heads
            .iter()
            .find(|(s, _, _)| *s >= whole.end() && line_start(*s) == line_start(at));

        let section = before.or(same_line_after).map(|(_, _, name)| name.clone());

        let path = match section {
            Some(s) if !key.starts_with(&format!("{}.", s.split('.').next().unwrap_or(&s))) => {
                format!("{s}.{key}")
            }
            _ => key,
        };
        out.push(Advertised {
            path,
            value,
            file: file.to_string(),
            line,
            context: context.to_string(),
        });
    }
    out
}

fn imperative_re() -> Regex {
    Regex::new(
        r"(?i)\b(set|setting|run|use|using|configure|install|add|pass|supply|provide|enable|disable|export|try|select|specify|remove|create|choose|turn|paste|permit|allow|edit|write|put|place)\b",
    )
    .unwrap()
}

/// Sweep every production source for advertised config assignments.
fn sweep_advertised() -> Vec<Advertised> {
    let imp = imperative_re();
    let toml_header = Regex::new(r"(?m)^\s*\[[a-z][a-z0-9_.]*\]\s*$").unwrap();
    let root = workspace_root();
    let mut out = Vec::new();
    for path in production_sources() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let src = strip_test_modules(&raw);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        let lines: Vec<&str> = src.split('\n').collect();
        for (line, text) in string_literals(&src) {
            if text.len() < 12 {
                continue;
            }
            // Instruction, or a pasteable TOML snippet (case 1's hint body is a
            // bare `[browser.policy]\nallowed_origins = [..]` const with no verb
            // in it at all -- requiring a verb made the gate blind to the very
            // defect it was built for).
            if !imp.is_match(&text) && !toml_header.is_match(&text) {
                continue;
            }
            let idx = line.saturating_sub(1);
            let lo = idx.saturating_sub(2);
            let window = lines[lo..(idx + 1).min(lines.len())].join("\n");
            let context = if window.contains("#[error(") {
                "error_display"
            } else if window.contains("bail!") || window.contains("anyhow!") {
                "error_construct"
            } else if window.contains("println!") || window.contains("eprintln!") {
                "stdout"
            } else {
                "other"
            };
            out.extend(advertised_assignments(&text, &rel, line, context));
        }
    }
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------- schema

/// The real config schema, as JSON.
///
/// JSON rather than TOML on purpose: `ConfigFile` has `Option` fields with no
/// `skip_serializing_if`, and the TOML value serializer rejects `None`. JSON
/// renders them as explicit `null` keys, which is exactly what we need — those
/// roots (`bedrock`, `vertex`, `memory`) are real fields and must not be
/// mistaken for unknown ones.
fn schema() -> serde_json::Value {
    serde_json::to_value(ConfigFile::default()).expect("ConfigFile must serialize to JSON")
}

fn schema_roots(schema: &serde_json::Value) -> BTreeSet<String> {
    schema
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn schema_leaf_names(v: &serde_json::Value, out: &mut BTreeSet<String>) {
    if let Some(obj) = v.as_object() {
        for (k, child) in obj {
            out.insert(k.clone());
            schema_leaf_names(child, out);
        }
    }
}

fn json_at<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

// ---------------------------------------------------------------- the checks

/// The core check, and the answer to "could this gate fail?".
///
/// For every config assignment a shipped operator-facing string tells someone to
/// write, that assignment must SURVIVE a round trip through the real
/// `ConfigFile` loader — present afterwards, and equal to what was advertised.
///
/// Three distinct historical defects red this, and each fails at a different
/// point:
///   * wrong section (`[browser] allowed_origins`, `credentials.backend`) —
///     parses fine, silently dropped, absent after the round trip;
///   * unaccepted value (`backend = "encrypted-file"`) — `ConfigFile` deser
///     errors while generic TOML parses fine;
///   * struct variant as a bare string (`backend = "encrypted_file"`) — same.
///
/// `checker_reds_on_the_historical_defect_shapes` below drives all three through
/// this same code path with synthetic input and asserts each goes red, so the
/// mechanism is proven able to fail independently of whatever the tree currently
/// ships.
#[test]
fn advertised_config_assignments_survive_the_real_loader() {
    let schema = schema();
    let roots = schema_roots(&schema);
    let mut leaves = BTreeSet::new();
    schema_leaf_names(&schema, &mut leaves);

    let mut checked = 0usize;
    let mut illustrative = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for adv in sweep_advertised() {
        let root = adv.path.split('.').next().unwrap_or_default().to_string();
        let leaf = adv.path.rsplit('.').next().unwrap_or_default().to_string();

        // Admission. Either the root is a real ConfigFile field (so the path is
        // about THIS product's config and any depth error inside it is a real
        // defect), or the leaf name is one the schema knows and the carrying
        // string is config-flavoured (which is how a WRONG root -- the case 1
        // and case 5(a) defect shape -- gets admitted at all).
        let admitted = roots.contains(&root) || leaves.contains(&leaf);
        if !admitted {
            continue;
        }

        let snippet = format!("{} = {}", adv.path, adv.value);
        // Illustrative text (`proposers = ["provider", ...]`, `model =
        // "{model}"`) is not literal TOML. Skip it, but ONLY when generic TOML
        // rejects it -- if generic TOML accepts it and ConfigFile does not, that
        // is exactly the case 5(b)/(c) defect and must red.
        let Ok(generic) = snippet.parse::<toml::Value>() else {
            illustrative += 1;
            continue;
        };
        checked += 1;

        let expected = serde_json::to_value(&generic)
            .ok()
            .and_then(|v| json_at(&v, &adv.path).cloned());

        match toml::from_str::<ConfigFile>(&snippet) {
            Err(e) => failures.push(format!(
                "{}:{} [{}] advertises `{}` -- the real loader REJECTS it, so an \
                 operator who follows this text ends up with a config that will \
                 not load at all: {}",
                adv.file, adv.line, adv.context, snippet, e
            )),
            Ok(cfg) => {
                let round = serde_json::to_value(&cfg).expect("ConfigFile -> JSON");
                match json_at(&round, &adv.path) {
                    None => failures.push(format!(
                        "{}:{} [{}] advertises `{}` -- it parses without error and \
                         is then SILENTLY DISCARDED: `{}` does not exist in the \
                         schema the loader reads. The operator gets no diagnostic \
                         at all and the setting never takes effect.",
                        adv.file, adv.line, adv.context, snippet, adv.path
                    )),
                    Some(got) => {
                        if let Some(want) = expected {
                            if *got != want {
                                failures.push(format!(
                                    "{}:{} [{}] advertises `{}` but the loader ends \
                                     up holding {} at `{}`",
                                    adv.file, adv.line, adv.context, snippet, got, adv.path
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    // Anti-vacuity. A sweep that extracted nothing would pass silently, which is
    // the single most common way a gate in this repo has self-passed. The floor
    // is well under the count measured at authoring time (48 header-bound
    // assignments) so ordinary churn cannot trip it, but a broken lexer or a
    // broken extractor drops straight through it.
    assert!(
        checked >= 15,
        "only {checked} advertised config assignments were checked ({illustrative} \
         skipped as illustrative). The sweep is broken -- fix the extraction, do \
         NOT lower this floor."
    );

    assert!(
        failures.is_empty(),
        "{} advertised config assignment(s) the product cannot honour \
         ({checked} checked, {illustrative} illustrative):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Proof the checker above can fail, driven through its own code path.
///
/// These are the three historical defect shapes as literal input. If any of them
/// stops going red, the mechanism has been broken and the green above means
/// nothing. This is the control that makes the main test's pass worth reading.
#[test]
fn checker_reds_on_the_historical_defect_shapes() {
    // case 1 (27-C2(a)): right key, wrong section. The whole point is that this
    // PARSES -- BrowserConfig is #[serde(default)] with no deny_unknown_fields --
    // so a parseability check passes on it and only retention catches it.
    let pre_fix_browser = "browser.allowed_origins = [\"example.com\"]";
    let cfg = toml::from_str::<ConfigFile>(pre_fix_browser)
        .expect("the defect is that this parses cleanly -- that is why it shipped");
    let round = serde_json::to_value(&cfg).unwrap();
    assert!(
        json_at(&round, "browser.allowed_origins").is_none(),
        "27-C2(a) must be detectable: `[browser] allowed_origins` has to vanish \
         from the loader's view, otherwise this gate is not measuring retention"
    );

    // case 1, fixed: the section the loader actually reads.
    let fixed_browser = "browser.policy.allowed_origins = [\"example.com\"]";
    let cfg = toml::from_str::<ConfigFile>(fixed_browser).expect("fixed form must parse");
    let round = serde_json::to_value(&cfg).unwrap();
    assert_eq!(
        json_at(&round, "browser.policy.allowed_origins"),
        Some(&serde_json::json!(["example.com"])),
        "the fixed form must be RETAINED, or the gate reds on correct text"
    );

    // case 5(a): `credentials` is not a section at all.
    let pre_fix_creds = "credentials.backend = \"encrypted-file\"";
    assert!(
        pre_fix_creds.parse::<toml::Value>().is_ok(),
        "must be well-formed TOML, else it would be skipped as illustrative"
    );
    let cfg = toml::from_str::<ConfigFile>(pre_fix_creds)
        .expect("unknown roots are ignored, not rejected -- that IS the defect");
    let round = serde_json::to_value(&cfg).unwrap();
    assert!(
        json_at(&round, "credentials.backend").is_none(),
        "case 5(a) must be detectable: a key at a non-existent section has to \
         vanish from the loader's view"
    );

    // case 5(b): correct section, value the parser rejects outright.
    assert!(
        toml::from_str::<ConfigFile>("storage.credentials.backend = \"encrypted-file\"").is_err(),
        "case 5(b) must be detectable: `encrypted-file` is not a variant"
    );
    // case 5(c): even the spelling the parser itself suggests fails, because the
    // variant is a struct variant.
    assert!(
        toml::from_str::<ConfigFile>("storage.credentials.backend = \"encrypted_file\"").is_err(),
        "case 5(c) must be detectable: EncryptedFile is a struct variant and can \
         never be a bare string in any spelling"
    );
    // and the value that was MEASURED to work must be retained.
    let cfg = toml::from_str::<ConfigFile>("storage.credentials.backend = \"keyring\"")
        .expect("the shipped remedy must parse");
    let round = serde_json::to_value(&cfg).unwrap();
    assert!(
        json_at(&round, "storage.credentials.backend").is_some(),
        "the shipped remedy must be retained"
    );
}

/// The extractor must survive the phrasings that actually ship.
///
/// Every string below is a real shipped remediation, reduced. Two extractor bugs
/// were found this way and both would have silently changed the gate's meaning:
/// line-scoped section binding (false positive) and length-changing comment
/// masking (mis-bound five keys).
#[test]
fn extractor_binds_sections_the_way_shipped_strings_phrase_them() {
    let cases: &[(&str, &str, &str)] = &[
        // header first, on its own line (pasted snippet)
        (
            "[browser.policy]\n# Allow specific domains\nallowed_origins = [\"example.com\"]\n",
            "browser.policy.allowed_origins",
            "[\"example.com\"]",
        ),
        // two headers on ONE line -- must bind positionally, not last-wins
        (
            "no OS keyring was usable. set [storage.credentials] backend = \"keyring\", \
             or turn durable sessions off with [session] enabled = false",
            "storage.credentials.backend",
            "\"keyring\"",
        ),
        // header AFTER the assignment, same line (prose remedy)
        (
            "Anvil is disabled (set `enabled = true` under `[anvil]`)",
            "anvil.enabled",
            "true",
        ),
        // the prose `to` form -- case 5's pre-fix wording
        (
            "Configure an OS keyring, or set `credentials.backend` to \"encrypted-file\"",
            "credentials.backend",
            "\"encrypted-file\"",
        ),
    ];

    for (text, want_path, want_value) in cases {
        let got = advertised_assignments(text, "t.rs", 1, "test");
        assert!(
            got.iter()
                .any(|a| a.path == *want_path && a.value == *want_value),
            "extractor did not recover `{want_path} = {want_value}` from:\n{text}\ngot: {got:#?}"
        );
    }

    // and the false positive that a line-scoped rule produced must NOT reappear
    let two_sections = "set [storage.credentials] backend = \"keyring\", or turn durable \
                        sessions off with [session] enabled = false";
    let got = advertised_assignments(two_sections, "t.rs", 1, "test");
    assert!(
        !got.iter().any(|a| a.path == "session.backend"),
        "line-scoped binding regression: `backend` bound to [session], a key that \
         does not exist. This reds the gate on CORRECT text. got: {got:#?}"
    );
    assert!(
        got.iter().any(|a| a.path == "session.enabled"),
        "the second assignment must still be recovered: {got:#?}"
    );
}

// ---------------------------------------------------------------- case 2

/// Markers for a surface that is advertised and cannot succeed.
///
/// Same list `lane/false-advertising` used for its class scan, which established
/// that `--skills-promote` was the only member at the time.
const DEAD_END_MARKERS: &[&str] = &[
    "temporarily unavailable",
    "not yet implemented",
    "is not implemented",
    "coming soon",
    "unimplemented!(",
    "todo!(",
];

fn kebab(field: &str) -> String {
    format!("--{}", field.replace('_', "-"))
}

/// `23A-C1`: a flag that appears in `--help` and can never succeed.
///
/// Finds every function in `wcore-cli/src` whose entire body is an unconditional
/// dead end, walks back to the clap field its call site reads, and asserts that
/// flag is not advertised in the real binary's `--help`.
///
/// This drives `CARGO_BIN_EXE_wayland-core` — the actual built binary, with clap
/// doing the actual rendering — not a reconstructed `clap::Command`, because a
/// reconstruction can disagree with what ships.
#[test]
fn no_unconditional_dead_end_is_reachable_from_an_advertised_flag() {
    let cli_src = workspace_root().join("crates/wcore-cli/src");
    let mut sources = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    walk(&cli_src, &mut sources);

    let fn_re =
        Regex::new(r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)\s*\(").unwrap();

    // fn name -> the dead-end message it always returns
    let mut dead_ends: Vec<(String, String)> = Vec::new();
    let mut file_bodies: Vec<(PathBuf, String)> = Vec::new();

    for path in &sources {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        let src = strip_test_modules(&raw);
        for cap in fn_re.captures_iter(&src) {
            let name = cap[1].to_string();
            let after = cap.get(0).unwrap().end();
            let Some(open_rel) = src[after..].find('{') else {
                continue;
            };
            let open = after + open_rel;
            let bytes = src.as_bytes();
            let mut depth = 0i32;
            let mut i = open;
            while i < bytes.len() {
                match bytes[i] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let body = &src[open + 1..i.min(src.len())];
            // Strip comments, then require the WHOLE body to be one bail/todo.
            // A conditional dead end (a real feature that refuses in some
            // states) is not false advertising and must not be flagged.
            let stripped: String = body
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.starts_with("//") && !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let is_unconditional = stripped.starts_with("anyhow::bail!")
                || stripped.starts_with("bail!")
                || stripped.starts_with("unimplemented!")
                || stripped.starts_with("todo!");
            if !is_unconditional {
                continue;
            }
            if DEAD_END_MARKERS.iter().any(|m| stripped.contains(m)) {
                dead_ends.push((name, stripped.clone()));
            }
        }
        file_bodies.push((path.clone(), src));
    }

    assert!(
        !dead_ends.is_empty(),
        "no unconditional dead-end handler was found anywhere in wcore-cli/src. \
         Either the class is genuinely gone -- in which case delete this test \
         deliberately, with a note -- or the detector is broken and this test is \
         passing vacuously. `run_skills_promote` was the known member (23A-C1)."
    );

    // Associate each dead end with the clap field its call site reads.
    let field_re = Regex::new(r"(?:cli|args|self)\.([a-z_][a-z0-9_]*)").unwrap();
    let mut advertised_dead: Vec<(String, String)> = Vec::new();
    for (name, _msg) in &dead_ends {
        let call = format!("{name}(");
        for (_p, src) in &file_bodies {
            let mut from = 0usize;
            while let Some(rel) = src[from..].find(&call) {
                let at = from + rel;
                let win_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let win_start = src[..win_start]
                    .rfind('\n')
                    .and_then(|i| src[..i].rfind('\n'))
                    .unwrap_or(win_start);
                for c in field_re.captures_iter(&src[win_start..at]) {
                    advertised_dead.push((name.clone(), kebab(&c[1])));
                }
                from = at + call.len();
            }
        }
    }
    advertised_dead.sort();
    advertised_dead.dedup();
    assert!(
        !advertised_dead.is_empty(),
        "found {} unconditional dead end(s) but could not associate ANY of them \
         with a CLI field, so nothing is actually checked. Fix the association; \
         do not delete the assert. dead ends: {:?}",
        dead_ends.len(),
        dead_ends.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );

    let out = Command::new(env!("CARGO_BIN_EXE_wayland-core"))
        .arg("--help")
        .output()
        .expect("the built binary must run");
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Sanity control: a `--help` that broke entirely would make every "absent"
    // assertion below pass for the wrong reason.
    assert!(
        help.len() > 2000 && help.contains("--skills-audit"),
        "`--help` did not render a real help page ({} bytes); every absence check \
         below would pass vacuously",
        help.len()
    );

    let offenders: Vec<String> = advertised_dead
        .iter()
        .filter(|(_, flag)| help.contains(flag.as_str()))
        .map(|(f, flag)| {
            format!("`{flag}` is advertised in --help but {f}() is an unconditional dead end")
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "advertised-but-dead CLI surface ({} checked):\n{}",
        advertised_dead.len(),
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------- doc paths

/// A remediation that cites a document must cite one that exists.
#[test]
fn advertised_doc_paths_exist() {
    let doc_re = Regex::new(r"\b((?:docs/[\w./-]+\.md)|README\.md)").unwrap();
    let root = workspace_root();
    let mut checked = 0usize;
    let mut missing = Vec::new();
    for path in production_sources() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let src = strip_test_modules(&raw);
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        for (line, text) in string_literals(&src) {
            for cap in doc_re.captures_iter(&text) {
                let cited = cap[1].to_string();
                checked += 1;
                if !root.join(&cited).exists() {
                    missing.push(format!(
                        "{rel}:{line} cites `{cited}`, which does not exist"
                    ));
                }
            }
        }
    }
    assert!(
        checked > 0,
        "no document citation was extracted; the check is vacuous"
    );
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}
