---
issue: 397
repo: FerroxLabs/wayland-core
kind: defect
title: "Stale doc comment on CredentialsBackend::Auto claims a plaintext fallback the code refuses"
status: open
last_verified_commit: cfcf97d0
criteria:
  - id: c1
    text: "The doc comment at crates/wcore-config/src/credentials.rs:40-47 and the module header at :8-12 state the fail-closed behaviour the implementation actually has: build_ladder mounts keyring then encrypted vault and nothing else, and put refuses when no secure rung is mounted."
    state: met
    evidence: "file:crates/wcore-config/src/credentials.rs:61:nothing else as a write target"
    owner: core
    note: "MET cfcf97d0. Both blocks the issue names were rewritten and now describe the ladder the code implements. READ OFF THE CODE, not off the issue prose: build_ladder (credentials.rs:2635-2688) constructs KeyringCredentialsStore and EncryptedFileCredentialsStore and passes the plaintext PATH to LadderCredentialsStore::new, where it lands in the `legacy` field the struct itself documents as read-and-delete-only; LadderCredentialsStore::put (:1679-1708) has arms for self.keyring and self.vault and a terminal Err(no_secure_backend_for_write(key)) with no plaintext arm at all. The module header (:6-33) and the Auto doc (:53-75) now carry the same machine-readable claim -- LADDER: keyring -> encrypted vault -> refuse -- plus the isolated-profile carve-out (WAYLAND_HOME skips the keyring), the refusal terminal, and the stderr warning warn_no_secure_credential_tier prints. The false sentence is gone from both. It cannot come back silently: c2. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c2
    text: "A test or a grep gate fails when the documented ladder and the rungs build_ladder actually mounts disagree again. The failure mode here is 2,650 lines of distance between the claim and the code, and distance does not shrink on its own."
    state: met
    evidence: "test:crates/wcore-config/tests/credentials_documented_ladder_matches_code.rs::the_documented_ladder_matches_the_rungs_build_ladder_mounts"
    owner: core
    note: "MET cfcf97d0. The gate compares TWO DERIVED THINGS, so it fails whichever half moves: (a) the ladder the doc blocks CLAIM, read from the LADDER: line each carries; (b) the ladder the CODE implements, derived from the store constructors build_ladder mentions plus the terminal LadderCredentialsStore::put takes. Nothing is hardcoded, so a fourth rung mounted tomorrow changes (b) with nobody editing the test. THREE POSITIVE CONTROLS run before the comparison, because an empty extraction grades clean against every rule: build_ladder must extract to >500 bytes and mention LadderCredentialsStore::new, put must mention self.keyring and self.vault, and both doc blocks must extract to >400 bytes. TWO RED ARMS, each with cargo check RC=0 FIRST so the red is behaviour and not a build break. RED-A, code side: an `if self.legacy.put(key, value).is_ok() { return Ok(()); }` arm added to put (MUTATION_SITES=1, CHECK_EXIT=0) -> TESTS_EXIT=100, naming actual 'keyring -> encrypted vault -> plaintext' against the documented 'keyring -> encrypted vault -> refuse'. RED-B, doc side: the Auto block's LADDER line changed to end in plaintext (MUTATION_SITES=1, CHECK_EXIT=0) -> TESTS_EXIT=100, naming the reverse mismatch. Both restores blob-verified equal to the HEAD blob (53bdad586ea979ae431b6763b7ebd29d4f85b918), tree clean after. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c3
    text: "The correction is verified at tag v0.13.11, where the claim was made, AND at HEAD -- so it is not graded on a tree where the comment already differed."
    state: met
    evidence: "test:crates/wcore-config/tests/credentials_documented_ladder_matches_code.rs::the_gate_reddens_on_the_v0_13_11_text"
    owner: core
    note: "MET cfcf97d0. AT v0.13.11: git show v0.13.11:crates/wcore-config/src/credentials.rs was read directly, and its module header (:1-21) and Auto doc (:40-47) are BYTE-IDENTICAL to the text at this lane's base ca15a48bf -- so the correction was made against the tree where the claim was made, and this is not a grade taken on a tree where the comment had already drifted. Those two blocks are checked in verbatim at crates/wcore-config/tests/fixtures/credentials_v0_13_11_doc_blocks.txt and fed to the SAME grade_doc_block the live blocks go through, so the v0.13.11 arm is permanent rather than a run somebody did once. It asserts not merely that the old text is flagged but that the plaintext-fallback rule specifically fires at least twice -- the header's 'The fallback half of the default Auto backend' and the variant's 'transparently falling back to the plaintext 0o600 file' -- so a gate that reddened on the old text for an unrelated reason would not pass. AT HEAD: the corrected blocks grade CLEAN through the same function, which is the wrong-refusal control for it; a gate that refused every wording would be caught here rather than by the next author. The fixture parse itself is controlled: exactly 2 blocks, each >200 bytes, or the test fails. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c4
    text: "The plaintext store's real status is stated where the stale comment was: read-and-delete-only legacy, or an explicit backend = plaintext opt-out that warns on stderr. Deleting the false sentence without stating the true one invites the next reader to guess again."
    state: met
    evidence: "file:crates/wcore-config/src/credentials.rs:65:READ-AND-DELETE-ONLY here"
    owner: core
    note: "MET cfcf97d0. The real status is stated where the stale sentence was, in BOTH blocks, and it is the status the code has: under Auto the legacy 0o600 file is READ-AND-DELETE-ONLY -- reads descend to it (LadderCredentialsStore::get, :1618-1622), a value found there is promoted to the top mounted rung and then purged below (promote, :1507-1550, which does write-new then verify-readback then delete-old), and put never writes to it. Writing cleartext is reachable only through the explicit backend = plaintext opt-out, which open_store (:2735-2740) gates behind warn_explicit_plaintext_backend -- an eprintln! to real stderr, not a tracing::warn!, which with RUST_LOG unset would reach nobody. THIS HALF IS GATED TOO, not merely written: grade_doc_block requires each block to contain read-and-delete-only AND to name backend = plaintext, so deleting the true sentence reddens the same test that catches re-introducing the false one. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

Not cosmetic. In one afternoon this comment produced a false Core falls back to
plaintext credentials claim in THREE independent readings -- two external audit
models and one drafting pass -- and came within one review of being published
in a public threat model, conceding a security weakness fixed nine releases
ago. The cost of this defect is already measured and it is not zero.

## Graded by lane doc-truth, cfcf97d0

All four criteria met on `lane/f13-s2-doc-truth`, cut from `integ/f13` at `ca15a48bf`.
NOT CLOSED -- that is a maintainer action.

What was changed: `crates/wcore-config/src/credentials.rs` (two doc blocks),
`crates/wcore-config/tests/credentials_documented_ladder_matches_code.rs` (new gate),
`crates/wcore-config/tests/fixtures/credentials_v0_13_11_doc_blocks.txt` (the historical
text, as a permanent red arm). No production behaviour changed; this ticket is about what
the product SAYS.

Stated because it bounds the claim: the gate is a SOURCE-TEXT check, not a semantic one.
It proves the two doc blocks and `build_ladder`/`put` agree on the rung list and the
terminal. It cannot prove the prose around the `LADDER:` line is true, and it does not
watch any other file -- a fresh false claim about the credential ladder written in, say,
`config.rs` would not redden it. That boundary is deliberate (widening it to the workspace
turns a decidable check into a maintained denylist) and is written down here rather than
discovered later.
