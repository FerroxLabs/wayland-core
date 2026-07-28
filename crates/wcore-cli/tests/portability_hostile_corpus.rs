//! F26-05 — the adversarial corpora, and the assertion that the product
//! produced each case's DECLARED outcome rather than merely surviving it.
//!
//! # Why every case here asserts a declared outcome
//!
//! Everything phase 26 built before this file was proven against cooperative
//! input: 26-01's corpora clone the SHAPE of real installs, 26-02's fixtures are
//! realistic and executable, 26-03's homes are synthetic and well formed. All
//! honest inputs. A migration path is precisely where a hostile source arrives,
//! and a hostile case whose only assertion is that the process exited PASSES
//! when the product silently does the wrong thing. So every case carries its
//! expected outcome as DATA in `scripts/portability-hostile-gen.py`, this suite
//! reads that data, binds itself to it (`assert_eq!(case.expect, …)`), and then
//! asserts the class-specific predicate that makes the outcome checkable.
//!
//! # Why the corpora are generated here rather than committed
//!
//! Two of the three platforms this product ships to collapse names the
//! authoritative Linux host treats as distinct. A committed corpus is a corpus
//! whose case-only and normal-form-only distinctions were destroyed by whichever
//! filesystem last checked it out. The generator materialises on the target
//! platform and VERIFIES afterwards; see its module docs.
//!
//! # The convention this follows
//!
//! `crates/wcore-fixture-harness/src/lib.rs` establishes the archetype model:
//! a sanitised snapshot of a `$WAYLAND_HOME`, the engine binary SPAWNED against
//! it, and assertions on the emitted document, on stderr cleanliness and on a
//! post-run state diff. That crate's catalog/playback/replay are a Wave 1
//! skeleton and are NOT built, so this file follows the convention without
//! calling an API that does not exist. Its sanitisation rule is honoured
//! absolutely: every secret in every corpus is a synthetic canary, and no real
//! peer home is read on any host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// The declared-outcome data
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct CaseEntry {
    id: String,
    class: String,
    deforms: String,
    attacks: String,
    /// The DECLARED outcome. One of `imported`, `quarantined`, `refused`,
    /// `conflict`. Nothing else is legitimate and a test asserts that.
    expect: String,
    /// `portable` (cross-compared between platforms) or `platform`.
    scope: String,
    note: String,
    corpus: PathBuf,
    /// Every symlink the generator was asked to create, or
    /// `<unlinkable:…>` where the platform refused. An escape case whose link
    /// could not be created tested nothing, and a test asserts that.
    symlinks: Vec<String>,
    entries: usize,
    corpus_digest: String,
    collapsed: bool,
    collapsed_pairs: Vec<Vec<String>>,
    require_distinct_on: Vec<String>,
    platform: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    platform: String,
    canaries: Vec<String>,
    baseline_profile: String,
    cases: Vec<CaseEntry>,
}

const LEGITIMATE_OUTCOMES: [&str; 4] = ["imported", "quarantined", "refused", "conflict"];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    // crates/wcore-cli -> crates -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn generator() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("portability-hostile-gen.py")
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Materialise one hostile corpus, on THIS platform, right now.
///
/// The generator's own post-creation verification runs here: a case whose
/// declared distinction collapsed on a platform that requires it makes the
/// generator exit non-zero, and this turns that into a red test rather than a
/// silently reduced corpus.
fn materialise(case_id: &str) -> (TempDir, Manifest) {
    let out = tempfile::tempdir().expect("tempdir");
    let script = generator();
    assert!(
        script.is_file(),
        "the hostile generator is missing at {}",
        script.display()
    );
    let run = Command::new("python3")
        .arg(&script)
        .arg("--out")
        .arg(out.path())
        .arg("--only")
        .arg(case_id)
        .output()
        .expect("python3 must be available to materialise a hostile corpus");
    assert!(
        run.status.success(),
        "hostile generator failed for case {case_id}: status={:?}\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let raw = std::fs::read_to_string(out.path().join("cases.json")).expect("cases.json");
    let manifest: Manifest = serde_json::from_str(&raw).expect("cases.json parses");
    assert_eq!(
        manifest.cases.len(),
        1,
        "--only {case_id} must materialise exactly one corpus"
    );
    (out, manifest)
}

fn case_of(manifest: &Manifest) -> CaseEntry {
    manifest.cases[0].clone()
}

/// A completed run of the REAL binary.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
    /// The product must never reach a hostile input by panicking. A panic is a
    /// refusal the operator cannot act on and a stack trace they should never
    /// see, and it is the single most likely thing malformed input produces.
    fn assert_no_panic(&self, case: &str) {
        for marker in ["panicked at", "stack backtrace", "RUST_BACKTRACE"] {
            assert!(
                !self.stderr.contains(marker),
                "case {case}: the product PANICKED on hostile input ({marker}).\n\
                 stderr:\n{}",
                self.stderr
            );
        }
    }
    fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// The target `$WAYLAND_HOME` a hostile import is pointed at.
struct Target {
    dir: TempDir,
    /// A sentinel tree OUTSIDE the target home. Isolation is proven by what did
    /// NOT change out here, not by asserting the code contains a path check.
    sentinel: TempDir,
}

impl Target {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("target home");
        let sentinel = tempfile::tempdir().expect("sentinel");
        // Shapes a hostile source would want to reach: a credential-looking
        // file, a nested tree, and a file whose name invites an overwrite.
        std::fs::create_dir_all(sentinel.path().join("nested/deeper")).unwrap();
        std::fs::write(
            sentinel.path().join("credentials.toml"),
            "sentinel-value-do-not-touch\n",
        )
        .unwrap();
        std::fs::write(
            sentinel.path().join("nested/config.toml"),
            "sentinel = true\n",
        )
        .unwrap();
        std::fs::write(
            sentinel.path().join("nested/deeper/SKILL.md"),
            "sentinel skill body\n",
        )
        .unwrap();
        Self { dir, sentinel }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn sentinel_digest(&self) -> String {
        tree_digest(self.sentinel.path())
    }

    /// Seed the target with a Core profile of this name, so a name collision is
    /// a collision with real state rather than with nothing.
    fn seed_profile(&self, name: &str, marker: &str) {
        let cfg = self.home().join("config.toml");
        let mut body = std::fs::read_to_string(&cfg).unwrap_or_default();
        body.push_str(&format!(
            "\n[profiles.{name}]\nprovider = \"anthropic\"\nmodel = \"{marker}\"\n"
        ));
        std::fs::write(&cfg, body).unwrap();
    }

    fn config_toml(&self) -> String {
        std::fs::read_to_string(self.home().join("config.toml")).unwrap_or_default()
    }

    fn run(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(binary());
        cmd.args(args)
            .current_dir(self.home())
            .env("WAYLAND_HOME", self.home())
            .env("HOME", self.home())
            .env("TERM", "dumb");
        for key in [
            "API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "OPENROUTER_API_KEY",
            "XDG_DATA_HOME",
        ] {
            cmd.env_remove(key);
        }
        let out = cmd.output().expect("spawn wayland-core");
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn migrate_json(&self, corpus: &Path) -> Run {
        self.run(&[
            "migrate",
            "hermes",
            "--home",
            corpus.to_str().unwrap(),
            "--json",
        ])
    }

    fn migrate_apply(&self, corpus: &Path) -> Run {
        self.run(&[
            "migrate",
            "hermes",
            "--home",
            corpus.to_str().unwrap(),
            "--yes",
        ])
    }
}

/// A stable digest over a tree: sorted relative paths, `/` separators, symlinks
/// recorded as links rather than followed.
///
/// Deliberately independent of the product's own digest, because the sentinel
/// exists to catch a product that wrote outside its target — and measuring that
/// with the product's own walker would share whatever blind spot let it happen.
fn tree_digest(root: &Path) -> String {
    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let mut items: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(Result::ok).collect(),
            Err(e) => {
                out.insert(
                    format!("<unreadable:{}>", dir.strip_prefix(root).unwrap().display()),
                    e.kind().to_string(),
                );
                return;
            }
        };
        items.sort_by_key(std::fs::DirEntry::path);
        for item in items {
            let path = item.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    out.insert(rel, format!("<stat-error:{}>", e.kind()));
                    continue;
                }
            };
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap_or_default();
                out.insert(rel, format!("L:{}", target.to_string_lossy()));
            } else if meta.is_dir() {
                out.insert(rel.clone(), "D".into());
                walk(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap_or_default();
                let mut h = Sha256::new();
                h.update(&bytes);
                out.insert(rel, format!("F:{:x}", h.finalize()));
            }
        }
    }
    walk(root, root, &mut entries);
    let mut h = Sha256::new();
    h.update(b"wlc-hostile-sentinel-v1\0");
    for (k, v) in &entries {
        h.update(k.as_bytes());
        h.update([0]);
        h.update(v.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())
}

/// `Accounting: discovered=N imported=N quarantined=N excluded=N …`
#[derive(Debug, PartialEq, Eq)]
struct Accounting {
    discovered: usize,
    imported: usize,
    quarantined: usize,
    excluded: usize,
}

impl Accounting {
    fn parse(stdout: &str) -> Option<Self> {
        let line = stdout.lines().find(|l| l.starts_with("Accounting:"))?;
        let get = |k: &str| -> Option<usize> {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix(k))
                .and_then(|v| {
                    v.trim_end_matches(|c: char| !c.is_ascii_digit())
                        .parse()
                        .ok()
                })
        };
        Some(Self {
            discovered: get("discovered=")?,
            imported: get("imported=")?,
            quarantined: get("quarantined=")?,
            excluded: get("excluded=")?,
        })
    }
    fn balances(&self) -> bool {
        self.imported + self.quarantined + self.excluded == self.discovered
    }
}

/// Every canary the generator seeds. A report that carries one has leaked it.
fn assert_no_canary(manifest: &Manifest, haystack: &str, where_: &str) {
    for canary in &manifest.canaries {
        assert!(
            !haystack.contains(canary.as_str()),
            "a hostile-corpus canary ({canary}) reached {where_} — a secret placed \
             where the classifier does not look was emitted verbatim"
        );
    }
}

/// The positive half of every canary-absence claim: prove the canary really IS
/// in the corpus, so "0 hits" measures redaction rather than a corpus that
/// never carried one.
fn assert_canary_present_in_corpus(manifest: &Manifest, corpus: &Path, canary: &str) {
    let mut found = false;
    fn scan(dir: &Path, needle: &str, found: &mut bool) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for item in rd.filter_map(Result::ok) {
            let path = item.path();
            if std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            if path.is_dir() {
                scan(&path, needle, found);
            } else if std::fs::read_to_string(&path)
                .map(|s| s.contains(needle))
                .unwrap_or(false)
            {
                *found = true;
            }
        }
    }
    scan(corpus, canary, &mut found);
    assert!(
        found,
        "the corpus does not actually carry {canary}, so any absence assertion \
         over the report would pass vacuously (canaries declared: {:?})",
        manifest.canaries
    );
}

// ===========================================================================
// META — the guards that keep the rest of this file from being decoration
// ===========================================================================

/// Every case must declare one of the four legitimate outcomes, name the real
/// peer format it deforms, and name what it attacks.
///
/// Without this a case could be added with no declared outcome, and the suite
/// would then assert nothing about it while still counting as coverage.
#[test]
fn hostile_every_case_declares_a_legitimate_outcome_and_what_it_attacks() {
    let out = tempfile::tempdir().unwrap();
    let spec = out.path().join("spec.json");
    let run = Command::new("python3")
        .arg(generator())
        .arg("--emit-spec")
        .arg(&spec)
        .output()
        .expect("python3");
    assert!(
        run.status.success(),
        "spec emission failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    #[derive(Deserialize)]
    struct SpecCase {
        id: String,
        klass: String,
        deforms: String,
        attacks: String,
        expect: String,
        scope: String,
        note: String,
    }
    #[derive(Deserialize)]
    struct SpecFile {
        cases: Vec<SpecCase>,
    }
    let raw = std::fs::read_to_string(&spec).unwrap();
    let file: SpecFile = serde_json::from_str(&raw).unwrap();
    assert!(
        file.cases.len() >= 15,
        "the hostile spec carries only {} cases — too few to cover conflicts, \
         escapes, hidden secrets, malformed input, resource pressure and \
         classification in both directions",
        file.cases.len()
    );

    let mut classes: Vec<String> = Vec::new();
    for case in &file.cases {
        let id = &case.id;
        assert!(
            LEGITIMATE_OUTCOMES.contains(&case.expect.as_str()),
            "case {id} declares outcome {:?}, which is not one of \
             {LEGITIMATE_OUTCOMES:?} — a case with no declared outcome asserts \
             nothing but still reads as coverage",
            case.expect
        );
        assert!(
            case.scope == "portable" || case.scope == "platform",
            "case {id} declares scope {:?}",
            case.scope
        );
        for (key, v) in [
            ("deforms", &case.deforms),
            ("attacks", &case.attacks),
            ("note", &case.note),
        ] {
            assert!(
                v.len() >= 20,
                "case {id} has a {key} of {v:?}: every hostile case must be a \
                 plausible DEFORMATION of a real peer format and must record \
                 which field or structure it attacks"
            );
        }
        classes.push(case.klass.clone());
    }
    classes.sort();
    classes.dedup();
    for required in [
        "bounds",
        "classification",
        "conflict",
        "escape",
        "hidden-secret",
        "malformed",
        "windows-hazard",
    ] {
        assert!(
            classes.iter().any(|c| c == required),
            "no hostile case covers the {required:?} class; present: {classes:?}"
        );
    }
}

/// The generator must be able to go RED. Handed a case id that does not exist
/// it exits non-zero; handed no arguments it exits non-zero.
#[test]
fn hostile_generator_can_go_red() {
    let bad = Command::new("python3")
        .arg(generator())
        .arg("--only")
        .arg("no-such-case")
        .arg("--out")
        .arg(tempfile::tempdir().unwrap().path())
        .output()
        .expect("python3");
    assert!(
        !bad.status.success(),
        "the generator reported SUCCESS for a case id it does not have — a \
         generator that cannot go red produces corpora that prove nothing"
    );
    let noargs = Command::new("python3")
        .arg(generator())
        .output()
        .expect("python3");
    assert!(
        !noargs.status.success(),
        "the generator reported SUCCESS with no arguments"
    );
}

/// On Linux, a case-only and a normal-form-only name distinction MUST survive
/// materialisation. If this host ever collapses them, every collision case
/// below is testing one file while claiming to test two — so this is the guard
/// that stops the authoritative proof host from silently going soft.
#[test]
fn hostile_name_distinctions_survive_on_this_platform_or_the_generator_says_so() {
    for id in ["conflict-casefold", "conflict-normalform"] {
        let (_out, manifest) = materialise(id);
        let case = case_of(&manifest);
        assert!(
            !case.require_distinct_on.is_empty(),
            "case {id} declares no platform on which its distinction must survive"
        );
        if case.require_distinct_on.contains(&manifest.platform) {
            assert!(
                !case.collapsed,
                "case {id} declares its distinction MUST survive on {} and it \
                 collapsed: {:?}",
                manifest.platform, case.collapsed_pairs
            );
            assert_eq!(
                case.entries, 3,
                "case {id} on {}: expected the baseline profile plus TWO distinct \
                 colliding names",
                manifest.platform
            );
        }
    }
}

// ===========================================================================
// CONFLICT SEMANTICS
// ===========================================================================

#[test]
fn hostile_exact_name_collision_is_reported_and_never_silently_overwrites() {
    let (_out, manifest) = materialise("conflict-exact");
    let case = case_of(&manifest);
    assert_eq!(case.expect, "conflict");

    let t = Target::new();
    t.seed_profile("collide", "PRE-EXISTING-MARKER");
    let before = t.config_toml();
    let sentinel_before = t.sentinel_digest();

    let json = t.migrate_json(&case.corpus);
    json.assert_no_panic(&case.id);
    assert!(json.ok(), "migrate --json failed: {}", json.stderr);
    let doc: serde_json::Value = serde_json::from_str(&json.stdout).expect("plan JSON");
    let collided = doc["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == "collide")
        .expect("the colliding profile must appear in the plan, named");
    assert_eq!(
        collided["conflict"], true,
        "the plan must REPORT the collision. Reporting it is the whole difference \
         between an operator keeping their profile and losing it silently."
    );

    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    let after = t.config_toml();
    assert!(
        after.contains("PRE-EXISTING-MARKER"),
        "the pre-existing profile was overwritten by a colliding peer profile \
         without --overwrite.\nbefore:\n{before}\nafter:\n{after}"
    );
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("no Accounting line:\n{}", apply.stdout));
    assert!(
        acct.balances(),
        "conservation broke under a conflict: {acct:?}"
    );
    assert_eq!(
        sentinel_before,
        t.sentinel_digest(),
        "the sentinel tree outside the target home changed"
    );
}

#[test]
fn hostile_casefold_collision_keeps_both_peer_items_accounted() {
    let (_out, manifest) = materialise("conflict-casefold");
    let case = case_of(&manifest);
    assert_eq!(case.expect, "conflict");

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("no Accounting line:\n{}\n{}", apply.stdout, apply.stderr));
    assert!(acct.balances(), "conservation broke: {acct:?}");

    if case.collapsed {
        // The filesystem merged them. That is the platform behaviour under test
        // and the declared outcome there is that ONE item is seen, not that two
        // were silently merged after being seen as two.
        assert_eq!(
            acct.discovered, 2,
            "on {} the two case-only names collapsed to one, so exactly the \
             baseline plus one collided profile must be discovered",
            manifest.platform
        );
    } else {
        assert_eq!(
            acct.discovered, 3,
            "on {} the two case-only names are distinct, so BOTH must be \
             discovered and accounted — merging them here is the silent \
             overwrite this case exists to catch",
            manifest.platform
        );
        let cfg = t.config_toml();
        let lower = cfg.matches("[profiles.collide]").count();
        let upper = cfg.matches("[profiles.Collide]").count();
        assert_eq!(
            lower + upper,
            2,
            "both case-distinct peer profiles must survive into config.toml as \
             two profiles; config.toml was:\n{cfg}"
        );
    }
    assert_eq!(sentinel_before, t.sentinel_digest(), "sentinel changed");
}

#[test]
fn hostile_normalform_collision_keeps_both_peer_items_accounted() {
    let (_out, manifest) = materialise("conflict-normalform");
    let case = case_of(&manifest);
    assert_eq!(case.expect, "conflict");

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("no Accounting line:\n{}\n{}", apply.stdout, apply.stderr));
    assert!(acct.balances(), "conservation broke: {acct:?}");
    let expected = if case.collapsed { 2 } else { 3 };
    assert_eq!(
        acct.discovered, expected,
        "on {} collapsed={} — the number of discovered items must match what the \
         filesystem actually materialised, or the report is describing a corpus \
         that does not exist",
        manifest.platform, case.collapsed
    );
    assert_eq!(sentinel_before, t.sentinel_digest(), "sentinel changed");
}

// ===========================================================================
// ISOLATION — proven by what did NOT change outside the target
// ===========================================================================

fn escape_case(id: &str) {
    let (_out, manifest) = materialise(id);
    let case = case_of(&manifest);
    assert_eq!(case.expect, "refused");
    assert!(
        case.symlinks.iter().all(|s| !s.starts_with("<unlinkable")),
        "case {id}: the escape link could not be created on {}, so the case did \
         not test anything: {:?}",
        manifest.platform,
        case.symlinks
    );

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();

    let json = t.migrate_json(&case.corpus);
    json.assert_no_panic(&case.id);
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);

    // The refusal must be NAMED. A silent skip is indistinguishable from a
    // successful traversal that happened to find nothing.
    let text = format!("{}{}", json.combined(), apply.combined());
    let names_it = text.contains("symlink") || text.contains("escape") || text.contains("refus");
    assert!(
        names_it,
        "case {id}: nothing in the product's output names the refusal. An escape \
         that is silently skipped is indistinguishable from one that was \
         followed and found nothing.\n--- json ---\n{}\n--- apply ---\n{}",
        json.combined(),
        apply.combined()
    );

    // And the escape reached NOTHING: no entry containing it was admitted to
    // the store. A refusal reported to the operator over a store that took the
    // content anyway is not a refusal.
    let listed = t.run(&["migrate", "quarantined"]);
    listed.assert_no_panic(&case.id);
    assert_eq!(
        listed.stdout.matches("reason:").count(),
        0,
        "case {id}: an item carrying a root escape was ADMITTED to the \
         quarantine store:\n{}",
        listed.stdout
    );

    // Never followed: the escape targets are real system paths, so their
    // contents must not appear anywhere in what the product emitted.
    for leaked in ["root:x:", "/etc/shadow", "PATH=/usr"] {
        assert!(
            !text.contains(leaked),
            "case {id}: content from outside the source root reached the output \
             ({leaked}) — the escape was FOLLOWED"
        );
    }

    // And the only honest isolation proof: nothing outside the target changed.
    assert_eq!(
        sentinel_before,
        t.sentinel_digest(),
        "case {id}: the sentinel tree OUTSIDE the target home changed. Asserting \
         that the code contains a path check proves the check exists; this proves \
         nothing escaped."
    );
    if let Some(acct) = Accounting::parse(&apply.stdout) {
        assert!(acct.balances(), "case {id}: conservation broke: {acct:?}");
    }
}

#[test]
fn hostile_absolute_symlink_escape_is_refused_and_nothing_outside_changed() {
    escape_case("escape-symlink-absolute");
}

#[test]
fn hostile_traversal_symlink_escape_is_refused_and_nothing_outside_changed() {
    escape_case("escape-symlink-traversal");
}

#[test]
fn hostile_directory_symlink_escape_is_refused_and_nothing_outside_changed() {
    escape_case("escape-symlink-dir");
}

// ===========================================================================
// SECRET-SOURCE REMAPPING UNDER HOSTILITY
// ===========================================================================

fn hidden_secret_case(id: &str, canary_index: usize) {
    let (_out, manifest) = materialise(id);
    let case = case_of(&manifest);
    let canary = manifest.canaries[canary_index].clone();
    // The positive half FIRST: a corpus that never carried the canary would make
    // every absence assertion below pass vacuously.
    assert_canary_present_in_corpus(&manifest, &case.corpus, &canary);

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();
    let json = t.migrate_json(&case.corpus);
    json.assert_no_panic(&case.id);
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);

    assert_no_canary(&manifest, &json.combined(), "the emitted --json plan");
    assert_no_canary(&manifest, &apply.combined(), "the apply report");

    match case.expect.as_str() {
        "quarantined" => {
            let acct = Accounting::parse(&apply.stdout)
                .unwrap_or_else(|| panic!("no Accounting line:\n{}", apply.stdout));
            assert!(
                acct.quarantined >= 1,
                "case {id}: an executable body carrying a secret must be \
                 CONTAINED, not imported: {acct:?}"
            );
            let listed = t.run(&["migrate", "quarantined"]);
            assert!(
                listed.stdout.contains("skill:"),
                "case {id}: the contained item is not listed by `migrate \
                 quarantined`:\n{}",
                listed.stdout
            );
            assert_no_canary(&manifest, &listed.combined(), "`migrate quarantined`");
        }
        "imported" => {
            let acct = Accounting::parse(&apply.stdout)
                .unwrap_or_else(|| panic!("no Accounting line:\n{}", apply.stdout));
            assert!(
                acct.balances() && acct.discovered >= 1,
                "case {id}: the corpus must be discovered and balance: {acct:?}"
            );
        }
        other => panic!("case {id} declares an outcome this test cannot assert: {other}"),
    }
    assert_eq!(sentinel_before, t.sentinel_digest(), "sentinel changed");
}

#[test]
fn hostile_secret_in_a_memory_note_never_reaches_a_plan_or_report() {
    hidden_secret_case("secret-in-memory-note", 0);
}

#[test]
fn hostile_secret_in_a_persona_body_never_reaches_a_plan_or_report() {
    hidden_secret_case("secret-in-persona", 1);
}

#[test]
fn hostile_secret_in_a_skill_body_is_contained_and_never_reported() {
    hidden_secret_case("secret-in-skill-body", 2);
}

/// The channel 26-01 already redacts, re-checked under attack — and with its
/// positive half, so "no value" is not just "no credential was found".
#[test]
fn hostile_env_credential_reports_its_name_and_never_its_value() {
    let (_out, manifest) = materialise("secret-in-env");
    let case = case_of(&manifest);
    let canary = manifest.canaries[3].clone();
    assert_canary_present_in_corpus(&manifest, &case.corpus, &canary);

    let t = Target::new();
    let json = t.migrate_json(&case.corpus);
    json.assert_no_panic(&case.id);
    assert!(json.ok(), "migrate --json failed: {}", json.stderr);
    assert_no_canary(&manifest, &json.combined(), "the emitted --json plan");
    assert!(
        json.stdout.contains("ANTHROPIC_API_KEY"),
        "the credential's NAME must be reported — otherwise the absence of its \
         value proves only that nothing was discovered:\n{}",
        json.stdout
    );
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    assert_no_canary(&manifest, &apply.combined(), "the apply report");
    assert_no_canary(&manifest, &t.config_toml(), "the written config.toml");
}

// ===========================================================================
// CLASSIFICATION, IN BOTH DIRECTIONS
// ===========================================================================

#[test]
fn hostile_executable_claiming_to_be_data_is_still_contained() {
    let (_out, manifest) = materialise("exec-disguised-as-data");
    let case = case_of(&manifest);
    assert_eq!(case.expect, "quarantined");

    let t = Target::new();
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("no Accounting line:\n{}\n{}", apply.stdout, apply.stderr));
    assert!(
        acct.quarantined >= 1,
        "five self-declared trust claims in the payload talked the classifier out \
         of a containment decision: {acct:?}\n{}",
        apply.stdout
    );
    assert!(acct.balances(), "conservation broke: {acct:?}");
}

#[test]
fn hostile_data_that_merely_looks_executable_is_not_contained() {
    let (_out, manifest) = materialise("data-that-looks-executable");
    let case = case_of(&manifest);
    assert_eq!(case.expect, "imported");

    let t = Target::new();
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("no Accounting line:\n{}\n{}", apply.stdout, apply.stderr));
    assert_eq!(
        acct.quarantined, 0,
        "a persona body containing shell-directive SYNTAX was contained. Treating \
         every data surface as dangerous trains an operator to promote without \
         reading, which is the failure that makes quarantine worthless: {acct:?}"
    );
    assert!(acct.balances(), "conservation broke: {acct:?}");
}

// ===========================================================================
// MALFORMED INPUT — a named error, never a panic, never a silent empty success
// ===========================================================================

fn malformed_case(id: &str) {
    let (_out, manifest) = materialise(id);
    let case = case_of(&manifest);
    assert_eq!(case.expect, "refused");

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();
    let json = t.migrate_json(&case.corpus);
    json.assert_no_panic(&case.id);
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);

    if json.ok() {
        // Surviving is allowed ONLY if the product said something about the
        // malformed input. A clean exit with an empty plan reads as success.
        let doc: serde_json::Value = serde_json::from_str(&json.stdout).unwrap_or_else(|e| {
            panic!(
                "case {id}: --json exited 0 but emitted no parseable plan ({e}):\n{}",
                json.stdout
            )
        });
        let warned = doc["warnings"]
            .as_array()
            .map(|w| !w.is_empty())
            .unwrap_or(false);
        let discovered = doc["items"].as_array().map(Vec::len).unwrap_or(0);
        assert!(
            warned,
            "case {id}: the product accepted malformed input and emitted a plan \
             with NO warning. A silently empty or silently partial result reads \
             as success, which is the precise failure this case exists to catch. \
             items={discovered}\n{}",
            json.stdout
        );
        assert!(
            discovered >= 1,
            "case {id}: exited 0 and discovered nothing at all — the baseline \
             profile {:?} should still be present",
            manifest.baseline_profile
        );
    } else {
        assert!(
            !json.stderr.trim().is_empty(),
            "case {id}: the product refused with an EMPTY message. A refusal an \
             operator cannot read is a refusal they cannot act on."
        );
        assert!(
            json.stderr.contains("profiles/") || json.stderr.contains("config.yaml"),
            "case {id}: the refusal does not NAME the offending input:\n{}",
            json.stderr
        );
    }
    assert_eq!(sentinel_before, t.sentinel_digest(), "sentinel changed");
    let _ = apply;
}

#[test]
fn hostile_truncated_configuration_is_named_not_panicked() {
    malformed_case("malformed-truncated");
}

#[test]
fn hostile_wrong_typed_configuration_is_named_not_panicked() {
    malformed_case("malformed-wrongtype");
}

#[test]
fn hostile_deeply_nested_configuration_is_named_not_panicked() {
    malformed_case("malformed-deepnest");
}

// ===========================================================================
// RESOURCE PRESSURE
// ===========================================================================

/// A refusal is a NAMED refusal that DENIES ADMISSION, and both halves are
/// asserted.
///
/// The product's contract, established in 26-02, is that a surface past a
/// ceiling is refused at `QuarantineStore::admit`, the refusal is reported to
/// the operator verbatim (`… — refused: <reason>`), and the item still
/// BALANCES in the accounting — a refusal is never a silent drop. So asserting
/// `quarantined == 0` would flag correct behaviour: the refused item is
/// accounted in the quarantined column while being absent from the store.
/// What must be asserted instead is that the refusal was named AND that the
/// offending surface did not reach the store.
///
/// `admitted_ceiling` is how many entries the store is allowed to hold
/// afterwards — 0 when the whole offending item was refused, and the
/// per-admission file ceiling when the case pushes an item COUNT past it.
fn bounds_case(id: &str, admitted_ceiling: usize) {
    let (_out, manifest) = materialise(id);
    let case = case_of(&manifest);
    assert_eq!(case.expect, "refused");

    let t = Target::new();
    let sentinel_before = t.sentinel_digest();
    let apply = t.migrate_apply(&case.corpus);
    apply.assert_no_panic(&case.id);

    let text = apply.combined();
    assert!(
        text.contains("refused:"),
        "case {id}: nothing in the output names a refusal, so a surface past a \
         ceiling was absorbed silently.\nexit={:?}\n{text}",
        apply.code
    );
    assert!(
        text.contains("exceeds"),
        "case {id}: the refusal does not name the ceiling it hit, so an operator \
         cannot tell what to do about it.\n{text}"
    );
    let acct = Accounting::parse(&apply.stdout)
        .unwrap_or_else(|| panic!("case {id}: no Accounting line:\n{}", apply.stdout));
    assert!(
        acct.balances(),
        "case {id}: a refusal must still balance — a refused item that vanishes \
         from the accounting is a silent drop: {acct:?}"
    );

    // The half that proves the ceiling actually held: the store.
    let listed = t.run(&["migrate", "quarantined"]);
    listed.assert_no_panic(&case.id);
    let admitted = listed.stdout.matches("reason:").count();
    assert_eq!(
        admitted, admitted_ceiling,
        "case {id}: the store holds {admitted} entries but the ceiling permits \
         {admitted_ceiling}. A refusal the operator is told about, over a store \
         that took the content anyway, is not a refusal.\n{}",
        listed.stdout
    );
    assert_eq!(sentinel_before, t.sentinel_digest(), "sentinel changed");
}

/// A single member past the 4 MiB per-file ceiling: the whole item is refused,
/// so the store must be EMPTY afterwards.
#[test]
fn hostile_oversized_member_hits_the_declared_refusal() {
    bounds_case("bounds-oversized-member", 0);
}

/// 600 executable items against a 512-file ceiling: the first 512 are admitted
/// and every one past it is refused BY NAME. The ceiling is read from the
/// product's own constant rather than retyped, so a change to it cannot make
/// this test quietly assert the wrong number.
#[test]
fn hostile_excessive_item_count_hits_the_declared_refusal() {
    bounds_case(
        "bounds-item-count",
        wcore_cli::migrate::quarantine::MAX_QUARANTINE_FILES,
    );
}

// ===========================================================================
// CONSERVATION UNDER ATTACK, and the aggregate isolation proof
// ===========================================================================

/// 26-02's conservation invariant balanced on cooperative input. The question
/// this asks is whether it still balances when the input is trying to make an
/// item disappear.
#[test]
fn hostile_conservation_invariant_balances_across_every_corpus() {
    let out = tempfile::tempdir().unwrap();
    let run = Command::new("python3")
        .arg(generator())
        .arg("--out")
        .arg(out.path())
        .output()
        .expect("python3");
    assert!(
        run.status.success(),
        "generating the full hostile corpus set failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let manifest: Manifest =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("cases.json")).unwrap())
            .unwrap();

    let mut checked = 0usize;
    let mut unbalanced: Vec<String> = Vec::new();
    for case in &manifest.cases {
        let t = Target::new();
        let sentinel_before = t.sentinel_digest();
        let apply = t.migrate_apply(&case.corpus);
        apply.assert_no_panic(&case.id);
        assert_eq!(
            sentinel_before,
            t.sentinel_digest(),
            "case {}: the sentinel tree outside the target home changed",
            case.id
        );
        let acct = Accounting::parse(&apply.stdout);
        if let Some(acct) = &acct {
            checked += 1;
            if !acct.balances() {
                unbalanced.push(format!("{}: {acct:?}", case.id));
            }
        }
        // The per-case record the SUMMARY reports from: the real peer format
        // each case deforms, the field it attacks, the declared outcome, and
        // what the product actually did with it.
        println!(
            "HOSTILE-OBSERVED: id={} class={} scope={} expect={} platform={} \
             collapsed={} entries={} digest={} exit={:?} accounting={:?} \
             deforms={:?} attacks={:?} note={:?}",
            case.id,
            case.class,
            case.scope,
            case.expect,
            case.platform,
            case.collapsed,
            case.entries,
            &case.corpus_digest[..16],
            apply.code,
            acct,
            case.deforms,
            case.attacks,
            case.note,
        );
    }
    assert!(
        unbalanced.is_empty(),
        "the conservation invariant broke under attack: {unbalanced:?}"
    );
    assert!(
        checked >= 8,
        "only {checked} corpora produced an Accounting line, so this measured far \
         less than it claims — a refusal-heavy set that never reaches the report \
         is not evidence about conservation"
    );
    assert!(
        !manifest.platform.is_empty() && !manifest.canaries.is_empty(),
        "the manifest records no platform or no canaries"
    );
}

// ===========================================================================
// RECOVERY UNDER HOSTILITY
// ===========================================================================

/// An operation that refuses part-way through hostile input must leave the
/// target byte-identical. Measured by digest, never read off the message —
/// which is how a warn-and-continue would be caught.
#[test]
fn hostile_refused_restore_leaves_an_occupied_target_byte_identical() {
    let (_out, manifest) = materialise("secret-in-env");
    let case = case_of(&manifest);

    // Build a real home by importing a hostile corpus, then archive it.
    let src = Target::new();
    let imported = src.migrate_apply(&case.corpus);
    imported.assert_no_panic(&case.id);

    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("hostile.wlbak");
    let create = src.run(&[
        "backup",
        "create",
        "--home",
        src.home().to_str().unwrap(),
        "--out",
        archive.to_str().unwrap(),
    ]);
    create.assert_no_panic("recovery");
    assert!(
        create.ok() && archive.is_file(),
        "backup create failed over a hostile-imported home:\nstdout:{}\nstderr:{}",
        create.stdout,
        create.stderr
    );

    // An OCCUPIED target holding live state the restore must not touch.
    let dst = Target::new();
    dst.seed_profile("live", "LIVE-PROFILE-MARKER");
    std::fs::write(
        dst.home().join("keep.txt"),
        "a file the archive does not carry\n",
    )
    .unwrap();
    let pre = tree_digest(dst.home());

    let restore = dst.run(&[
        "backup",
        "restore",
        archive.to_str().unwrap(),
        "--home",
        dst.home().to_str().unwrap(),
    ]);
    restore.assert_no_panic("recovery");
    assert!(
        !restore.ok(),
        "restore into an OCCUPIED target succeeded without --replace, so live \
         state was replaced in place:\n{}",
        restore.stdout
    );
    assert_eq!(
        pre,
        tree_digest(dst.home()),
        "the refused restore MODIFIED the occupied target. Measured by digest, \
         not read off the refusal message."
    );
    assert!(
        dst.config_toml().contains("LIVE-PROFILE-MARKER"),
        "the live profile did not survive the refusal"
    );
    assert!(
        dst.home().join("keep.txt").is_file(),
        "an untouched file vanished"
    );
}

/// The archive verification 26-03 built, shown a manifest that declares one
/// thing while the payload contains another.
#[test]
fn hostile_manifest_payload_mismatch_is_refused_by_verification() {
    let (_out, manifest) = materialise("secret-in-env");
    let case = case_of(&manifest);

    let src = Target::new();
    src.migrate_apply(&case.corpus).assert_no_panic("mismatch");
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("ok.wlbak");
    let create = src.run(&[
        "backup",
        "create",
        "--home",
        src.home().to_str().unwrap(),
        "--out",
        archive.to_str().unwrap(),
    ]);
    assert!(create.ok(), "backup create failed: {}", create.stderr);

    let good = src.run(&["backup", "verify", archive.to_str().unwrap()]);
    assert!(
        good.ok(),
        "the untampered archive does not verify, so the tampered result below \
         would prove nothing:\n{}\n{}",
        good.stdout,
        good.stderr
    );

    // Tamper the PAYLOAD so the manifest's declared digests no longer describe
    // it. Byte-level, in the middle, so length-only checks cannot catch it.
    let mut bytes = std::fs::read(&archive).unwrap();
    let mid = bytes.len() / 2;
    for b in bytes.iter_mut().skip(mid).take(64) {
        *b ^= 0xff;
    }
    let tampered = work.path().join("tampered.wlbak");
    std::fs::write(&tampered, &bytes).unwrap();

    let bad = src.run(&["backup", "verify", tampered.to_str().unwrap()]);
    bad.assert_no_panic("mismatch");
    assert!(
        !bad.ok(),
        "a payload that no longer matches the manifest it travels with VERIFIED. \
         An archive whose manifest declares one thing while its payload contains \
         another is the case verification exists for.\n{}\n{}",
        bad.stdout,
        bad.stderr
    );
    assert!(
        !bad.stderr.trim().is_empty() || !bad.stdout.trim().is_empty(),
        "the rejection carried no message"
    );
}
