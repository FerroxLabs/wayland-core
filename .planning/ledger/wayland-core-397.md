---
issue: 397
repo: FerroxLabs/wayland-core
kind: defect
title: "Stale doc comment on CredentialsBackend::Auto claims a plaintext fallback the code refuses"
status: open
last_verified_commit: 488fbbae9
criteria:
  - id: c1
    text: "The doc comment at crates/wcore-config/src/credentials.rs:40-47 and the module header at :8-12 state the fail-closed behaviour the implementation actually has: build_ladder mounts keyring then encrypted vault and nothing else, and put refuses when no secure rung is mounted."
    state: met
    evidence: "test:crates/wcore-config/src/credentials.rs::the_documented_ladder_matches_the_rungs_build_ladder_mounts"
    owner: core
    note: "MET. CredentialsBackend::Auto's doc comment and the module header both state the fail-closed ladder: keyring -> encrypted vault -> refuse, with 'It does not fall back to the plaintext file' said in as many words, and the read path (which descends one rung further into the legacy file and promotes upward) described separately so the two are not confused."
  - id: c2
    text: "A test or a grep gate fails when the documented ladder and the rungs build_ladder actually mounts disagree again. The failure mode here is 2,650 lines of distance between the claim and the code, and distance does not shrink on its own."
    state: met
    evidence: "test:crates/wcore-config/src/credentials.rs::a_ladder_with_no_secure_rung_refuses_the_write_and_writes_no_cleartext"
    owner: core
    note: "MET, by two arms because the claim has two halves. (a) The rung list: Auto declares `ladder: keyring -> encrypted_vault -> refuse` and the_documented_ladder_matches_the_rungs_build_ladder_mounts derives the actual rungs from the store types build_ladder constructs, enumerating the file's *CredentialsStore structs rather than matching a short list, so a rung from a NEW store type cannot slip through. RED ARM, verbatim, with the v0.13.11 claim written into the declaration: `assertion `left == right` failed ... left: ['keyring', 'plaintext'] right: ['keyring', 'encrypted_vault']`. (b) The terminal word is measured as BEHAVIOUR: a ladder with no keyring and no vault refuses the put and leaves no plaintext file, with a wrong-refusal control that the legacy READ rung still resolves a pre-existing key."
  - id: c3
    text: "The correction is verified at tag v0.13.11, where the claim was made, AND at HEAD -- so it is not graded on a tree where the comment already differed."
    state: met
    evidence: "commit:08f06dc4e"
    owner: core
    note: "MET. Verified at the tag where the claim was made AND at HEAD. `git show v0.13.11:crates/wcore-config/src/credentials.rs` carries the stale sentence byte-identically ('Default: prefer the OS keyring, transparently falling back to the plaintext `0o600` file when no keyring is available'), and at that same tag build_ladder mounts only KeyringCredentialsStore and EncryptedFileCredentialsStore and put ends in `Err(no_secure_backend_for_write(key))` -- so the doc and the code disagreed AT THE TAG, not only at HEAD. The same stale text was present at integ/f13 HEAD before this change and is what was replaced."
  - id: c4
    text: "The plaintext store's real status is stated where the stale comment was: read-and-delete-only legacy, or an explicit backend = plaintext opt-out that warns on stderr. Deleting the false sentence without stating the true one invites the next reader to guess again."
    state: met
    evidence: "file:crates/wcore-config/src/credentials.rs"
    owner: core
    note: "MET. The module header now states the plaintext store's real status in both of its remaining roles rather than only deleting the false sentence: read-and-delete-only legacy (the ladder READS it so pre-existing keys are not stranded and promotes a hit upward; nothing new is ever written to it by the ladder), and an explicit `backend = 'plaintext'` opt-out that warns on stderr, which OAuth token sets bypass by opening the ladder through open_secure_ladder_store. The Auto doc repeats the opt-out pointer at the site the stale claim occupied."

---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records, and were GRADED 2026-08-31 by lane f13-authority.

The correction is not only prose: the claim is now machine-checked. `Auto`
declares `ladder: keyring -> encrypted_vault -> refuse` and a test derives the
actual rungs from the store types `build_ladder` constructs, so the 2,650 lines
between the claim and the code stop mattering.

Not cosmetic. In one afternoon this comment produced a false Core falls back to
plaintext credentials claim in THREE independent readings -- two external audit
models and one drafting pass -- and came within one review of being published
in a public threat model, conceding a security weakness fixed nine releases
ago. The cost of this defect is already measured and it is not zero.
## Independently re-verified 2026-08-31 by lane f13-authority at 488fbbae9

c2's red arm was RE-RUN on BOTH halves of the claim, because the claim has two.

Rung list -- the declaration changed to `ladder: keyring -> plaintext -> refuse`:

    panicked at crates/wcore-config/src/credentials.rs:3617:9:
    assertion `left == right` failed: the `ladder:` line in
    `CredentialsBackend::Auto`'s doc comment and the rungs `build_ladder` mounts
    disagree (FerroxLabs/wayland-core#397)
      left: ["keyring", "plaintext"]
     right: ["keyring", "encrypted_vault"]

Terminal word -- the declaration changed to `ladder: keyring -> plaintext`:

    panicked at crates/wcore-config/src/credentials.rs:3612:9:
    assertion `left == right` failed: #397: the ladder's terminal behaviour is
    REFUSE. A declaration ending any other way is claiming a fallback the code
    does not have
      left: "plaintext"
     right: "refuse"

**Correction to the first pass's wording on c3.** The stale sentence IS present
at `v0.13.11`, but NOT "byte-identically": it is line-wrapped there as
`transparently falling back to the` / `//!   plaintext `0o600` file`. A one-line
grep for the sentence returns EMPTY at the tag, which reads as absence and is
not. At that same tag `build_ladder`'s write path still ends in
`Err(no_secure_backend_for_write(key))` (`credentials.rs:1707`), so doc and code
did disagree AT THE TAG and not only at HEAD. c3 holds as written; only the word
"byte-identically" was wrong, and it is corrected here rather than left standing.
