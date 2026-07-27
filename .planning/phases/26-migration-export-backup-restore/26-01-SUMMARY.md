---
phase: 26-migration-export-backup-restore
plan: "01"
status: complete
termination_state: 1 (Complete)
requirements: [F26-01]
lane_branch: lane/26
---

# Phase 26 Plan 01: Typed, Redacted, Deterministic Migrate Discovery — Summary

Made Hermes AND OpenClaw discovery typed, deterministic, structurally secret-redacted and
non-mutating, and proved each of the four with a gate that can go red — on Linux against
canary corpora and on macOS against Sean's REAL peer installs with live credentials.

**Termination state: 1 (Complete).** F26-01 is claimed, with two honest scope limits recorded
below rather than glossed.

## Verdict on the plan's own criteria

| Criterion | Outcome |
|---|---|
| Pre-change behavior MEASURED against both real installs | YES — 12 profiles, 1 MCP, 540 skills, 12 personas, exit 0 |
| Determinism + non-mutation measured, not asserted | YES — byte-identical reruns; identical tree digest |
| Both peer formats grounded in three reconciled readings | YES — pinned, HEAD, real install; OpenClaw drift = NONE |
| Discovery/dry-run TYPED | YES — `--json`, prose preserved |
| DETERMINISTIC | YES — byte-identical across independent walks, both corpora |
| SECRET-REDACTED structurally | YES — canaries on Linux AND real credentials on macOS |
| NON-MUTATING | YES — digest unchanged on both corpora and both real homes |
| Reciprocal OpenClaw path visible in the real binary | YES — `migrate --help` lists both |
| Root-profile gap closed | YES — root-only and rooted-plus-profiles |
| Every discovered item mapped or named | YES — nothing dropped unnamed |
| Panel harness proven able to go red | YES — self-test: 1 accept + 9 rejects |
| Redaction question DECIDED, not deferred | YES — 3 rounds, `contract-holds`, dissent recorded |

## What landed

- `crates/wcore-config/src/portability/` — typed plan vocabulary, the structural redaction
  boundary, and the tree digest. Reuses `profile::is_secret_entry` (lifted to `pub`) rather
  than growing a second definition of "secret".
- `crates/wcore-cli/src/migrate/openclaw.rs` — the reciprocal source, grounded in the peer's
  own `src/config/paths.ts`.
- `migrate --json` — the typed preview; never writes.
- Root-profile gap closed; root mapped by the SAME mapper as a profile.
- `scripts/portability-corpus-gen.py` + committed corpora (600 files, 328 KB).
- `scripts/portability-real-state-check.sh`, `scripts/panel-ask.sh`,
  `scripts/panel-decision-check.sh`.

## Gate results — real numbers

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked -p wcore-config -p wcore-cli --all-targets -- -D warnings`: **clean**.
- `cargo nextest run --locked -p wcore-config -p wcore-cli`: **2628 run, 2627 passed, 1 failed.**
  The single failure is `hermeticity_audit_test::no_dirs_config_dir_bypasses_outside_canonical_helper`,
  flagging `crates/wcore-gateway/src/service.rs:321`. **Pre-existing** — the line is present
  verbatim at my base `de977949` (introduced by phase-24 commit `8b582851`) in a file this work
  never touched. Out of scope per the scope boundary; logged to BACKLOG, not fixed.
- Multi-emitter probe: PASS with **1 test selected** (not 0, so non-vacuous).
- Panel checker `--self-test`: PASS — 1 accept, 9 rejects.

## Live evidence (the bar that matters)

**Linux, real binary, both corpora:** hermes exit 0, 14 items (12 profiles + 1 root + 1 mcp),
36 canaries declared / **0 in the document**, 13 credential refs present (positive half),
byte-identical across two runs, tree digest unchanged. openclaw exit 0, 3 items, 28 canaries /
**0 hits**, deterministic, unmutated, and `config_revisions_excluded: 8` naming the real
install's backup siblings as deliberately excluded.

**macOS, REAL homes, real credentials:** 7 real secret values extracted (non-vacuous),
**0 hits** in either emitted document, both homes unmutated, hermes profiles=12 — the
pre-change baseline preserved exactly.

**The real binary's own help now lists both sources**, closing the Core direction of the
ledger's `PORT-*` gap — proven by running the built binary, not by reading source.

## Deviations, each with its reason

1. **`ci.yml` amended (+1 branch line).** `push:` named only `main` and
   `plan/f20-unified-audit-repair`; this lane runs on `lane/26`, so the mandatory macOS proof
   leg had no producer. `workflow_dispatch` is unavailable (main's ci.yml has no such trigger).
   Put to the panel: **4/4 unanimous `amend-ci`**. **The orchestrator should drop this commit at
   serial merge — it is proof scaffolding, not product.**
2. **`.gitignore` negation for corpus `.env`.** `.gitignore:9` (`.env`) silently excluded every
   corpus dotenv — zero tracked. Locally green, remotely red. Renaming would destroy the shape
   fidelity the corpus exists for, so the negation is scoped to the fixture path.
3. **Cherry-picked `9a86b287`** (the coordinator's `Cargo.lock` fix). **This phase adds NO new
   dependency; `Cargo.lock` is otherwise untouched by me.**
4. **`git fetch --all` does not fetch lane branches on hetzner** (restricted refspec). Every
   remote gate uses an explicit `git fetch origin lane/26`.
5. **Corpus generator is BOUNDED.** A real Hermes home is 2.0 GB. Importer-relevant files are
   cloned verbatim; everything else becomes directory markers preserving counts (540 skill dirs
   reproduced exactly). Recorded as `bounded: true` in each manifest.
6. **Existing-test touch-ups, no assertion weakened:** `migrate_hermes.rs` gained `json: false`
   (compile necessity) and its error-text assertion now matches the new, more accurate message
   (the guard legitimately widened). Still asserts a hard error.
7. **TDD RED not observed per-test.** The Mac cannot compile and each Linux round-trip is a
   multi-minute remote build. Tests and implementation went together; rigor was preserved
   instead by every assertion carrying a POSITIVE half, and in practice the suite went red four
   separate times on real defects (below) before it went green.

## Defects the gates actually caught (evidence they can fail)

1. Generator crashed with `PermissionError` on an unreadable real directory — fixed.
2. `E0382` partial move in `openclaw.rs` — fixed.
3. Corpus `.env` files silently gitignored → credentials undiscoverable on Linux — fixed.
4. `build_plan` still hard-required `profiles/`, so root-only homes errored — fixed.
5. `PeerSource` serialized as `open_claw` — explicit rename.
6. My own ordering assertion in `redact.rs` was wrong — the test was fixed, not the code.

## The redaction decision — three rounds, closed by code not by counting

`CHOSEN: contract-holds`, `BASIS: majority` (3-1). Each round a member named a concrete path
and it was CLOSED and re-measured, per the plan's own rule:

| Round | Named path | Fix |
|---|---|---|
| 1 | `DiscoveredItem.details` untyped map (MCP `url` with `?token=`, `command` with `--api-key`) | `scrub_detail` + probe extended |
| 2 | `details` still `pub` — a sanitizer, not an invariant | made private; only writer scrubs; deserialization scrubs |
| 3 | `CredentialRef::name` a public deserializable `String` | narrowed to identifier shape on all paths |

**Dissent (recorded, not disposed of):** codex held `contract-cosmetic`. Its round-3 basis —
`name`/`source_file` as public deserializable Strings — was acted on by `f63da68a` AFTER the
capture. A fourth round was attempted; codex timed out twice on the larger bundle, so its
**pre-fix verdict stands as the honest record rather than being replaced by an assumed one.**

## Still open — carried forward honestly

- **HIGH-adjacent residual, named by the panel:** `McpServerConfig::headers` is not currently
  emitted, but would reopen a value channel if a future edit began emitting it. Not covered by
  the probe. **26-02 should close or explicitly fence this.**
- `source_file` remains a free `String` (produced only by the walk's own `relative_to()`).
- The macOS real-secret leg ran at `b671f9ad`; the three hardening commits after it are
  Linux-proven only.
- Pre-existing `wcore-gateway` hermeticity failure → BACKLOG (belongs to lane 24's area).
