//! A remediation string that names something the product cannot honour must
//! fail CI, not ship.
//!
//! # Why this file exists
//!
//! Seven separate defects on this program share exactly one shape: **the product
//! tells the operator to do something that does not work.** Every one of the
//! first five was found by a human driving the real binary; none was visible
//! from source review, because the string and its consumer live in different
//! crates and nothing ever compared them.
//!
//! | # | ledger | advertised | why it was dead |
//! |---|---|---|---|
//! | 1 | `27-C2(a)` | `[browser] allowed_origins = [..]` | loader reads `browser.policy.*`; the key parsed cleanly and was **silently discarded** |
//! | 2 | `23A-C1` | `--skills-promote` in `--help` | handler is an unconditional `bail!` — always exits 1 |
//! | 3 | `24-C2` | `--trigger webhook:` / `poll:` | accepted at add, never fired |
//! | 4 | ollama hint | "select a model id prefixed `ollama:` — no API key is needed" | credential resolution returns `MissingApiKey` before the model string is read |
//! | 5 | headless keyring | `credentials.backend = "encrypted-file"` | wrong section, unparseable value, struct variant, and a passphrase mechanism named in 0 docs / 0 help / 0 errors |
//! | 6 | *(none — found by this file)* | `wayland-core init --model X` wrote root-level `model = "X"` | loader reads `default.model`; dropped in silence, so the model the operator chose never took effect |
//! | 7 | *(handed over mid-lane)* | "download Piper voices via `piper_download`" | `piper_download` is a module name, not a tool; nothing by that name exists at runtime |
//!
//! Case 6 was found by this file, on its first green run, with no prior report
//! to work from. That is the only evidence worth much: the five historical cases
//! were known before the gate was written, so catching them proves the gate was
//! aimed correctly, not that it sees.
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
//! * **Tool REACHABILITY.** Case 7 was dead four ways; the tool check reads only
//!   the first (the name does not exist). A tool that is named, registered, and
//!   then returns `None` unconditionally still passes. See the coverage note on
//!   `advertised_tool_names_resolve_to_a_real_tool`.
//!
//! The two STILL-GREEN rows in `.planning/evidence/remedy-gate/mutate.py` exist
//! so these limits are **measured** rather than asserted: they re-introduce
//! cases 3 and 4 and record that the gate does not see them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

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
    //
    // The optional backticks are load-bearing, not cosmetic: shipped remediation
    // text markdown-quotes the key ("set `credentials.backend` to ..."), and a
    // pattern without the CLOSING backtick recovered nothing at all from case
    // 5's own pre-fix string -- measured, this test file's first run.
    Regex::new(
        r#"(?:^|[^\w.\-])(`?)([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)`?\s*(=|\sto\s)\s*`?("[^"]*"|\[[^\]]*\]|true|false|-?\d+)"#,
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
    /// The whole string that carries this assignment. Kept because an operator
    /// pastes the WHOLE snippet, not one key: checking keys one at a time
    /// reported `missing field transport` against a snippet that supplies
    /// `transport` two lines down. Also used for the admission rule.
    carrier: String,
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
        let backtick = !cap[1].is_empty();
        let key = cap[2].to_string();
        let is_to_form = cap[3].trim() == "to";
        let value = cap[4].to_string();
        let at = cap.get(2).unwrap().start();

        // The prose `X to "Y"` form is loose enough to swallow ordinary English:
        // the shipped string "storage.credentials.backend is set to \"plaintext\""
        // yielded the key `set`, which then bound to a nearby header and reported
        // a config key nobody ever advertised. So the `to` form requires a key
        // that is dotted or explicitly backticked -- and case 5's real pre-fix
        // wording ("set `credentials.backend` to ...") was both.
        if is_to_form && !key.contains('.') && !backtick {
            continue;
        }

        let before = heads.iter().rfind(|(s, _, _)| *s < at);
        // The forward fallback applies ONLY to a bare key. A dotted key is
        // already self-qualifying, and prefixing it with a section that happens
        // to appear later in the same sentence invents a path nobody advertised:
        // reverting case 5 produced `session.credentials.backend` because the
        // string's second clause names `[session]`. The verdict was right by
        // luck there (both paths are dropped), but the same mis-binding on
        // CORRECT text would red the gate for a key that is perfectly fine.
        let same_line_after = if key.contains('.') {
            None
        } else {
            heads
                .iter()
                .find(|(s, _, _)| *s >= whole.end() && line_start(*s) == line_start(at))
        };

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
            carrier: text.to_string(),
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

/// Every full dotted path the real schema contains.
fn schema_paths(v: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
    if let Some(obj) = v.as_object() {
        for (k, child) in obj {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            out.insert(path.clone());
            schema_paths(child, &path, out);
        }
    }
}

/// Is `path` a real schema path that has LOST its leading section(s)?
///
/// This is the defect shape itself, stated as a predicate. The headless-keyring
/// HIGH advertised `credentials.backend`; the schema has
/// `storage.credentials.backend`. The advertised path is a genuine suffix of a
/// genuine path, missing only its root — which is precisely why it looked right
/// to whoever wrote the message and why the loader ignored it.
///
/// Without this clause that case is admitted only if the surrounding sentence
/// happens to contain the word "config". Measured: it was passing for exactly
/// that reason under an earlier revision, and a paraphrase of the same message
/// would have slipped straight through.
fn is_truncated_schema_path(path: &str, paths: &BTreeSet<String>) -> bool {
    if !path.contains('.') {
        return false;
    }
    let tail = format!(".{path}");
    paths.iter().any(|p| p.ends_with(&tail))
}

/// Resolve a dotted path, descending into arrays.
///
/// Arrays matter: `[[hooks.pre_tool_use]]` is an array-of-tables, so the path
/// `hooks.pre_tool_use.command` has to look INSIDE an element. Treating an array
/// as a dead end reported three shipped hook snippets as unhonourable when they
/// are correct.
fn json_at<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    fn go<'a>(cur: &'a serde_json::Value, segs: &[&str]) -> Option<&'a serde_json::Value> {
        let Some((head, rest)) = segs.split_first() else {
            return Some(cur);
        };
        match cur {
            serde_json::Value::Array(items) => items.iter().find_map(|it| go(it, segs)),
            _ => go(cur.as_object()?.get(*head)?, rest),
        }
    }
    go(v, &path.split('.').collect::<Vec<_>>())
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
    let mut paths = BTreeSet::new();
    schema_paths(&schema, "", &mut paths);

    let mut checked = 0usize;
    let mut illustrative = 0usize;
    let mut illustrative_reasons: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    // Group by the carrying string. An operator pastes a whole snippet, so the
    // snippet is the unit under test: checking `mcp.servers.x.command` alone
    // reported `missing field transport` against a snippet that supplies
    // `transport` two lines further down -- a defect of the instrument, not the
    // product.
    let mut groups: Vec<(String, Vec<Advertised>)> = Vec::new();
    for adv in sweep_advertised() {
        match groups.iter_mut().find(|(c, _)| *c == adv.carrier) {
            Some((_, v)) => v.push(adv),
            None => groups.push((adv.carrier.clone(), vec![adv])),
        }
    }

    for (carrier, advs) in groups {
        // Three admission rules, in descending order of confidence. All three
        // exist because a rule that admits too much reds the gate on correct
        // text and gets deleted, and a rule that admits too little is a gate
        // that fires on nothing.
        //
        //   A. the ROOT is a real `ConfigFile` field. The path is about this
        //      product's config, so any depth or value error inside it is real.
        //      Admits case 1 (`browser.allowed_origins`) and case 6 is not this.
        //   B. the path is a real schema path with its leading section(s) LOST
        //      (`credentials.backend` vs `storage.credentials.backend`). That is
        //      the headless-keyring defect stated as a predicate, and it does not
        //      depend on how the sentence around it is worded.
        //   C. the LEAF name is one the schema knows AND the carrying string is
        //      talking about configuration. The weakest rule, and the only one
        //      that reaches a bare key: it is what admits case 6's root-level
        //      `model` from the `init` template.
        //
        // Without C's second half, `[server] port = 8080` (a mock server in an
        // eval scenario) and a cron tool's `skills=[]` parameter doc get admitted
        // and red the gate on text that is perfectly correct.
        let carrier_is_config_flavoured = carrier.to_lowercase().contains("config");
        let admitted: Vec<&Advertised> = advs
            .iter()
            .filter(|a| {
                let root = a.path.split('.').next().unwrap_or_default();
                let leaf = a.path.rsplit('.').next().unwrap_or_default();
                roots.contains(root)
                    || is_truncated_schema_path(&a.path, &paths)
                    || (carrier_is_config_flavoured && leaves.contains(leaf))
            })
            .collect();
        if admitted.is_empty() {
            continue;
        }
        let adv = admitted[0];

        // Prefer the carrier VERBATIM when it is itself a well-formed TOML
        // document (pasteable snippets: the browser hint, the `init` template,
        // the eval-scenario configs). That preserves array-of-table syntax and
        // sibling required fields, which a reconstruction destroys. Fall back to
        // reconstructing dotted keys for prose remedies, which are not TOML.
        // `toml::from_str::<toml::Value>` and NOT `str::parse::<toml::Value>()`:
        // under toml 1.x the `FromStr` impl rejected every candidate, so the
        // first run of this gate skipped 36 of 36 as "illustrative" and checked
        // ZERO. The anti-vacuity floor below is the only reason that was visible
        // instead of shipping as a green.
        let reconstructed = admitted
            .iter()
            .map(|a| format!("{} = {}", a.path, a.value))
            .collect::<Vec<_>>()
            .join("\n");
        let (snippet, generic) = match toml::from_str::<toml::Value>(&carrier) {
            Ok(v) => (carrier.clone(), v),
            Err(_) => match toml::from_str::<toml::Value>(&reconstructed) {
                Ok(v) => (reconstructed.clone(), v),
                Err(e) => {
                    // Illustrative text (`proposers = ["provider", ...]`) is not
                    // literal TOML. Skipping it is safe ONLY because generic
                    // TOML rejected it: if generic TOML accepts it and
                    // `ConfigFile` does not, that is exactly the case 5(b)/(c)
                    // defect and it reds below.
                    illustrative_reasons.push(format!("{reconstructed} :: {e}"));
                    illustrative += admitted.len();
                    continue;
                }
            },
        };
        checked += admitted.len();

        let as_generic = serde_json::to_value(&generic).ok();

        match toml::from_str::<ConfigFile>(&snippet) {
            Err(e) => failures.push(format!(
                "{}:{} [{}] advertises\n    {}\n  -- the real loader REJECTS it, so \
                 an operator who follows this text ends up with a config that will \
                 not load AT ALL, which is strictly worse than ignoring the advice: \
                 {}",
                adv.file,
                adv.line,
                adv.context,
                snippet.replace('\n', "\n    "),
                e
            )),
            Ok(cfg) => {
                let round = serde_json::to_value(&cfg).expect("ConfigFile -> JSON");
                for a in &admitted {
                    match json_at(&round, &a.path) {
                        None => failures.push(format!(
                            "{}:{} [{}] advertises `{} = {}` -- it parses without \
                             error and is then SILENTLY DISCARDED: `{}` does not \
                             exist in the schema the loader reads. The operator gets \
                             NO diagnostic at all and the setting never takes effect.",
                            a.file, a.line, a.context, a.path, a.value, a.path
                        )),
                        Some(got) => {
                            let want = as_generic.as_ref().and_then(|v| json_at(v, &a.path));
                            if let Some(want) = want
                                && got != want
                            {
                                failures.push(format!(
                                    "{}:{} [{}] advertises `{} = {}` but the \
                                     loader ends up holding {} at `{}`",
                                    a.file, a.line, a.context, a.path, a.value, got, a.path
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
    eprintln!(
        "remedy-gate: {checked} advertised config assignments checked, \
         {illustrative} skipped as illustrative"
    );
    for r in illustrative_reasons.iter().take(20) {
        eprintln!("  illustrative: {r}");
    }
    assert!(
        checked >= 15,
        "only {checked} advertised config assignments were checked ({illustrative} \
         skipped as illustrative). The sweep is broken -- fix the extraction, do \
         NOT lower this floor. skips: {:#?}",
        illustrative_reasons
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

    // case 5(a): `credentials` is not a section at all. Admission rule B has to
    // recognise it as a real schema path that lost its root, because rule A
    // cannot (there is no `credentials` root) and rule C only fires if the
    // surrounding sentence happens to contain the word "config". Measured: for
    // one revision this case was admitted purely by the accident of a nearby
    // `[session]` clause, which is not a property anyone should rely on.
    let mut paths = BTreeSet::new();
    schema_paths(&schema(), "", &mut paths);
    assert!(
        paths.contains("storage.credentials.backend"),
        "the schema path this case is a truncation OF must exist, or the \
         predicate below is vacuous"
    );
    assert!(
        is_truncated_schema_path("credentials.backend", &paths),
        "case 5(a) must be admitted on its own shape -- a real schema path \
         missing its leading section -- not on the wording around it"
    );
    assert!(
        !is_truncated_schema_path("core.bare", &paths),
        "git config must NOT be admitted; that reds the gate on correct text"
    );

    let pre_fix_creds = "credentials.backend = \"encrypted-file\"";
    assert!(
        toml::from_str::<toml::Value>(pre_fix_creds).is_ok(),
        "must be well-formed TOML, else the main check would skip it as \
         illustrative rather than red on it"
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
        // case 5's pre-fix wording IN FULL: a dotted key whose sentence goes on
        // to name an unrelated section. The dotted key is self-qualifying and
        // must NOT absorb `[session]`. Measured: it did, yielding
        // `session.credentials.backend`, a path nobody ever advertised.
        (
            "Configure an OS keyring, or set `credentials.backend` to \
             \"encrypted-file\", or turn durable sessions off with [session] \
             enabled = false",
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

/// Find every function in `src` whose **entire** body is one unconditional dead end
/// carrying a `DEAD_END_MARKERS` phrase. Returns `(fn name, stripped body)`.
///
/// Extracted from the test body in 23A-C1 so the detector can be exercised against a
/// synthetic fixture. While it was inline, the only evidence it worked was that it
/// happened to find a real defect — so the moment the last defect was fixed, the
/// instrument became both untested and red.
fn scan_dead_ends(src: &str) -> Vec<(String, String)> {
    let fn_re =
        Regex::new(r"(?m)^\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)\s*\(").unwrap();
    let mut found = Vec::new();
    for cap in fn_re.captures_iter(src) {
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
        // Strip comments, then require the WHOLE body to be one bail/todo. A
        // conditional dead end (a real feature that refuses in some states) is not
        // false advertising and must not be flagged.
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
        if is_unconditional && DEAD_END_MARKERS.iter().any(|m| stripped.contains(m)) {
            found.push((name, stripped));
        }
    }
    found
}

/// Anti-vacuity for the dead-end scan, per the standing rule that a repaired
/// instrument needs three assertions rather than two.
#[test]
fn dead_end_detector_self_test() {
    // 1. KNOWN-POSITIVE — an unconditional dead end is found.
    let positive = r#"
        async fn run_something(_id: &str) -> anyhow::Result<()> {
            anyhow::bail!("this feature is temporarily unavailable while it is built")
        }
    "#;
    let hits = scan_dead_ends(positive);
    assert_eq!(
        hits.len(),
        1,
        "detector missed an unconditional dead end; it cannot police anything. got {hits:?}"
    );
    assert_eq!(hits[0].0, "run_something");

    // 2. KNOWN-NEGATIVE — a *conditional* refusal is a working feature, not false
    //    advertising, and must not be reported.
    let negative = r#"
        async fn run_conditional(id: &str) -> anyhow::Result<()> {
            if id.is_empty() {
                anyhow::bail!("this feature is temporarily unavailable for empty ids")
            }
            Ok(())
        }
    "#;
    assert!(
        scan_dead_ends(negative).is_empty(),
        "detector flagged a conditional refusal; every feature with a guard clause \
         would be reported as an advertised dead end"
    );

    // 3. THE ASSERTION THAT PROVES THE DETECTOR DOES ANY WORK. The naive matcher —
    //    "the source contains a dead-end marker" — reports the conditional case too.
    //    Without this, assertions 1 and 2 both pass on a detector that is really just
    //    a substring search plus luck.
    let naive_flags_the_negative = DEAD_END_MARKERS.iter().any(|m| negative.contains(m));
    assert!(
        naive_flags_the_negative,
        "the naive matcher must actually be wrong on this fixture, or assertion 3 is \
         vacuous and this self-test would pass on the broken instrument too"
    );
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

    let mut dead_ends: Vec<(String, String)> = Vec::new();
    let mut file_bodies: Vec<(PathBuf, String)> = Vec::new();

    for path in &sources {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        let src = strip_test_modules(&raw);
        dead_ends.extend(scan_dead_ends(&src));
        file_bodies.push((path.clone(), src));
    }

    // The class is empty as of 23A-C1, and that is the goal state rather than a
    // problem. `run_skills_promote` was the last member: it is now real governed
    // promotion, and a marker sweep over `wcore-cli/src` returns 0 for all six
    // `DEAD_END_MARKERS` against a live 110-hit `bail!` control.
    //
    // **The old `assert!(!dead_ends.is_empty())` has been REPLACED, not removed.**
    // It existed to stop the scan passing vacuously, and that job is real — but it
    // discharged it by requiring a genuine defect to exist in the tree, so fixing the
    // last defect turned a working instrument red and the file's own header invited
    // deleting it. Deleting a class scanner because the class is briefly empty is how
    // the class comes back unnoticed.
    //
    // Anti-vacuity now lives in `dead_end_detector_self_test`, which proves the
    // detector fires on a synthetic dead end, stays silent on a conditional refusal,
    // and disagrees with the naive matcher. That is a stronger guarantee: the old form
    // could not distinguish "detector works" from "detector is broken but some other
    // defect happened to match".
    if dead_ends.is_empty() {
        return;
    }

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

// ---------------------------------------------------------------- tool names

/// Every tool name the workspace actually defines.
///
/// Taken from the `Tool::name` implementations themselves — the one string the
/// model and the registry both key on — rather than from a hand-kept list,
/// which would go stale the first time somebody added a tool.
fn real_tool_names() -> BTreeSet<String> {
    let re = Regex::new(r#"fn\s+name\s*\(\s*&self\s*\)\s*->\s*&(?:'\w+\s+)?str\s*\{\s*"([^"]+)""#)
        .unwrap();
    let mut out = BTreeSet::new();
    // Deliberately NOT restricted to production sources: a tool that only a
    // test defines is still a real name, and counting it can only make this
    // check more conservative (fewer reports), never more trigger-happy.
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let n = e.file_name();
            if p.is_dir() {
                if n == "target" || n == ".git" {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&workspace_root().join("crates"), &mut files);
    for f in files {
        if let Ok(src) = std::fs::read_to_string(&f) {
            for cap in re.captures_iter(&src) {
                out.insert(cap[1].to_string());
            }
        }
    }
    out
}

/// Tool names a string tells the operator to reach for.
///
/// See the narrowness note on
/// [`advertised_tool_names_resolve_to_a_real_tool`]; the shape of this pattern
/// is a measurement, not a guess.
fn advertised_tool_mentions(text: &str) -> Vec<String> {
    // Compiled ONCE, not per call.
    //
    // This function runs once per string literal, and
    // `advertised_tool_names_resolve_to_a_real_tool` sweeps every string
    // literal in all ~1224 production sources. Building these two regexes on
    // each call put a full regex compilation on every literal in the workspace.
    //
    // Measured on an idle box before this change: `real_tool_names()` 227ms,
    // source discovery 6ms, and the scan loop **68.67s** of a 68.90s test —
    // i.e. essentially the whole runtime, and it was regex CONSTRUCTION rather
    // than matching. That put the test over nextest's 60s terminate budget, so
    // its CI outcome was decided by runner speed rather than by the code.
    static VIA: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bvia\s+`?([a-z][a-z0-9]*(?:_[a-z0-9]+)+)`?").unwrap());
    static TICKED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:use|using|run|invoke|call)\s+`([a-z][a-z0-9]*(?:_[a-z0-9]+)+)`").unwrap()
    });
    VIA.captures_iter(text)
        .chain(TICKED.captures_iter(text))
        .map(|c| c[1].to_string())
        .collect()
}

/// The extractor must read the phrasing that actually shipped, and must not
/// read the phrasings that would flood it.
///
/// This control carries the anti-vacuity weight for the corpus sweep, because
/// the corpus itself is now down to a single mention — this lane's own fix
/// removed the other two. A regex that stops matching fails HERE, loudly,
/// whatever the tree happens to contain.
#[test]
fn tool_mention_extraction_reads_the_phrasing_that_shipped() {
    // Case 7's string, verbatim as it shipped.
    let shipped = "tts: no TTS backend configured — set OPENAI_API_KEY or \
                   ELEVENLABS_API_KEY (or download Piper voices via piper_download). \
                   Tool hidden.";
    assert_eq!(
        advertised_tool_mentions(shipped),
        vec!["piper_download".to_string()],
        "the extractor no longer reads the phrasing this check was built for"
    );
    // ...and the name it advertises really is absent from the tool set, which is
    // what made it a defect rather than a stylistic quibble.
    assert!(
        !real_tool_names().contains("piper_download"),
        "`piper_download` resolves as a tool now. If a real tool by that name \
         was added, delete this control deliberately -- do not leave it asserting \
         something false."
    );

    // The rejected phrasings. Every one of these is a real string from this
    // workspace that an earlier, broader pattern picked up as a "tool": 11 false
    // positives to 1 true one. They must stay rejected.
    for noise in [
        "Retrying with a different finish_reason",
        "pass the job_id you got from list",
        "the access_token expired",
        "use replace_all to change every occurrence",
        "Use the `mcp_echo` tool to echo back this exact string",
    ] {
        assert!(
            advertised_tool_mentions(noise).is_empty(),
            "extractor widened and now reads struct fields as tools: {noise:?} -> \
             {:?}",
            advertised_tool_mentions(noise)
        );
    }
}

/// A remediation that tells an operator to reach for a TOOL must name a tool
/// that exists.
///
/// Case 7, handed over mid-lane: `tool_backends/tts.rs` told every keyless user
/// to "download Piper voices via `piper_download`". That name is not a tool. It
/// is a *module* (`wcore_tools::piper_download`) and a builder function that no
/// production caller invokes, so the thing the message names has no runtime
/// existence at all. Three further defects sit behind it — see the coverage
/// note below.
///
/// # Extraction is deliberately narrow, and that is measured
///
/// A sweep for "any snake_case token after any verb" produced 16 candidates on
/// this workspace of which 11 were struct fields and JSON keys (`access_token`,
/// `finish_reason`, `max_tokens`, `job_id`, `replace_all`, …). At 11 false
/// positives to 1 true one it would have been deleted within a week.
///
/// Restricting to `via <name>` (plus a backticked form after the invocation
/// verbs) gives 3 extractions on the same corpus: one real tool (`meet_say`,
/// which resolves), and the two Piper mentions, which do not. Zero false
/// positives. Narrow and true beats broad and noisy — but it IS narrow, and the
/// phrasings it does not read are listed in `.planning/REMEDY-GATE.md`.
///
/// # What this check does NOT reach
///
/// Name existence only. The Piper defect was dead in four independent ways and
/// this catches the first:
///   1. **caught** — `piper_download` is not a tool name anywhere;
///   2. not caught — `build_piper_tts_backend()` returns `None` unconditionally;
///   3. not caught — `PiperTtsBackend::synthesize` is a stub returning
///      `DependencyMissing`;
///   4. not caught — `piper_tts` is a non-default feature, so the branch is not
///      even compiled into the shipped binary.
///
/// (2) and (3) are the same *unconditional dead end* shape that
/// `no_unconditional_dead_end_is_reachable_from_an_advertised_flag` models for
/// CLI flags; extending that detector past `wcore-cli` and past `bail!` to a
/// bare `Err(..)` return is the obvious next step and is NOT done here. Do not
/// read this test as proving a tool is reachable — only that it is named.
#[test]
fn advertised_tool_names_resolve_to_a_real_tool() {
    let tools = real_tool_names();
    assert!(
        tools.len() > 50,
        "only {} tool names were recovered from the workspace; the extractor is \
         broken and every name below would be reported missing",
        tools.len()
    );

    let root = workspace_root();
    let mut checked = 0usize;
    let mut resolved = 0usize;
    let mut missing: Vec<String> = Vec::new();

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
            for name in advertised_tool_mentions(&text) {
                checked += 1;
                if tools.contains(&name) {
                    resolved += 1;
                } else {
                    missing.push(format!(
                        "{rel}:{line} tells the operator to reach for `{name}`, which is \
                         not a tool this product defines. Nothing by that name can be \
                         called at runtime."
                    ));
                }
            }
        }
    }

    eprintln!("remedy-gate: {checked} advertised tool names, {resolved} resolved");

    // Anti-vacuity, in two parts.
    //
    // The corpus floor is 1, not a larger number, and deliberately so: this
    // lane's own fix removed two of the three `via <tool>` mentions that existed
    // when the pattern was calibrated. Raising a floor above what the corpus can
    // supply would make the gate red for a reason unconnected to any defect --
    // and lowering a floor to reach green is exactly the move this program has
    // been burned by, so the weight is moved rather than removed.
    //
    // It moves onto `tool_mention_extraction_reads_the_phrasing_that_shipped`,
    // which pins the extractor against literal known input. The corpus count can
    // now fall to 1 without the check becoming meaningless, because a broken
    // regex fails that control regardless of what the tree happens to contain.
    assert!(
        checked >= 1,
        "no advertised tool name was extracted from the whole workspace. Either \
         the pattern broke or the phrasing vanished entirely; check \
         tool_mention_extraction_reads_the_phrasing_that_shipped first."
    );
    // The resolution side must be exercised too. A run where NOTHING resolves
    // would mean `real_tool_names()` is returning junk, and every report below
    // would be an artefact rather than a finding.
    assert!(
        resolved >= 1,
        "no advertised tool name resolved against the real tool set ({checked} \
         extracted, {} names known). The resolver, not the product, is broken.",
        tools.len()
    );

    assert!(
        missing.is_empty(),
        "{} advertised tool name(s) that do not exist ({checked} checked, \
         {resolved} resolved):\n{}",
        missing.len(),
        missing.join("\n")
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
