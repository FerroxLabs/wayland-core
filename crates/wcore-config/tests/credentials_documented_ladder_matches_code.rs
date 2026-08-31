//! `wayland-core#397` c2/c3/c4 — the documented credential ladder must agree
//! with the rungs `build_ladder` actually mounts.
//!
//! # The defect this exists to make impossible
//!
//! At tag `v0.13.11` the doc comment on `CredentialsBackend::Auto`
//! (`src/credentials.rs:40-47`) and the module header (`:8-12`) said the
//! default backend fell back to the plaintext `0o600` file when no keyring
//! was available. The implementation, ~2,650 lines further down, refuses:
//! `build_ladder` mounts keyring then encrypted vault and nothing else, and
//! `LadderCredentialsStore::put` returns `no_secure_backend_for_write` rather
//! than descending to cleartext.
//!
//! That is not a cosmetic staleness. In one afternoon the comment produced a
//! false "Core falls back to plaintext credentials" claim in three
//! independent readings — two external audit models and one drafting pass —
//! and came within one review of being published in a public threat model,
//! conceding a security weakness fixed nine releases earlier.
//!
//! # Why a gate and not just a rewrite
//!
//! The failure mode is DISTANCE: 2,650 lines between the claim and the code,
//! and distance does not shrink on its own. So the claim is made machine
//! readable — one `LADDER:` line, carried in BOTH doc blocks — and compared
//! against a ladder derived from the source of `build_ladder` and of
//! `LadderCredentialsStore::put`. A rung added to the code, or a rung
//! rewritten in the prose, reddens here.
//!
//! Two red arms are checked in, not merely described:
//!
//! * `the_gate_reddens_on_the_v0_13_11_text` feeds the VERBATIM doc blocks
//!   from the tag where the false claim was made
//!   (`tests/fixtures/credentials_v0_13_11_doc_blocks.txt`) to the same
//!   grader the live blocks go through, and asserts it names the
//!   plaintext-fallback sentence. That is `#397` c3's "verified at v0.13.11"
//!   arm, made permanent instead of run once.
//! * The live arm is the wrong-refusal control for it: the corrected text
//!   must grade CLEAN, so a gate that refuses every wording is caught here
//!   rather than by the next author.

use std::path::Path;

/// The machine-readable half of the claim. Both doc blocks carry one line
/// beginning with this, and it is compared against [`actual_ladder`].
const LADDER_MARKER: &str = "LADDER:";

fn source() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/credentials.rs"))
        .expect("this crate's credentials.rs is readable from its own test")
}

fn fixture() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/credentials_v0_13_11_doc_blocks.txt"),
    )
    .expect("the v0.13.11 fixture is checked in beside this test")
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// The leading `//!` run: the module header the issue names at `:8-12`.
fn module_header(src: &str) -> String {
    src.lines()
        .take_while(|l| l.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `///` run immediately above `#[default] Auto,` — the doc comment the
/// issue names at `:40-47`. Anchored on the variant rather than on a line
/// number so it cannot drift onto a neighbour.
fn auto_doc(src: &str) -> String {
    let anchor = "    #[default]\n    Auto,";
    let at = src
        .find(anchor)
        .expect("`CredentialsBackend::Auto` must be findable by its `#[default]` attribute");
    let before: Vec<&str> = src[..at].lines().collect();
    let mut out: Vec<&str> = Vec::new();
    for line in before.iter().rev() {
        if line.starts_with("    ///") {
            out.push(line);
        } else {
            break;
        }
    }
    out.reverse();
    out.join("\n")
}

/// A whole free function's source, from its signature to the closing brace in
/// column 0.
fn free_fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
    let at = src
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` must be findable"));
    let rest = &src[at..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("`{signature}` must close at column 0"));
    &rest[..end]
}

/// `LadderCredentialsStore::put` — the write ladder itself. Scoped to the
/// `impl CredentialsStore for LadderCredentialsStore` block so it cannot pick
/// up another store's `put`.
fn ladder_put_body(src: &str) -> &str {
    let at = src
        .find("impl CredentialsStore for LadderCredentialsStore {")
        .expect("the ladder's `CredentialsStore` impl must be findable");
    let block = &src[at..];
    let put = block
        .find("    fn put(&self")
        .expect("the ladder must have a `put`");
    let rest = &block[put..];
    let end = rest
        .find("\n    }\n")
        .expect("the ladder's `put` must close at impl-member indentation");
    &rest[..end]
}

// ---------------------------------------------------------------------------
// The two halves that must agree
// ---------------------------------------------------------------------------

/// The ladder the CODE implements: the rungs `build_ladder` mounts as write
/// targets, then the terminal `put` takes when none of them is mounted.
///
/// Read off constructor calls rather than off a list somebody maintains, so a
/// fourth rung mounted tomorrow changes this string without anyone editing
/// this file.
fn actual_ladder(src: &str) -> String {
    let build = free_fn_body(
        src,
        "fn build_ladder(cfg: &CredentialsStorageConfig, plaintext_path: &Path)",
    );
    // POSITIVE CONTROL on the extraction. A `find` that drifted would return a
    // stub, and a stub mounts nothing — which would make the comparison below
    // pass or fail for reasons having nothing to do with the ladder.
    assert!(
        build.len() > 500 && build.contains("LadderCredentialsStore::new"),
        "the extracted `build_ladder` is {} bytes and does not look like the \
         real one; this test is reading the wrong thing: {build:?}",
        build.len()
    );

    let mut rungs: Vec<&str> = Vec::new();
    for (ctor, name) in [
        ("KeyringCredentialsStore::new", "keyring"),
        ("EncryptedFileCredentialsStore::new", "encrypted vault"),
        ("PlaintextCredentialsStore::new", "plaintext"),
    ] {
        if build.contains(ctor) {
            rungs.push(name);
        }
    }
    assert!(
        rungs.len() >= 2,
        "`build_ladder` appears to mount {rungs:?}; fewer than two rungs means \
         the constructor names this test scans for have been renamed and it is \
         now grading nothing"
    );

    let put = ladder_put_body(src);
    assert!(
        put.contains("self.keyring") && put.contains("self.vault"),
        "the extracted `put` does not descend the ladder; this test is reading \
         the wrong function: {put:?}"
    );
    // The terminal. A `put` that reaches the legacy cleartext store has a
    // plaintext rung whatever the prose says; one that returns
    // `no_secure_backend_for_write` refuses.
    let terminal = if put.contains("legacy.put(") {
        "plaintext"
    } else if put.contains("no_secure_backend_for_write") {
        "refuse"
    } else {
        "UNKNOWN — `put` neither writes cleartext nor returns \
         `no_secure_backend_for_write`; this gate cannot tell what it does"
    };
    rungs.push(terminal);
    rungs.join(" -> ")
}

/// The ladder a doc block CLAIMS, from its `LADDER:` line.
fn documented_ladder(block: &str) -> Option<String> {
    block.lines().find_map(|line| {
        line.find(LADDER_MARKER)
            .map(|at| line[at + LADDER_MARKER.len()..].trim().to_owned())
    })
}

/// Doc-comment text as prose: markers stripped, wrapped lines rejoined.
fn prose(block: &str) -> String {
    block
        .lines()
        .map(|l| {
            let l = l.trim_start();
            l.strip_prefix("//!")
                .or_else(|| l.strip_prefix("///"))
                .unwrap_or(l)
                .trim()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Everything wrong with one doc block, named rather than counted.
///
/// Three rules, each with a red arm in this file:
/// 1. it must carry the machine-readable `LADDER:` claim;
/// 2. no sentence may pair cleartext storage with a fall-back, unless it is
///    denying one — the exact shape of the `v0.13.11` sentence;
/// 3. it must state the plaintext store's REAL status, because deleting a
///    false sentence without stating the true one invites the next guess
///    (`#397` c4).
fn grade_doc_block(block: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let text = prose(block);

    if documented_ladder(block).is_none() {
        violations.push(format!(
            "no `{LADDER_MARKER}` claim: this block describes the default \
             credential backend and nothing here can be compared against the \
             code"
        ));
    }

    for sentence in text.split(". ") {
        let lower = sentence.to_lowercase();
        let cleartext = ["plaintext", "cleartext"]
            .iter()
            .any(|w| lower.contains(*w));
        let fallback = ["fall back", "falling back", "fallback", "falls back"]
            .iter()
            .any(|w| lower.contains(w));
        let denied = ["never", "not ", "no longer"]
            .iter()
            .any(|w| lower.contains(*w));
        if cleartext && fallback && !denied {
            violations.push(format!(
                "claims a cleartext fallback the code refuses — \
                 `build_ladder` mounts keyring then encrypted vault and \
                 nothing else, and `put` returns \
                 `no_secure_backend_for_write`: {sentence:?}"
            ));
        }
    }

    let lower = text.to_lowercase();
    if !lower.contains("read-and-delete-only") {
        violations.push(
            "does not state the plaintext store's real status \
             (`read-and-delete-only` under the default ladder); #397 c4 — \
             deleting the false sentence without stating the true one invites \
             the next reader to guess again"
                .to_owned(),
        );
    }
    if !text.contains("backend = \"plaintext\"") {
        violations.push(
            "does not name the explicit `backend = \"plaintext\"` opt-out, so a \
             reader cannot tell how cleartext writing is reached at all"
                .to_owned(),
        );
    }

    violations
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[test]
fn the_documented_ladder_matches_the_rungs_build_ladder_mounts() {
    let src = source();
    let header = module_header(&src);
    let auto = auto_doc(&src);

    // POSITIVE CONTROLS on both extractions. An empty block grades clean
    // against every rule above, which is the vacuity this file exists to
    // close.
    assert!(
        header.len() > 400,
        "the module header extracted to {} bytes; the scanner is not reading \
         it: {header:?}",
        header.len()
    );
    assert!(
        auto.len() > 400,
        "the `Auto` doc comment extracted to {} bytes; the scanner is not \
         reading it: {auto:?}",
        auto.len()
    );

    let actual = actual_ladder(&src);
    for (name, block) in [
        ("module header", &header),
        ("CredentialsBackend::Auto", &auto),
    ] {
        let violations = grade_doc_block(block);
        assert!(
            violations.is_empty(),
            "the {name} doc block is wrong about the credential ladder:\n  - {}",
            violations.join("\n  - ")
        );
        assert_eq!(
            documented_ladder(block).as_deref(),
            Some(actual.as_str()),
            "the {name} doc block claims a ladder the code does not implement. \
             `build_ladder`'s mounted rungs plus `put`'s terminal are \
             {actual:?}. Fix whichever half is wrong — the whole point of \
             this gate is that the claim and the code sit ~2,650 lines apart."
        );
    }
}

/// `#397` c3 — the correction is verified against the tree where the claim was
/// MADE, not only against a tree where the comment already differed.
///
/// The fixture is the two doc blocks at tag `v0.13.11`, copied verbatim. They
/// go through the same `grade_doc_block` the live blocks do, so this is a red
/// arm on the gate itself and not a second, weaker check.
#[test]
fn the_gate_reddens_on_the_v0_13_11_text() {
    let fixture = format!("\n{}", fixture());
    let blocks: Vec<&str> = fixture
        .split("\n===")
        .skip(1)
        .filter_map(|s| s.split_once("===\n").map(|(_, body)| body))
        .collect();
    // POSITIVE CONTROL on the fixture parse. Zero blocks would make every
    // assertion below vacuous, and an unparsed fixture reads exactly like a
    // clean one.
    assert_eq!(
        blocks.len(),
        2,
        "the v0.13.11 fixture must yield exactly the two doc blocks #397 \
         names; got {}",
        blocks.len()
    );

    let mut fallback_claims = 0usize;
    for block in &blocks {
        assert!(
            block.len() > 200,
            "a fixture block extracted to {} bytes: {block:?}",
            block.len()
        );
        let violations = grade_doc_block(block);
        assert!(
            !violations.is_empty(),
            "the v0.13.11 text graded CLEAN. That text is the defect #397 was \
             filed for, so a gate that accepts it grades nothing: {block:?}"
        );
        fallback_claims += violations
            .iter()
            .filter(|v| v.contains("claims a cleartext fallback"))
            .count();
    }
    // Not merely "something was wrong with it": the rule that must fire is the
    // one about the sentence the issue quotes. Both v0.13.11 blocks carry one
    // — the module header's "The fallback half of the default `Auto` backend"
    // and the variant's "transparently falling back to the plaintext file".
    assert!(
        fallback_claims >= 2,
        "the plaintext-fallback rule fired {fallback_claims} times on the \
         v0.13.11 text; both blocks carry the claim, so a gate that catches \
         fewer is catching them for the wrong reason"
    );
}
