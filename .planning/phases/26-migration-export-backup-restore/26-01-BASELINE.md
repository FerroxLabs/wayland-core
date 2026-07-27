# 26-01 BASELINE — measured pre-change behavior, peer format grounding, corpus provenance

Host: planning Mac (arm64). Binary under test for the PRE-CHANGE measurement only:
`/Users/seandonahoe/dev/waylandcore-ferrox/target/debug/wayland-core`, which self-reports
`wayland-core 0.12.25 (source a8ed732216bb650a7980f4dcc06e52daef3cc793)` — a commit that is NOT
HEAD, so this binary is admissible for the pre-change baseline and for nothing else.

Lane branch: `lane/26`, worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-26`.

---

## 0. macOS binary producer confirmation (run FIRST, before any measurement)

Task 4 needs a fresh arm64 macOS binary built from this plan's SHA. Its only admissible producer
is CI's `build` job. Each claim below was checked against the file and against a real run.

| Check | Command | Literal result |
|---|---|---|
| build matrix includes the target | `/usr/bin/grep -n 'target: aarch64-apple-darwin' .github/workflows/ci.yml` | line 387 — present |
| artifact name is target-derived | `/usr/bin/grep -n 'name: wayland-core-\$\{\{ matrix.target \}\}' .github/workflows/ci.yml` | line 453 — present |
| upload fails loudly on a miss | `/usr/bin/grep -n 'if-no-files-found: error' .github/workflows/ci.yml` | line 457 — present |
| build job has no branch gating | `awk 'NR>=370&&NR<=460 && /if:/' .github/workflows/ci.yml` | only `runner.os == 'Linux'` / `matrix.use_cross` conditions — no branch or PR gate |
| producer WORKS on a real run | `gh run view 30211891454 -R FerroxLabs/wayland-core --json jobs` | `Build (aarch64-apple-darwin): success` **while the run overall concluded `failure`** |
| that run published a live artifact | `gh api repos/FerroxLabs/wayland-core/actions/runs/30211891454/artifacts` | `wayland-core-aarch64-apple-darwin 31772150` bytes, `expired=false` |

F26-SC1-CI-ARTIFACT: PRESENT — artifact `wayland-core-aarch64-apple-darwin`, produced by the
`build:` job whose matrix includes `target: aarch64-apple-darwin` and whose upload step sets
`if-no-files-found: error`; trigger is `push:` on the branch list at `.github/workflows/ci.yml`
lines 6-8; most recent observed `Build (aarch64-apple-darwin)` job: run 30211891454, conclusion
`success`, publishing a live 31,772,150-byte artifact even though that run concluded `failure`
overall. The producer is confirmed intact and independently retrievable.

### DEVIATION — the push trigger did not name this lane's branch

MEASURED: `ci.yml`'s `push:` branch list contains exactly `main` and `plan/f20-unified-audit-repair`.
This lane executes on `lane/26` (mandated by the lane brief's worktree rule), which is NOT in that
list, so pushing it fires no workflow and produces no artifact. `workflow_dispatch` is unavailable:
`ci.yml` on the DEFAULT branch (`main`) declares only `pull_request: [main]` and `push: [main]`, and
GitHub exposes that trigger only for workflows already present on the default branch. Opening a PR
and merging to main are reserved actions.

Put to the four-member panel (`amend-ci` vs `escalate`). Verdicts: codex `amend-ci`, gemini
`amend-ci`, kimi `amend-ci`, internal adversarial pass `amend-ci` after resolving its own objection
(that `ci.yml` is not in this plan's `files_modified`) as a Rule-3 blocking-issue fix: the plan's own
gate REQUIRES the working branch to appear in that list, and it was the brief's lane-worktree mandate
that made the plan's assumption false. Kimi's caveat — that the upload step might be branch- or
PR-gated — was checked and cleared (no such condition exists).

CHOSEN: `amend-ci`, BASIS: majority (4/4, unanimous). One additive line, appended LAST in the branch
list to minimise the chance two lanes pick the same insertion point. **The orchestrator may drop this
one-line commit at serial merge; it is proof scaffolding, not product.**

---

## 1. Pre-change behavior of the shipped binary — REAL `~/.hermes`

Command (run without `--include-credentials`; that flag was never passed against a real home):

```
./target/debug/wayland-core migrate hermes --home "$HOME/.hermes" --dry-run
```

Exit status: **0**. Captured stdout, verbatim (57 non-empty lines):

```
Migration plan: hermes → wayland-core
Source: /Users/seandonahoe/.hermes

Profiles (12):
  • flux-backend-eng
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-ceo
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-cro
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-cron-sentinel
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-eng-lead
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-intake
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-ops-lead
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-pricing
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-qa
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-rq-lead
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • flux-security
      provider=deepseek model=deepseek-v4-pro
      base_url=https://api.deepseek.com/v1
      credential: DEEPSEEK_API_KEY found — NOT imported (pass --include-credentials)
  • fred
      provider=anthropic model=claude-opus-4.6
      base_url=https://openrouter.ai/api/v1
      mcp: ijfw-memory
      credential: OPENROUTER_API_KEY found — NOT imported (pass --include-credentials)

MCP servers to add (1): ijfw-memory

Detected but NOT imported in this pass (tracked for a follow-up):
  • 540 skill directories
  • 12 SOUL.md persona files

Dry run — no changes written.
```

### Observed counts — the baseline this plan must PRESERVE

| Quantity | Measured |
|---|---|
| Profiles | **12** |
| MCP servers to add | **1** (`ijfw-memory`) |
| Skill directories (detected, not imported) | **540** |
| `SOUL.md` persona files (detected, not imported) | **12** |
| Memory notes | **0** |

### Determinism — MEASURED, not assumed

The identical invocation was run a second time and the two captures compared byte for byte:
`/usr/bin/diff -q /tmp/26-01-prechange.txt /tmp/26-01-prechange-2.txt` → **no difference**.
Determinism is therefore a pre-existing property this plan must preserve, never one it may claim
to have introduced.

### Non-mutation — MEASURED, not assumed

A content digest over every file in the real Hermes home was taken before and after a full dry-run:

```
/usr/bin/find "$HOME/.hermes" -type f -print0 | /usr/bin/sort -z | xargs -0 shasum -a 256 | shasum -a 256
```

before: `57c365cefedc3910819203fedf132a93fc1f5e0001cd56250f6d991c423531cb`
after:  `57c365cefedc3910819203fedf132a93fc1f5e0001cd56250f6d991c423531cb`

Identical — the dry-run does not mutate the source it previews.

---

## 2. The four gaps — established by interrogating the BINARY, not the source

| # | Gap | How established | Literal result |
|---|---|---|---|
| 1 | No OpenClaw path | `wayland-core migrate --help` | Lists exactly two commands: `hermes` and `help`. There is no `openclaw`. |
| 2 | Dry-run output is untyped prose | `wayland-core migrate hermes --help` | Flags are exactly `--home`, `--dry-run`, `--yes`, `--include-credentials`, `--overwrite`. There is no `--json`. |
| 3 | No backup/restore/export | `wayland-core --help` filtered for those words | No match — none of the three commands exists. |
| 4 | Root-profile discovery gap | read `hermes::detect_home` + check the real install | `detect_home` bails unless `<home>/profiles` `is_dir()` (hermes.rs:33-39). The real install HAS a root `config.yaml` (8.9K) and root `.env` (306B) that the guard causes to be ignored entirely. |

Gap 4 confirmed load-bearing: the real root `config.yaml` carries a `model:` block with exactly the
keys the existing mapper consumes (`default`, `provider`, `base_url`, `api_mode`) plus an
`mcp_servers:` map containing `ijfw-memory` — i.e. the root setup is importable by the SAME mapper
and is being dropped only because of the guard. The root `.env` carries `OPENROUTER_API_KEY` and
`DEEPSEEK_API_KEY` among its six keys.

---

## 3. Peer source-format grounding — three readings reconciled

### OpenClaw

| Reading | Ref | Version |
|---|---|---|
| pinned baseline (ledger) | `11a0ad10` | 2026.6.2 |
| checkout HEAD | `3659c85e534fdb8b8ce6b7505a83d92cc2e4df8e` | 2026.7.2 |
| real install | `~/.openclaw` | — |

Home resolution DETERMINED from peer source, not assumed — `src/config/paths.ts`:

```
const LEGACY_STATE_DIRNAMES = [".clawdbot"] as const;   // line 23
const NEW_STATE_DIRNAME = ".openclaw";                  // line 24
const CONFIG_FILENAME = "openclaw.json";                // line 25
const LEGACY_CONFIG_FILENAMES = ["clawdbot.json"] as const; // line 26
```

Resolution order (`resolveStateDir` / `resolveConfigPath`, lines ~62-229): `OPENCLAW_CONFIG_PATH`
overrides everything; else `OPENCLAW_STATE_DIR`; else `OPENCLAW_HOME`/`os.homedir()` joined with
`.openclaw`; the config file is `openclaw.json`, falling back to `clawdbot.json`. **There is no
platform-specific branch** — the same `os.homedir()`-relative resolution is used on every platform.

**Compatibility range: NO DRIFT.** All four constants are byte-identical at `11a0ad10` and at HEAD.
Every field the importer depends on exists in both, so the importer targets a single format and
nothing needs to be treated as optional on version grounds.

Real install top-level keys observed: `agents`, `auth`, `channels`, `commands`, `discovery`,
`gateway`, `mcp`, `messages`, `meta`, `models`, `plugins`, `session`, `tools`, `wizard`. Secret-bearing
sites: `channels.telegram.botToken`, `gateway.auth.token`, `gateway.remote.token`, `auth.profiles.*`.
Backup/last-known-good siblings present and MUST NOT be counted as extra sources: `openclaw.json.bak`,
`.bak.1`, `.bak.2`, `.bak.3`, `.bak.4`, `.bak-20260511-190206`, `.bak.20260725-114041`, `.last-good`
— eight of them beside the primary, which would multiply every discovered item nine-fold if treated
as sources.

### Hermes

| Reading | Ref | Version |
|---|---|---|
| pinned baseline (ledger) | `dbe734be` (verified present) | 0.17.0 |
| checkout HEAD | `d59b79fadd1e9edd7afc5c679cc3b143838e7c01` | 0.18.2 |
| real install | `~/.hermes` | — |

Format consumed: `profiles/<name>/config.yaml` (`model.{default,provider,base_url}` + `mcp_servers`),
`profiles/<name>/.env` (dotenv, `<PROVIDER>_API_KEY`), `profiles/<name>/{skills/,SOUL.md,memories/}`
for the deferred inventory, plus the previously-ignored ROOT `config.yaml` + `.env`.

**Compatibility range:** the peer carries no machine-readable config schema at either ref, so the
format was reconciled against the real install, which is authoritative for what users actually run.
No parse-changing disagreement was found between the two refs for the fields the importer reads.
Ledger claims about peer capability were verified present at HEAD: `hermes_cli/migrate.py`,
`hermes_cli/backup.py`, `agent/curator_backup.py` all exist — Core is indeed the only party without
a reciprocal path.

---

## 4. Corpus generator and its provenance

`scripts/portability-corpus-gen.py` clones a real peer tree's SHAPE while substituting a distinct,
deterministic canary token for every value it classifies as secret.

**Classification rule (explicit, and the corpus manifest records it):**
- R1 — dotenv key matching `*_(API_KEY|TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL)`.
- R2 — JSON/YAML mapping key containing `key`/`token`/`secret`/`auth`/`pass`/`cred` with a string
  value of at least 8 characters, minus a small allowlist of structural look-alikes
  (`max_tokens`, `keywords`, `authorized`, …) that would otherwise destroy the corpus's shape.
- R3 — every string value under `credentials/` or in an `auth.json`.

**Bounding rule (a deliberate, recorded deviation from a byte-for-byte clone):** the real Hermes home
is **2.0 GB**. Cloning it verbatim is not committable and adds no discriminating power. The generator
clones the importer-relevant files verbatim and reproduces everything else as DIRECTORY MARKERS —
each real subdirectory becomes an empty directory holding a `.keep` — so a counter still sees the
real shape. Measured: the corpus reproduces 12 profiles × 45 skill directories = **540**, matching
the real install exactly. `MANIFEST.json` records `bounded: true` with the rules, so the corpus
cannot be mistaken for a full clone.

Committed corpus: **600 files, 328 KB** across `hermes/` (581 files, 36 canaries) and `openclaw/`
(19 files, 28 canaries).

**Determinism — MEASURED.** Two runs over each real source, compared with `diff -r`: byte-identical
for both peers.

**Zero real secrets — MEASURED, and the search was proven non-vacuous first.** Real values were
extracted from both real homes into a scratch file outside the repository: **7 values** found
(matching the count measured at plan time, so the search had something real to look for). Each was
searched for across the committed corpus and the generator source with `/usr/bin/grep -RqF`:
**searched=7 hits=0**. The scratch file was deleted. No real secret value was written into the
corpus, into this document, or into any committed file.

**Generator robustness fix (deviation, Rule 1).** The first run crashed with `PermissionError` on
`~/.openclaw/plugins` (mode 0700, unreadable). A peer tree is hostile input by construction; an
unreadable directory now contributes no shape and is recorded in the manifest rather than aborting
the walk.

---

## 5. Environment drift recorded

- `hetzner-dsm` toolchain is `cargo 1.96.0`, not the `1.95.0` the plan recorded. Non-blocking;
  noted so a later reader does not treat it as tampering. Disk free on `/root`: **741 GB**.
