# HANDOFF — v0.12.26-rc.1, the release PR is open — 2026-08-03 (session 2)

Setup: `export WL_LANE=core`; `gh auth switch --user FerroxLabs` before EVERY gh op.
Supersedes `.planning/HANDOFF-2026-08-03.md` §0/§0b. Everything else there still holds.

---

## 0. ONE LINE

**PR #257 is open (integration → `main`). The bar is met. Four CI checks failed;
three are fixed and waiting on `lane/osv-bumps`; the fourth is unreadable until
the three-platform matrix finishes.** Merging and tagging are Sean's.

---

## 1. THE MAP — every SHA you need

| What | Ref | SHA |
|---|---|---|
| Integration (PR head) | `plan/f20-unified-audit-repair` | `c615daed` |
| Release PR | https://github.com/FerroxLabs/wayland-core/pull/257 | → `main` |
| `main` | | `61b79c4f` — **3,143 commits behind** |
| Fixes waiting to merge | `lane/osv-bumps` | `4c08ac1e` |
| — the lockfile bump | | `3750c916` |
| — the CI dbus fix | | `4c08ac1e` |
| #170 merge (already in) | | `4160bf76` |

**Gate on `3750c916`** (the lockfile bump, run in `/root/orch-gate` on hetzner):
`FMT_OK · LOCKED_OK · CLIPPY_RC=0 · CLIPPY_POSITIVE_CONTROL_RC=101 ·
PID_GATE_RC=0 · ratchet 31/31 · 13904 tests run: 13904 passed, 75 skipped`.
`4c08ac1e` differs only by `.github/workflows/supply-chain.yml`, which cannot
affect a cargo build — but re-gate if you want the ceremony.

---

## 2. THE FOUR CI FAILURES, and what each one actually was

PR #257 at last look: **12 pass, 4 fail, 3 pending, 1 skipping.**

| Check | Real cause | Fix |
|---|---|---|
| `scan` (OSV) | `serde_with 3.20.0` (GHSA-7gcf-g7xr-8hxj, 5.1) and `wasmtime 36.0.12` (RUSTSEC-2026-0222, 3.8) — both patched upstream | `3750c916`, lockfile only → 3.21.0 / 36.0.13 |
| `Dependency policy (deny.toml)` | **The same two advisories.** `advisories FAILED, bans ok, licenses ok, sources ok` | Same commit — one fix closes two checks |
| `SBOM byte-determinism` | **Not a determinism failure.** `libdbus-sys`'s `build.rs` calls `panic!` when pkg-config can't find dbus, so `wcore-eval-scenarios --test sbom_contract` never compiled. `ubuntu-latest` lacks the headers; every other Linux job runs in a container that has them | `4c08ac1e` — apt step |
| `CI (linux-containerized)` | **UNKNOWN.** Fails at 1m04s. GitHub refuses the log while the parent run is in progress | — |

### Reading the last one
`CI (linux-containerized)` is job `91642878748` in run `30800234966`. That run
also contains the three still-pending matrix jobs, which is why the log is
locked. The moment the run completes:

```
rtk proxy gh run view --job 91642878748 -R FerroxLabs/wayland-core --log-failed
```

**Use `rtk proxy`.** The bare `gh run view --job` is rewritten by the rtk hook
into something that demands a run ID and fails with `rtk: Run ID required`.
Cost me two attempts.

A 1m04s failure in a job whose first act is a docker image build is far more
likely to be setup than tests — the same shape as the SBOM one. Do not assume
it is a product defect until the log says so.

---

## 3. WHAT IS PENDING IS THE THING WE HAVE NEVER HAD

The three outstanding checks are `CI (macos-latest)`, `CI (windows-latest,
hosted)` and `CI (Array)` — the full three-platform matrix. **It has never once
completed on this branch.** Every prior attempt was cancelled by a subsequent
push (the #158 pattern; 91 of 100 integration runs).

**Therefore: do not push to `plan/f20-unified-audit-repair` until it reports.**
I cancelled it three times today before working this out. That is why the fixes
are parked on `lane/osv-bumps` instead of pushed to the PR branch — a `lane/**`
push is platform-opt-in and does not disturb the PR run.

A red X in `gh run list` is often `cancelled`, not `failed`. Check the
conclusion before drawing any conclusion:
```
gh api repos/FerroxLabs/wayland-core/actions/runs/<id> > /tmp/r.json
python3 -c "import json;d=json.load(open('/tmp/r.json'));print(d['status'],d['conclusion'])"
```

---

## 4. THE SEQUENCE FROM HERE

1. **Wait for #257's matrix to report.** Do not push to the PR branch.
2. **Read the `linux-containerized` log** (§2) and fix whatever it names.
3. **Merge `lane/osv-bumps` into integration.** Tree-identical gate already
   green; the merge will move the corpus digests only if it touches
   `SOURCE_INPUTS` — it does not (lockfile + workflow YAML only), so **no
   corpus re-stamp is needed for this one.**
4. **RE-DERIVE the per-platform test table in `.planning/KNOWN-ISSUES-rc.md`**
   from the completed matrix. It is still the unsound one — its numbers came
   from a `report` job silently discarding two of three platforms
   (`merge-multiple: true`, proven on run 30699019736: three artifacts
   uploaded, `junit report count : 1`). **These are the numbers a user reads
   before deciding to deploy.** This is the last substantive item.
5. **Sean merges #257 to `main`.** 3,143 commits; he chose merge-to-main over
   tagging the plan branch.
6. **Sean creates the tag, then dispatches the workflow.** Note the order —
   `release.yml` checks out `refs/tags/${{ inputs.tag_name }}`, so it publishes
   FROM a tag that must already exist. It does not create one.
   ```
   gh workflow run release.yml -f tag_name=v0.12.26-rc.1
   ```

---

## 5. RULES THIS SESSION PAID FOR

1. **Do not push to a branch whose checks you are waiting on.** Three cancelled
   runs. This is the single biggest time sink of the cycle.
2. **`rtk proxy gh …`** for `gh run view --job` (§2).
3. **The contract corpus hashes `wcore-agent/src/{bootstrap,engine}.rs` by
   PATH** (`SOURCE_INPUTS`, `crates/wcore-protocol/src/contract/spec.rs:983`).
   The crate graph says `wcore-protocol` depends only on `wcore-types` and that
   reasoning is *wrong* here — it cost a bisect. Editing those two files moves
   the digests. Regenerate with `cargo run --bin wcore-contract -- generate`,
   then **key-diff**: expect zero keys added, zero removed, only
   `fixture_digest` + `source_inputs_digest`, `schema_digest` unchanged.
   `docs/` and `.planning/` are NOT inputs, so doc edits are free.
4. **Branch ancestry is the wrong instrument for "did this land".** I concluded
   OAuth storage was plaintext because `lane/oauth-finish` is not an ancestor of
   integration. True, and the wrong conclusion — that branch was the *rejected*
   repair; a later one landed. **Read the merged source.** This one nearly cost
   us the release: it was the strongest argument against shipping and it was
   false.
5. **A mutation run is not optional on a new gate.** Dropping
   `&& memory.enabled` from `Config::skills_lifecycle_enabled()` left the whole
   suite green — the accessor had no coverage at all.
6. **hetzner cannot push to GitHub** (no credentials). Generate there, `scp` the
   files back, commit and push from the Mac.
7. Still true: never run cargo on the Mac (`cargo fmt` only); verify every `-q`
   push resolves.

---

## 6. STATE OF THE BAR

**Nothing lies to the user, nothing loses their data — both closed.**

- Data loss: credential splice, keyless-host journal amnesia, Windows log
  rotation past 5 MiB, Grep's silent no-match.
- Lies: the last one was `[memory] enabled = false` not stopping recording,
  merged at `4160bf76`. Enforced at three points plus a chokepoint on
  `set_memory_api`; red baseline proven on the parent, four mutants killed, and
  a four-way cross audit (Codex 5.6 Sol / Gemini 3.1 Pro / Kimi K3 / internal)
  returned SOUND-WITH-FIXES unanimously — all three external legs found the
  host bypass, which is now closed.

**Corrected this session:** `KNOWN-ISSUES-rc.md` falsely claimed OAuth tokens
are stored in plaintext. They are not — `OAuthStorage::store_serialized` runs
write-secure → verify-readback → remove-cleartext and fails closed with
`NoSecureBackend`. Fixed at `c615daed`.

**Open and disclosed, not blocking:** cross-process OAuth refresh collision
(#172, plan is `.planning/PLAN-172-refresh-single-flight.md` v3, work rescued
on `lane/refresh-single-flight` at `93432b1f` — INCOMPLETE, DO NOT MERGE);
certification passes and is ~2,100 commits behind; one-way migration off the
legacy token file.

**Unmeasured, and the honest reason this is a candidate:** the credential
ladder has never met a real OS keyring on any platform, and seven of ten
channel adapters have never been driven against the real platform. The code is
plausibly full-release quality; the evidence is rc.1.

**The 8 advisory suppressions in `deny.toml` expire 2026-09-02** — about a
month out. That is task #131 and it is now dated, not hypothetical.
