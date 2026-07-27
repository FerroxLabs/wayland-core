//! F23-06 — the persistent index's invalidation, scope and isolation
//! properties, one test per behaviour clause of Phase 23B plan 03 Task 1.
//!
//! Every test runs against a **real temporary git repository**, not a
//! synthetic in-memory fixture, because the properties under test are
//! properties of a filesystem and a checkout.
//!
//! Two disciplines are load-bearing here and are called out where they apply:
//!
//! - **Incrementality is asserted by a READ COUNT, never by a stopwatch.**
//!   A timing assertion is flaky under load and, worse, a sufficiently fast
//!   full rebuild satisfies it. [`IndexStats::files_read`] cannot be
//!   satisfied by a fast rebuild.
//! - **Secret isolation is asserted against the STORE FILE'S OWN BYTES.**
//!   Asserting that a query does not return an excluded file proves only that
//!   it was filtered; the requirement is that it was never read, and the only
//!   witness to that is the artifact on disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use wcore_repomap::{IndexOptions, IndexStore, RepoMap, RepoMapError, SearchQuery};

// ── Test repository helpers ──────────────────────────────────────────────

/// Run a git command in `dir`, failing the test loudly if git is absent or
/// the command failed.
///
/// Deliberately NOT tolerant of a missing git: a test that silently skips
/// when its tool is unavailable is a gate that cannot go red. Every host this
/// suite runs on already drives git for the checkout the gate pins.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.email=index@example.invalid",
            "-c",
            "user.name=index test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} could not be spawned in {dir:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {dir:?}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct TestRepo {
    _dir: tempfile::TempDir,
    _store_dir: tempfile::TempDir,
    root: PathBuf,
    store_path: PathBuf,
}

impl TestRepo {
    /// A git repository with one commit containing two Rust files and a
    /// `.gitignore` excluding `secrets/` and `target/`.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        // The store lives OUTSIDE the indexed root, so the index can never
        // index itself and no test result depends on that self-reference.
        let store_dir = tempfile::tempdir().expect("store tempdir");
        let store_path = store_dir.path().join("index.db");

        git(&root, &["init", "-q", "-b", "main"]);
        write(&root, ".gitignore", "secrets/\ntarget/\n");
        write(
            &root,
            "src/alpha.rs",
            "pub fn alpha_function() {}\npub struct AlphaStruct;\n",
        );
        write(
            &root,
            "src/beta.rs",
            "pub fn beta_function() {}\npub trait BetaTrait {}\n",
        );
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "initial"]);

        Self {
            _dir: dir,
            _store_dir: store_dir,
            root,
            store_path,
        }
    }

    fn open(&self) -> IndexStore {
        IndexStore::open(&self.store_path, &self.root).expect("open store")
    }

    fn refresh(&self) -> wcore_repomap::IndexStats {
        let mut store = self.open();
        store.refresh(&IndexOptions::default()).expect("refresh")
    }

    /// Every byte the store occupies on disk, database and WAL sidecar alike.
    ///
    /// The sidecar is included deliberately: with WAL journalling the most
    /// recently written content lives there until a checkpoint, so a
    /// secret-isolation proof that read only the main database file could
    /// pass while the secret sat in the log beside it.
    fn store_bytes(&self) -> Vec<u8> {
        let mut all = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.store_path.clone().into_os_string();
            path.push(suffix);
            if let Ok(bytes) = fs::read(PathBuf::from(path)) {
                all.extend_from_slice(&bytes);
            }
        }
        assert!(
            !all.is_empty(),
            "store at {:?} produced no bytes to search — the isolation \
             assertion below would be vacuous",
            self.store_path
        );
        all
    }
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
}

/// A value that exists only for the duration of this process run.
///
/// A hard-coded secret could have leaked into the store from any earlier run
/// on this machine, so an assertion about it would prove nothing about THIS
/// run. Built from a caller-supplied tag, the process id and the current
/// nanosecond clock.
///
/// The tag matters: the secret nonce and the control nonce must not be
/// substrings of one another, or the control marker's presence in the store
/// would also satisfy a substring search for the secret and the isolation
/// assertion would report a leak that did not happen.
fn runtime_nonce(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{tag}{}x{nanos}x", std::process::id())
}

// ── 1. A first build writes one record per in-scope file, plus symbols ───

#[test]
fn first_build_writes_one_record_per_in_scope_file_with_symbols() {
    let repo = TestRepo::new();
    let stats = repo.refresh();

    assert_eq!(stats.added, 3, "expected .gitignore + 2 sources: {stats:?}");
    assert_eq!(stats.changed, 0);
    assert_eq!(stats.deleted, 0);

    let store = repo.open();
    assert_eq!(store.record_count().expect("count"), 3);
    assert!(
        store.symbol_count().expect("symbols") >= 4,
        "expected the four declared symbols to be recorded"
    );

    let alpha = store
        .lookup(Path::new("src/alpha.rs"))
        .expect("lookup")
        .expect("src/alpha.rs must be recorded");
    assert_eq!(alpha.content_hash.len(), 64, "content hash must be SHA-256");
    assert!(alpha.indexable);

    let symbols = store.symbols_for("src/alpha.rs").expect("symbols_for");
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"alpha_function"), "{names:?}");
    assert!(names.contains(&"AlphaStruct"), "{names:?}");
}

// ── 2. Re-open with no change reads NOTHING ─────────────────────────────

#[test]
fn reopening_an_unchanged_store_reads_no_files() {
    let repo = TestRepo::new();
    let first = repo.refresh();
    assert!(first.files_read > 0, "the first pass must read: {first:?}");

    let second = repo.refresh();

    // THE assertion of this plan's incrementality claim. Proved by counting
    // reads, not by timing: a rebuild that happened to be fast would satisfy
    // a stopwatch and cannot satisfy this.
    assert_eq!(
        second.files_read, 0,
        "an unchanged re-open must open no file: {second:?}"
    );
    assert_eq!(second.files_extracted, 0, "{second:?}");
    assert_eq!(second.added, 0);
    assert_eq!(second.changed, 0);
    assert_eq!(second.deleted, 0);
    assert_eq!(second.renamed, 0);
    assert_eq!(second.unchanged, first.scanned, "{second:?}");
}

// ── 3. add / change / delete each touch exactly one record ───────────────

#[test]
fn adding_a_file_adds_exactly_one_record() {
    let repo = TestRepo::new();
    repo.refresh();

    write(&repo.root, "src/gamma.rs", "pub fn gamma_function() {}\n");
    let stats = repo.refresh();

    assert_eq!(stats.added, 1, "{stats:?}");
    assert_eq!(stats.changed, 0, "{stats:?}");
    assert_eq!(stats.deleted, 0, "{stats:?}");
    assert_eq!(
        stats.files_read, 1,
        "only the new file may be opened: {stats:?}"
    );
    assert_eq!(stats.files_extracted, 1, "{stats:?}");
    assert_eq!(repo.open().record_count().expect("count"), 4);
}

#[test]
fn changing_a_files_bytes_changes_only_that_records_hash() {
    let repo = TestRepo::new();
    repo.refresh();
    let before = repo
        .open()
        .lookup(Path::new("src/alpha.rs"))
        .expect("lookup")
        .expect("record");
    let beta_before = repo
        .open()
        .lookup(Path::new("src/beta.rs"))
        .expect("lookup")
        .expect("record");

    write(
        &repo.root,
        "src/alpha.rs",
        "pub fn alpha_function() {}\npub struct AlphaStruct;\npub enum AlphaEnum { A }\n",
    );
    let stats = repo.refresh();

    assert_eq!(stats.changed, 1, "{stats:?}");
    assert_eq!(stats.added, 0, "{stats:?}");
    assert_eq!(
        stats.files_extracted, 1,
        "only the changed file may be re-extracted: {stats:?}"
    );

    let store = repo.open();
    let after = store
        .lookup(Path::new("src/alpha.rs"))
        .expect("lookup")
        .expect("record");
    assert_ne!(before.content_hash, after.content_hash);
    let beta_after = store
        .lookup(Path::new("src/beta.rs"))
        .expect("lookup")
        .expect("record");
    assert_eq!(
        beta_before.content_hash, beta_after.content_hash,
        "an untouched file's record must not move"
    );

    let names: Vec<String> = store
        .symbols_for("src/alpha.rs")
        .expect("symbols")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(names.contains(&"AlphaEnum".to_string()), "{names:?}");
}

#[test]
fn deleting_a_file_removes_its_record_and_its_symbols() {
    let repo = TestRepo::new();
    repo.refresh();
    assert!(
        !repo
            .open()
            .symbols_for("src/beta.rs")
            .expect("sym")
            .is_empty(),
        "precondition: beta had symbols"
    );

    fs::remove_file(repo.root.join("src/beta.rs")).expect("remove");
    let stats = repo.refresh();

    assert_eq!(stats.deleted, 1, "{stats:?}");
    assert_eq!(stats.files_read, 0, "a deletion needs no read: {stats:?}");

    let store = repo.open();
    assert!(
        store
            .lookup(Path::new("src/beta.rs"))
            .expect("lookup")
            .is_none(),
        "the deleted path must not resolve"
    );
    assert!(
        store
            .symbols_for("src/beta.rs")
            .expect("symbols")
            .is_empty(),
        "the deleted file's symbols must be gone"
    );
    assert_eq!(store.record_count().expect("count"), 2);
}

// ── 4. Rename reuses the content hash and does not re-extract ────────────

#[test]
fn renaming_an_unchanged_file_reuses_its_hash_and_re_extracts_nothing() {
    let repo = TestRepo::new();
    repo.refresh();
    let before = repo
        .open()
        .lookup(Path::new("src/beta.rs"))
        .expect("lookup")
        .expect("record");
    let symbols_before = repo.open().symbols_for("src/beta.rs").expect("symbols");

    fs::rename(
        repo.root.join("src/beta.rs"),
        repo.root.join("src/renamed.rs"),
    )
    .expect("rename");
    let stats = repo.refresh();

    assert_eq!(stats.renamed, 1, "{stats:?}");
    assert_eq!(stats.added, 0, "a rename is not an add: {stats:?}");
    assert_eq!(stats.deleted, 0, "a rename is not a delete: {stats:?}");
    assert_eq!(
        stats.files_extracted, 0,
        "a rename must NOT re-extract — the content hash is reused: {stats:?}"
    );

    let store = repo.open();
    let after = store
        .lookup(Path::new("src/renamed.rs"))
        .expect("lookup")
        .expect("the new path must resolve");
    assert_eq!(
        before.content_hash, after.content_hash,
        "the content hash must be REUSED, not recomputed into a new record"
    );
    assert!(
        store
            .lookup(Path::new("src/beta.rs"))
            .expect("lookup")
            .is_none(),
        "the old path must no longer resolve"
    );
    assert_eq!(
        store.symbols_for("src/renamed.rs").expect("symbols"),
        symbols_before,
        "the symbols must have moved with the record, unchanged"
    );
    assert_eq!(store.record_count().expect("count"), 3);
}

// ── 5. Switching HEAD changes scope identity and invalidates only the
//       records whose content differs ────────────────────────────────────

#[test]
fn switching_branches_invalidates_only_the_records_whose_content_differs() {
    let repo = TestRepo::new();
    repo.refresh();
    let scope_before = repo.open().recorded_scope().expect("scope");
    assert!(
        !repo.open().scope_drifted().expect("drift"),
        "a freshly refreshed store must not report drift"
    );
    let alpha_before = repo
        .open()
        .lookup(Path::new("src/alpha.rs"))
        .expect("lookup")
        .expect("record");

    // A real second branch differing in exactly one of the three files.
    git(&repo.root, &["checkout", "-q", "-b", "other"]);
    write(
        &repo.root,
        "src/beta.rs",
        "pub fn beta_function() {}\npub trait BetaTrait {}\npub fn only_on_other() {}\n",
    );
    git(&repo.root, &["add", "-A"]);
    git(&repo.root, &["commit", "-qm", "diverge"]);

    let store = repo.open();
    assert!(
        store.scope_drifted().expect("drift"),
        "a store built on `main` must report drift once HEAD moved to `other`"
    );
    drop(store);

    let stats = repo.refresh();

    assert!(stats.scope_changed, "{stats:?}");
    assert_eq!(
        stats.changed, 1,
        "exactly the differing file may be invalidated: {stats:?}"
    );
    assert_eq!(stats.deleted, 0, "a branch switch is not a wipe: {stats:?}");
    assert_eq!(stats.added, 0, "{stats:?}");
    assert_eq!(
        stats.unchanged, 2,
        "the two identical files must survive untouched: {stats:?}"
    );

    let store = repo.open();
    let alpha_after = store
        .lookup(Path::new("src/alpha.rs"))
        .expect("lookup")
        .expect("record");
    assert_eq!(alpha_before.content_hash, alpha_after.content_hash);
    let scope_after = store.recorded_scope().expect("scope");
    assert_ne!(
        scope_before, scope_after,
        "the recorded scope identity must have moved with HEAD"
    );
    assert!(!store.scope_drifted().expect("drift"));
}

// ── 6. A gitignored file is never READ into the store ────────────────────

#[test]
fn a_nonce_in_a_gitignored_file_is_absent_from_the_store_bytes() {
    let repo = TestRepo::new();
    let nonce = runtime_nonce("SECRETNONCE");
    let control = runtime_nonce("CONTROLNONCE");

    // `secrets/` is in .gitignore. Plant the nonce there, and plant an
    // unrelated marker in an INDEXED file — the control that proves this
    // assertion can go red at all.
    write(
        &repo.root,
        "secrets/credentials.rs",
        &format!("pub const TOKEN: &str = \"{nonce}\";\n"),
    );
    write(
        &repo.root,
        "src/control.rs",
        &format!("pub const CONTROL: &str = \"{control}\";\n"),
    );

    repo.refresh();
    let bytes = repo.store_bytes();

    // Control first: if the indexed marker is NOT in the store, the store
    // holds no content at all and the isolation assertion below would be
    // vacuously true. This makes the gate capable of failing.
    assert!(
        contains(&bytes, control.as_bytes()),
        "the CONTROL marker from an indexed file is missing from the store — \
         the isolation assertion that follows would be vacuous"
    );

    assert!(
        !contains(&bytes, nonce.as_bytes()),
        "a run-time nonce planted in a gitignored file was found in the \
         store's own bytes: the excluded file was READ into the index"
    );

    // And it is not merely filtered at query time — it is not there at all.
    let store = repo.open();
    assert!(
        store
            .lookup(Path::new("secrets/credentials.rs"))
            .expect("lookup")
            .is_none()
    );
    let outcome = wcore_repomap::search(&store, &SearchQuery::new(nonce.clone())).expect("search");
    assert!(outcome.hits.is_empty(), "{:?}", outcome.hits);
}

// ── 7. Nothing outside the indexed root is stored, symlinks included ─────

#[test]
#[cfg(unix)]
fn a_symlink_pointing_outside_the_root_is_never_read_into_the_store() {
    let repo = TestRepo::new();
    let nonce = runtime_nonce("OUTSIDENONCE");
    let control = runtime_nonce("INSIDENONCE");

    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_file = outside.path().join("outside.rs");
    fs::write(
        &outside_file,
        format!("pub const OUTSIDE: &str = \"{nonce}\";\n"),
    )
    .expect("write outside");

    std::os::unix::fs::symlink(&outside_file, repo.root.join("src/link.rs")).expect("symlink");
    // A control marker inside the root, so a store that stored nothing at all
    // could not pass this test vacuously.
    write(
        &repo.root,
        "src/control.rs",
        &format!("pub const CONTROL: &str = \"{control}\";\n"),
    );

    repo.refresh();
    let bytes = repo.store_bytes();
    assert!(
        contains(&bytes, control.as_bytes()),
        "control marker missing — the assertion below would be vacuous"
    );
    assert!(
        !contains(&bytes, nonce.as_bytes()),
        "content from OUTSIDE the indexed root reached the store through a symlink"
    );
    assert!(
        repo.open()
            .lookup(Path::new("src/link.rs"))
            .expect("lookup")
            .is_none(),
        "the symlink itself must not be recorded"
    );
}

// ── 8. Every stored path round-trips, including the Windows-fragile shapes ─

#[test]
fn stored_paths_round_trip_for_non_ascii_and_deeply_nested_names() {
    let repo = TestRepo::new();

    // The two shapes the Windows path-simplification helper conditionally
    // no-ops on. An index that stores one representation and looks up
    // another passes every Linux test and silently misses on Windows, so
    // these are asserted through the real store on whatever host runs them.
    let non_ascii = "src/ünïcode/módulo.rs";
    write(&repo.root, non_ascii, "pub fn unicode_function() {}\n");

    let deep_dirs: PathBuf = (0..18).map(|i| format!("lvl{i}")).collect();
    let deep = format!(
        "src/{}/leaf.rs",
        deep_dirs.to_string_lossy().replace('\\', "/")
    );
    write(&repo.root, &deep, "pub fn deep_function() {}\n");

    repo.refresh();
    let store = repo.open();

    for rel in [non_ascii, deep.as_str()] {
        let record = store
            .lookup(Path::new(rel))
            .unwrap_or_else(|e| panic!("lookup {rel}: {e}"))
            .unwrap_or_else(|| panic!("{rel} did not round-trip through the store"));
        assert_eq!(record.path, rel, "stored key must equal the lookup key");
        // And through the platform's own separator, which is what a caller
        // that built the path with `Path::join` will actually hand in.
        let native: PathBuf = rel.split('/').collect();
        assert!(
            store.lookup(&native).expect("native lookup").is_some(),
            "{rel} did not resolve when looked up as a native path"
        );
    }
}

// ── 9. A corrupt store is a structured error, never a panic or an empty index ─

#[test]
fn a_corrupt_store_returns_a_structured_error_naming_the_file() {
    let repo = TestRepo::new();
    repo.refresh();
    assert_eq!(repo.open().record_count().expect("count"), 3);

    // Truncate the database to garbage, the shape a partial copy or an
    // interrupted write produces.
    fs::write(&repo.store_path, b"this is not a sqlite database at all").expect("corrupt");
    for suffix in ["-wal", "-shm"] {
        let mut path = repo.store_path.clone().into_os_string();
        path.push(suffix);
        let _ = fs::remove_file(PathBuf::from(path));
    }

    match IndexStore::open(&repo.store_path, &repo.root) {
        Err(RepoMapError::Store { path, message }) => {
            assert_eq!(path, repo.store_path, "the error must name the store file");
            assert!(
                message.to_lowercase().contains("rebuild"),
                "the error must offer the remedy: {message}"
            );
        }
        Err(other) => panic!("expected RepoMapError::Store, got {other:?}"),
        Ok(store) => panic!(
            "a corrupt store opened successfully and reported {} records — \
             an unreadable index must never present as an empty one",
            store.record_count().unwrap_or(0)
        ),
    }
}

#[test]
fn a_store_written_by_a_future_schema_is_refused_with_a_rebuild_hint() {
    let repo = TestRepo::new();
    repo.refresh();
    // Written through a plain SQLite connection rather than through a
    // test-only method on the store, so the production type exposes no
    // mutation hole that exists only for tests.
    {
        let conn = rusqlite::Connection::open(&repo.store_path).expect("open raw");
        conn.execute(
            "UPDATE meta SET value = '9999' WHERE key = 'schema_version'",
            [],
        )
        .expect("bump schema version");
    }
    match IndexStore::open(&repo.store_path, &repo.root) {
        Err(RepoMapError::Store { message, .. }) => {
            assert!(message.contains("9999"), "{message}");
            assert!(message.contains("rebuild"), "{message}");
        }
        Err(other) => panic!("expected a schema refusal, got {other:?}"),
        Ok(_) => panic!(
            "a store carrying schema_version 9999 opened successfully — a \
             version this build does not understand must be refused, not \
             silently read with the wrong assumptions"
        ),
    }
}

// ── 10. The pre-existing public API is untouched ─────────────────────────

#[test]
fn the_existing_repomap_build_entry_point_behaves_exactly_as_before() {
    let repo = TestRepo::new();
    repo.refresh();

    // Same tree, same gitignore, same expectations the pre-persistence
    // fixture suite already pins — asserted here so a regression in the
    // shared walk shows up beside the persistent index that now uses it.
    let map = RepoMap::build(&repo.root).expect("build");
    let paths: Vec<String> = map
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(paths.contains(&"src/alpha.rs".to_string()), "{paths:?}");
    assert!(paths.contains(&"src/beta.rs".to_string()), "{paths:?}");
    assert!(paths.contains(&".gitignore".to_string()), "{paths:?}");
    assert!(
        !paths.iter().any(|p| p.starts_with("secrets/")),
        "{paths:?}"
    );

    let alpha = map
        .files
        .iter()
        .find(|f| f.path.to_string_lossy().replace('\\', "/") == "src/alpha.rs")
        .expect("alpha present");
    assert_eq!(alpha.language, wcore_repomap::Language::Rust);
    assert_eq!(alpha.symbols.len(), 2);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
