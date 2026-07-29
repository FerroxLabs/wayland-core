# RELEASE-TRUST-ROOT — working notes

Lane `release-trust-root`. Branch `lane/release-trust-root`. Base (captured once, quoted
everywhere): `63481a2d2931e888dce7214c6968b95e26f9e10d`.

Seam requests addressed: **SR-29-9**, **SR-29-11**. Both are the disposition of open HIGH
**F29-03-01** (`self-update` installs nothing).

Append-and-recommit after every measurement. No partial credit for uncommitted reasoning.

---

## Plan

1. Review the two inherited uncommitted edits adversarially. Commit or repair.
2. Prove the substitution compiles + passes on `hetzner-dsm` (never the Mac).
3. Wire `.github/workflows/release.yml` per SR-29-9 (manifest-build + manifest-sign,
   seed on stdin only, `-release-manifest.json` suffix, monotonic `--sequence`).
4. End-to-end proof with a **throwaway** run-time key through the real `ReleaseVerifier`:
   one ACCEPT control plus every refusal (rollback, stale/frozen, revoked, retired key,
   wrong role).
5. Write `.planning/RELEASE-TRUST-ROOT.md`, commit, push.

Fences honoured: no `ci.yml` (owned by lane `ci-macos-budget`), no merge/PR/tag/release,
no `wcore-contract generate`, no real release run triggered.

---

## Measurement log

### M1 — worktree identity (2026-07-29)

```
/usr/bin/git rev-parse --show-toplevel
  -> /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-release-trust-root
/usr/bin/git rev-parse --abbrev-ref HEAD  -> lane/release-trust-root
/usr/bin/git merge-base HEAD plan/f20-unified-audit-repair -> 63481a2d2931e888...
```

Dirty at entry, exactly two files, both inherited:
`crates/wcore-cli/src/update_trust.rs`, `crates/wcore-cli/tests/self_update_trust.rs`.

### M2 — the bundled key is structurally a real Ed25519 public key

`public_key_base64 = ycwkW1xZnCxruh59zJnQiuoN5xuXYkMurhquhHMBXXY=` decodes (strict base64)
to **32 bytes**, `all_zero = false`. So it clears both placeholder refusals in
`ReleaseVerifier::with_trust_root` (empty key set; all-zeros identity point).

### M3 — the "only bundle release_acceptance" reasoning CHECKS OUT against the code

`update_trust.rs:588` — `ReleaseVerifier::resolve` refuses any key whose `role !=
RELEASE_MANIFEST_ROLE`, and `RELEASE_MANIFEST_ROLE = "release_acceptance"`
(`update_trust.rs:83`). `resolve` is the ONLY path from a manifest to a `VerifyingKey`
(`verify_manifest_json:567`). So `packaging`, `deployment_preparation` and
`rollback_rehearsal` keys in the bundled root could never authorise an install; bundling
them would be pure trust surface. **Reasoning sustained — bundling one key is correct.**

Consequence to state, not hide: the `RoleMismatch` refusal is now unreachable *via
`bundled()`*. It stays reachable via `with_trust_root_json`, and the corpus must keep
proving it there. Recorded as a deliberate consequence, not an accident.

### M4 — the test edit is PARTLY over-weakened; one deleted assertion must come back

`UpdateTrustError::PlaceholderTrustRoot`'s `#[error(...)]` string (`update_trust.rs:122-126`)
names `RELEASE_TRUST_ROOT_JSON` **unconditionally** — it is a static format string, not
derived from which root was passed. So the deleted assertion
`message.contains("RELEASE_TRUST_ROOT_JSON")` **still passes against an injected empty
root**. Deleting it removed a live guard for no reason. **Restore it.**

The rest of the split survives review: reverting the constant to `"keys":[]` tomorrow makes
`bundled()` return `Err`, which panics `the_bundled_trust_root_is_real_...`'s `.expect()`
AND fails its `keys.len() == 1` assertion. The placeholder path stays genuinely gated.

### M5 — stale module doc

`tests/self_update_trust.rs:16-17` still asserts in prose "The bundled production trust root
is EMPTY on purpose and this file proves it is refused." False as of the substitution. Must
be corrected or the file documents a guard it no longer has.

<!-- appended below as work proceeds -->
