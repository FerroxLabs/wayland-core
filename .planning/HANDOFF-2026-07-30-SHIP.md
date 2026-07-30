# HANDOFF — Wayland Core, SHIP milestone, 2026-07-30

Integration `plan/f20-unified-audit-repair` @ **`57a41c7d`**.
Supersedes `HANDOFF-2026-07-30-WAVE3.md`. Read `.planning/MILESTONE-SHIP.md` alongside this.

**`.planning/LANE-BRIEF.md` outranks any orchestrator instruction, including this file.**

---

## 0. Do this first

1. **A legal question is open and it is Sean's, not yours** — §4. Do not strip the nine
   attribution headers until it resolves.
2. **Lanes in flight** — §1. Measure their branches before assuming anything; a death notice is
   an absence claim.
3. **Slack is one `/invite` away** — §3.
4. Then §5, the ranked open work.

**Merge cadence, unchanged and load-bearing:** one lane at a time → merge locally → push to a
disposable `orch-verify-*` ref → on hetzner `git checkout -- .` then checkout that ref and
**assert the SHA** → `cargo check --workspace --all-targets` + `cargo fmt --check` → only then push
to integration → delete the scratch ref.

---

## 1. In flight when this was written

| lane | doing |
|---|---|
| `discord-live` | send/edit/delete/receive + **live** idempotency against a real Discord server |
| `matrix-live` | same five against a real Matrix room |
| `glibc-reach` | lower the Linux glibc floor (see §2) |
| `media-cost-complete` | cost records for TTS, `video_analyze`, 3 vision backends |
| `provenance-comparison` | the legal fact-finding in §4 |

---

## 2. The MVP cut Sean approved

**Ship:** engine, multi-provider routing, tools, sandbox + egress, delegated workspace/journal/
gated-merge, plugins, sessions, memory, repomap, gateway, and **Slack + Discord + Matrix**.

**Do not ship / do not advertise:** voice (correctly reported as not-compiled-in), browser
`Download` (unimplemented, unadvertised, fails closed), the other seven channel adapters, and **any
comparative claim about peers** — our own checker refuses all ten that were attempted.

**Still open before a tag:**

- **The glibc floor.** Measured `GLIBC_2.39` on the built binary; builder is Ubuntu 24.04, CI is
  `ubuntu-latest`. That excludes **Ubuntu 22.04 LTS (2.35), Debian 12 (2.36), RHEL 9 / Rocky 9 /
  Amazon Linux 2023 (2.34)** — most deployed server Linux. Not a code defect, a build-container
  choice. Lane running.
- **Contract regeneration #4 — orchestrator only, and the LAST action before any tag.** The corpus
  is RED at HEAD: `14 passed / 1 failed`, `missing=[] extra=[]`, 5 files drifted including
  `events/ready.json` and `manifest.json`. Benign digest drift, but **`events/ready.json` is what
  Desktop reads capabilities from, so Desktop UAT is blocked until this runs.** Generate on hetzner,
  `rsync` `crates/wcore-protocol/contracts/` back, commit and push from the Mac.
- **The Windows journey is a permanently-red gate** (§6).
- The UAT: TUI on three platforms, Desktop against the regenerated contract, and the three channels.

**Release build works:** `cargo build --release -p wcore-cli` → `RBRC=0`, 5m53s.

---

## 3. Credentials — all live, stored `0600` in `~/.wayland-secrets/`

| file | state |
|---|---|
| `discord.env` | **working.** Bot `WaylandCoreBot`, guild `Wayland Test Server`, channel `#general` `1532226655102173318`. Throwaway app + server |
| `matrix.env` | **working.** `@seandonahoe:matrix.org`, room `!kntRqkQCkPjhPvMMvf:matrix.org`, joined. **Sean's PERSONAL account — only that room, never join/leave/invite, redact test events** |
| `slack.env` | token + scopes correct (`chat:write, channels:history, channels:read`), workspace **Trade Canyon, Inc.**. **BLOCKED: bot is a member of 0 channels.** Needs `/invite @wayland_core_test` in a public channel |
| `flux.env` | pre-existing FluxRouter burn key |

**Never print, echo, commit or write any of these values.** Slack is a live company workspace —
bound any lane to one named channel id. Sean should rotate the Slack and Discord tokens after the
UAT; both were pasted in a chat transcript.

---

## 4. OPEN LEGAL QUESTION — Sean's call, do not pre-empt it

An external audit read our comments as implying we took peer code. Sean's position: reference points
only, everything is original Rust, strip the references.

`lane/peer-reference-scrub` removed **44** and correctly kept **390** (the migration subsystem must
name those tools — the product migrates users *off* them, and `migrate --help` listing both is a
feature). Diff proven **comment-only**: 169 changed `.rs` lines, **0 non-comment**. Migration suites
green: 7 / 34 / 14 passed, 0 ignored, 0 filtered.

**I restored nine of the removals** (`57a41c7d`) because they are attribution notices naming a
copyright holder, five of them at **line 1 of a whole module**:

```
wcore-providers/src/{failover,key_rotation,classify,cache_observation}.rs:1
wcore-pricing/src/refresh.rs:1        "ported from openclaw MIT (c) Peter Steinberger 2025"
wcore-providers/src/retry.rs:738      "Source: openclaw MIT (c) Peter Steinberger 2025"
wcore-providers/src/anthropic.rs:307  "ported from openclaw/hermes-agent"
wcore-channel-{imessage,msteams}/src/lib.rs  "Ported from the desktop app's TypeScript ... (OpenClaw MIT)"
```

**The reasoning, which must survive compaction:** copyright protects expression, not ideas — reading
a peer and writing your own implementation carries no obligation. But a language change is not a
clean-room defence, because translation is ordinarily still a derivative work. **The word in the
header is "ported", which is the word for translation.** The asymmetry decides the interim posture:
if the headers are wrong, leaving them one more day costs nothing; if they are right, deleting the
notice while keeping the code converts a compliant MIT use into a non-compliant one — and the
deletion sits in git history as evidence someone thought it was owed.

`lane/provenance-comparison` is establishing the facts: per-site, did only the idea carry over or
did structure/taxonomy/literals? It must produce a **control** (modules with no peer counterpart) or
its method is unfalsifiable. It reports into three buckets — independent / derived / unclear — and
is explicitly told not to resolve ambiguity in the flattering direction.

**If derived: keep attribution but move it to a single `THIRD-PARTY-NOTICES.md`.** MIT compliance is
nearly free — no copyleft, no commercial restriction, one file.

Two smaller ones, also Sean's: `docs/design/2026-07-13-*` is a **public** gap audit whose line 456
says peers *"should be copied where they are operationally better"* — the question is whether an
internal gap audit belongs in public `docs/` at all. And `docs/providers.md:380` names peer clients
as the evidence for a ToS disclosure, so deleting it weakens the disclosure.

---

## 5. What landed this session

Fifteen lanes merged, each workspace-verified on hetzner at a programmatically asserted SHA.

**Security and correctness:**
- **Egress master switch was project-negotiable (HIGH).** `security.enabled` merged `global && project`,
  so a cloned repo could disable the exfil boundary entirely — and
  `restrict_untrusted_project_config`, whose whole job is neutralising untrusted config, forwarded it
  deliberately. Now operator-owned. The obvious `||` fix was measured **defective**.
- **Untrusted config could RAISE `max_tokens`/`max_turns`** — clamped, trust-gated. The sweep also
  found the merge runs **with `project = default()` even when no project file exists**, so two fields
  override the operator's global with no project file at all (`tools.verify_edits`, `mcp.curation`).
- **`RUSTSEC-2026-0194` was reachable** through `wcore-tools` parsing user `.xlsx`. Now gated by
  `scripts/verify-suppression-traces.py`, which re-derives every suppression's parents from
  `Cargo.lock` and requires exact set equality. **Suppressions expire 2026-09-02.**
- **Lockfile drift broke every `--locked` build** including `release.yml` — `serial_test` missing.
- **A money bug**: transcription priced as `usd_per_image * images`, so a rate-card operator recorded
  **$0.00** for a real billed call.
- **CLI danger tiers** renamed; `--force`/`--yolo` are aliases on the same clap field, so they share a
  tier by construction. `--auto-approve` measured NOT tier 1 and left alone.

**Proof that was missing:**
- `27-C2(c)` three policy baselines, on the **real Camoufox backend** under Xvfb.
- `27-C5` aarch64 executed — 5 of 6 shipped targets now run; Windows-ARM impossibility *proven*.
- `REACH-*` C1/C2/C4 all **PASS** — Fly.io cloud leg, second physical host, orphans measured.
- Windows legs: single-owner lease, `F21-04-03` re-proof (20/20 fixed, 12/20 reverted), Phase 22
  M1/M4/M5, `21-C3` measured.
- **`docs/delivery-semantics.md`** — per-adapter table with a drift test that fails the build if doc
  and code disagree.

---

## 6. Corrections and decisions that must not be re-litigated

- **`F24-GWP-H1` is NOT a defect.** The journey submits `every:15`, rate-floored to **60s**
  (`trigger.rs:238`), so those are recurring jobs; every repeat carried a **different delivery id**,
  5 of 5 keyed jobs, zero replays. The heartbeat in the same run recurred too and nobody called that
  a duplicate. Windows only crosses the window because Task Scheduler's `PT1M` exceeds the 60s floor.
  **`F24-GWP-M1` was real and is fixed** — the receipt read the journal twice, four steps apart.
- **The real defect there:** only **8 of 24** arrivals carry an `idempotency_key`; `twilio.messages`
  and `whatsapp.messages` emit none, so a replay is indistinguishable from a recurrence *in
  principle*. Receipt now reports `{replays, recurrences, indeterminate, unidentified}`.
- **DECISION OWED: the Windows journey is a permanently-red gate.** Its deliveries recur every 60s and
  Windows recovery always exceeds that, so it can never pass. The honest gate fails on `replays > 0`
  or `indeterminate > 0` and passes on proven recurrences — but Rust `verify_counts` independently
  refuses any `duplicates != 0`, so a driver-only change makes the two gates disagree. **Needs a
  coordinated driver+verifier change.** Also `docs/delivery-semantics.md` §5 is wrong twice.
- **`24-C1` is 7 of 10, not 9.** Exactly-once is 3 of 10 and is **scoped to a delivery id**, not a
  message.
- **`release.yml`'s npm gating claim was false** for both aarch64 packages; corrected in place.

---

## 7. Standing lessons — all earned by a false green this session

- **Run every control in both directions.** Can it fail, *and* can it pass.
- **A skip is not a pass.** Count and report unrun cells.
- **`rtk` fabricates machine-readable counts and the absolute path does NOT save you.**
  `--numstat` gave `162 0` for a 40-line deletion; `grep -c` returned 0 for a present string;
  `wc -l < file` returned 0 for a 25-line file. **Redirect to a file, read with the Read tool.**
- **`${PIPESTATUS[0]}` fails in `sh`/dash** with `Bad substitution`, silently killing the script.
  Use `bash -c`. **`grep -c X || echo 0`** yields `"0\n0"`. Use `grep -q`. **zsh eats an unquoted
  `--include=*.rs`.**
- **Assert the SHA after every checkout.** A checkout that aborts on a dirty file leaves the OLD SHA.
- **Before testing for the absence of X, assert nothing you ran earlier could have created X.**
- **A participant that never started reports a clean run.**
- **Grade off code and executed tests, never a SUMMARY.**
- **The tracking documents are systematically stale in the product's favour.** Six of ten lanes this
  session returned "your premise is false" — including two whole plans I briefed as unbuilt that had
  landed two days earlier. **Re-measure before dispatching.**
- **A comment asserting a safety property the code does not implement** is this codebase's most
  common security defect class — three instances in `config.rs` alone.
