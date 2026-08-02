# What changed between v0.12.25 and today

**Baseline** `v0.12.25` = `61b79c4f`, 2026-07-13.
**HEAD** at time of writing = `02575b6f` on `plan/f20-unified-audit-repair`, 2026-08-01.
**Window** 19 active days, 3,012 commits, no release cut.

Produced by a five-agent research sweep over git history, `.planning/`, and the
live product surface. Every number below came from a command that ran. Where a
claim is a document's rather than a measurement's, it says so.

---

## 1. The one-paragraph answer

This was not a features release. Between 0.12.25 and today the agent gained
**zero new tools, zero new providers, zero new channel platforms, and zero new
TUI screens**. What arrived is a new **operations layer** around the same agent
— ten new top-level CLI commands — and, more than anything else, a large body of
**honesty infrastructure**: the ability to tell which of our claims are true.
The capability surface roughly doubled. The proof surface did not keep up, and
the project's own grading says so in writing.

---

## 2. Scale, and the caveat that must travel with it

| Measure | Value |
|---|---|
| Commits | 3,012 (2,732 non-merge, 280 merge) |
| Active days | 19 (2026-07-13 → 2026-08-01, one gap) |
| Lines | +1,079,387 / −59,727 |
| Unique files touched | 5,563 |
| Repo file count | 1,832 → 6,736 (3.7x) |

**72% of the window landed in four days** (27–30 July). 29 July alone was 1,097
commits.

**The caveat: 58% of that churn is `.planning/` prose**, not source.
`.planning/` went from 0 files to 3,441. Source under `crates/` is 42%. Any
"over a million lines" headline without this qualifier is misleading.

### What kind of work it was

| Type | Count | Share |
|---|---:|---:|
| docs | 704 | 23.4% |
| **fix** | **567** | **18.8%** |
| test | 477 | 15.8% |
| feat | 248 | 8.2% |
| style | 47 | 1.6% |
| chore | 39 | 1.3% |
| ci | 29 | 1.0% |
| refactor | 11 | 0.4% |
| perf | 1 | 0.03% |

**fix:feat = 2.29:1. Tests are 2x features.** `perf: 1` and `refactor: 11` mean
essentially no time went to optimization or cleanup. This was a repair-and-prove
window.

### Structure

- **Two new crates:** `wcore-exec-backend`, `wcore-gateway`.
- **Zero crates removed. Zero files deleted anywhere under `crates/`.** 1,305 new
  files. Add:delete = 18:1 — near-pure accretion.
- Top churn: `wcore-agent` (136k lines), `wcore-cli` (86k), `wcore-eval-scenarios`
  (50k), `wcore-sandbox` (29k). `wcore-agent` + `wcore-cli` = 49% of crate churn.
- **Trap:** `wcore-swarm`, `wcore-budget`, `wcore-acp`, `wcore-cron`, `wcore-egress`
  and others rank high in churn but **already existed at 0.12.25**. The AGENTS.md
  crate map is stale and omits most of them.
- Five new CI workflows: `lint`, `macos-docker-gate`, `macos-native-suites`,
  `release-rehearsal`, `supply-chain`.
- **`CHANGELOG.md` moved 7 lines in 3,012 commits.** It cannot describe this
  window; release notes need writing from scratch.

---

## 3. What shipped — the product surface

Evidence tags: **[live]** = driven against the real product with a receipt;
**[impl]** = implemented, not live-proven.

### Ten new top-level commands

| Command | Verbs | Evidence |
|---|---|---|
| `index` | build/status/search/verify | **[live] all 3 platforms** — strongest in the window; nonce-bound, SHA-asserted binaries |
| `backup` | create/verify/restore/recover/digest | **[live]** incl. a real Windows long-path fix |
| `goal` | 7 verbs, durable Goals + Fleet | **[live]** survives `kill -9` mid-wave on Linux + Windows; effects 12/12/12 |
| `backend` | 9 verbs; local/container/ssh/cloud + receipts | **[live] 3 of 4** — cloud leg unexercised |
| `sandbox` | status/exec | **[live]** 50 greens, each with a differential activeness observation |
| `session` | 11 verbs incl. checkpoint/rewind/fork/export | **[live] Linux only** — macOS, Windows, TUI legs open |
| `gateway` | 12 verbs, launchd/systemd/task service | **[live] Linux only** — systemd-observed recovery |
| `node` | 9 verbs, pairing/attribution | **[live] single host** — no second machine exists to test with |
| `channel` | list/probe/health/reload/actions/credential | **[impl]** |
| `cache` | report/list/show/verify | **[impl]** — Phase 23 C4 graded NOT MET |

### New verbs on existing commands

- `plugin` +10 (new/test/verify/sign/publish/inspect/approve/update/rollback/recover)
  — **[live] Linux + Windows**, the only MET criterion in Phase 25.
- `migrate` +openclaw/grok/gemini/promote — **[live]** against real Hermes and
  OpenClaw installs.
- `cron` +publish/leases/retry — **[impl]**; webhook and poll triggers do not exist.

### Safety-relevant CLI changes

- Two-tier danger model: `--dangerously-skip-permissions` (tier 1 — bypasses
  approvals, **OS sandbox stays ON**; `--force`/`--yolo` are now aliases *of it*,
  one clap field with `visible_aliases`, so the three spellings are the same tier
  by construction) vs `--dangerously-skip-permissions-and-sandbox` (tier 2 —
  argv-only provenance, TTL-leased).
- `--trust-workspace` / `--untrust-workspace`: project hooks, MCP and skills are
  now **inert by default**.
- `--skills-govern/revoke/rollback`, `--allow-host-{workspace,budget}-grants`.
- Nothing removed.

### Config, protocol, TUI

- New `[execution]` global-only admin floor; `[provider_policy]` (allow/deny
  providers and regions, `require_priced`); provider `region`/`organization`;
  `[session] require_durability`; per-provider `compat.image_model`.
- Protocol: **13 new host→Core commands, 24 new events**, plus a machine-checked
  producer contract (`wcore-contract`). The contract gate was itself found
  self-passing and repaired mid-window.
- TUI: only `/goal` and `/recover` added.

### Channels — the honest line

**3 of 10 driven against the real platform.**

Slack and Discord both claimed **exactly-once on a mock** and produced **two
messages** under live kill-and-restart. Both corrected to at-most-once. Matrix is
the only surviving exactly-once adapter, and only below the length cap.

**Implemented but not measured:** Telegram, WhatsApp, SMS, email, Signal,
iMessage, MS Teams. `channel probe` exists only for Discord, email, WhatsApp.

---

## 4. Security — what actually closed

289 non-merge commits name a security concept; 185 touch core security paths;
`wcore-sandbox` alone took 105 commits.

**7 RUSTSEC advisories addressed. 0 CVEs. 0 published advisories.**

### The ten most consequential

1. **Untrusted cloned repo could switch off the egress boundary** (HIGH).
   `security.enabled` merged `global && project`, so *either* layer could turn the
   gate off — the least restrictive merge, under a comment claiming the most
   restrictive. Forwarded by `restrict_untrusted_project_config`, the very
   function meant to neutralize untrusted config. The existing test had **pinned
   the vulnerability as a property to preserve**. RED 2/7 → GREEN 7/7 plus 574
   regression tests, proven with a real POST to an attacker-shaped host. **[live]**
2. **Root RCE via the ssh exec backend.** `--task-id 'x;id>/tmp/w'` executed as
   root on the far end: `ssh` carries no argv, so the far end's login shell
   re-parses the string and near-side argv safety buys nothing. The guard passed
   the whole time **because it grepped its own source**. Fixed `6861b3aa`.
   **Determined never shipped** — `git tag --contains` empty, with the instrument
   sanity-checked first (36 tags, root commit in all 36). Would have shipped in
   the RC had the lane not driven a second real host.
3. **`PolicyGate` had zero production callers.** ACL machinery shipped in v0.6.1
   was orphan code on the agent path; a session narrowed away from `Read` produced
   children that could read files the parent could not. Ablation-proven.
4. **Spawn `toolsets` treated as authority.** A bare `Delegate` restored Grep and
   Glob to a child of a `Full`-posture remote parent that had dropped them
   *precisely because their recursive scan escapes the jail*. **[live]**
5. **Plaintext credential fallback → fail-closed ladder** (keyring → encrypted
   vault → refuse), plus three shadowing sinks (`auth add` wrote an api_key into
   `config.toml` that *outranks* the store; the TUI modal wrote a `.env` the
   resolver never reads). Test-proven; **2 of 7 sinks still deferred**.
6. **Windows AppContainer `fs_read_deny` was completely inert** — the lowbox check
   ignores a DENY ace against the container's own package SID; secret disclosed,
   exit 0. Reimplemented as enforced DACL removal. **[live] on hardware**
7. **Cross-user prompt leak** (HIGH). One user's inferred traits reached another
   user's system prompt and shipped to a third-party provider. **Proven on the wire**,
   extracted from a captured provider body.
8. **`backend` never armed the egress policy** — returned ~429 lines before the
   chokepoint, so cloud backends ran allow-all. The receipt had been disclosing it
   all along: `"egress_decision": "allow-all-default-no-policy-installed"`. **[live]**
9. **Every release shipped with no manifest**, so the updater refused every
   install; manifest digests were decorative until the archive was bound to them.
10. **A stale AppContainer lease permanently refused ALL sandboxed execution**
    until a human deleted a file nobody knew to look for. The operator text had
    blamed "transient (AV, disk contention)" — which is why the wedge went
    unrecognised for weeks.

### Supply chain

`cargo deny` exit 5 → 0; `cargo audit` 4 → 0 **with no ignore list**. Four
advisories closed at source (one by a `cargo update` that moved 2 packages, after
the pin's comment claiming a repo-wide bump was measured false); two accepted with
dated, fully parent-traced exceptions. `[graph] all-features` turned on — the old
setting never evaluated optional dependencies at all, so a GPL/AGPL crate arriving
through `hf-hub` or `bollard` would have passed every gate. Deterministic SBOM
(no clock, no random, no env, no filesystem). Signed release manifest with
role-scoped trust root and load-bearing domain separation.

### Three things not to claim

- **No CVE was patched.** Zero issued.
- **The egress boundary still has no exfil interlock.** `--i-accept-exfil-risk`
  never existed — the product returned `error: unexpected argument` while
  advertising it in four places including the user-facing deny message. The false
  claims were removed; the interlock was deliberately **not** added.
- **Supply chain did not succeed.** Phase 29: all four criteria PARTIAL, goal not
  achieved. Signing is proved with throwaway keys. **No release was ever accepted
  through the chain.**

---

## 5. Where we are — the programme's own verdicts

| Phase | Verdict (its own words) |
|---|---|
| 20 Delegated mutation | COMPLETE — by **scope re-reading** over an unrepaired Windows hardware FAIL |
| 20A Native UAT | COMPLETE — same-day live pass on that exact sealed binary found **3 HIGH** |
| 21 Child authority | **NOT ACHIEVED** (graded 4x) |
| 22 Supervision/Goals/Fleet | **NOT ACHIEVED** — "surfaces observe, cannot control" |
| 23A Governed skills | **SC1 NOT MET** — "it ships to nobody" |
| 23B Continuous agency | **NOT ACHIEVED** — 1 met / 2 partial / 3 not met |
| 24 Gateway/Channels/API | **NOT ACHIEVED** — "'support' is the word that fails" |
| 25 Remote reach/Plugins | NOT FULLY ACHIEVED — 1 MET, 1 MET-w-exc, 2 PARTIAL |
| 26 Migration/Backup | PARTIALLY ACHIEVED |
| 27 Multimodal/Voice | NOT ACHIEVED (partly upgraded since) |
| 28 Native certification | Gate passes **only** against a superseding receipt, over a stale tree |
| 29 Supply chain | **NOT ACHIEVED** — all four PARTIAL |
| 30 Scorecard/Frontier | **NOT ACHIEVED** |
| 31 Vacuous greens | **ACHIEVED** |
| 32 glibc reach | **ACHIEVED** — floor 2.39 → 2.34, live on 5 distros |

**51 success criteria: 23 MET-family / 20 PARTIAL / 8 NOT MET.**
Caveat: 10 of the 23 belong to phases 20, 20A and 28 — the three carrying
contested evidence.

**`REQUIREMENTS.md` checkboxes: 12 of 58 ticked.**

### Stale records that will mislead anyone who reads them

- `STATE.md` and `HANDOFF.json` were abandoned ~2026-07-25 and still say
  *"Phase 20, executing, 0 phases complete."*
- `ROADMAP.md` says Phase 30 is "NOT STARTED" — its verdict landed 2026-07-29.
- `CRITERIA-STATUS.md` is stale **by its own stated rule** (81 commits behind).

---

## 6. The defect class that defines this window

**61 of 86 gate scripts had never been shown to fail.** The repo's own
anti-vacuity control was a no-op. `.config/nextest.toml` carried
`no-tests = "fail"` which **nextest silently ignores** — fail-closed behaviour
came entirely from a CLI default, the exact dependency the key existed to remove.

Recurring instances, including inside the tools built to catch it:

- A guard for shell injection that **grepped its own source** — made a real RCE
  look actively covered.
- A test that **asserted the egress vulnerability as a property to preserve**.
- A credential scanner whose "no cleartext found" was true of an empty directory.
- Phase 22's falsifier greps the wrong directory — *"reports FAILED forever,
  including after the criterion closes."*
- Phase 24: five self-passing gates, including *"a gate against self-passing gates
  was self-passing."*
- Phase 27: a test with `#![cfg(target_os = "linux")]` compiled to an empty
  harness on macOS and printed `ok. 0 passed` at exit 0.

The governing rule the project adopted from this:

> **A gate that cannot fail and a gate that cannot pass are the same bug wearing
> different colours.**

And the corollary, learned the expensive way: *a security property that crosses a
process, host or trust boundary must be proved by driving the boundary, never by
inspecting the source on one side of it.*

**Consequence for reading any claim in this document: "test added" is a materially
weaker claim in this codebase than "control fired."**

---

## 7. Release readiness

**Blocking:**

1. **The native certification is stale.** Phase 28 certified candidate `32e2f57d`;
   HEAD is **2,097 commits** past it. In the drift document's own words:
   *"It passes and it is stale. Both are true and neither cancels the other. A
   release cut today would carry a passing certification that never saw the
   shipped binary."*
2. **No three-platform green run exists yet at one SHA.** Being worked now.
3. `release-please.yml`'s release job is **never executable** — `on:` is
   dispatch-only while its condition reads `github.event.head_commit.message`.

**Not blocking, contrary to earlier belief:**

- Contract corpus is **fresh** — digest measured equal to the pinned manifest.
- `#661` (grep swallowing exit-2, and the two sub-agent silent-success defects) is
  **already fixed** at HEAD for its three dangerous findings; two lower-severity
  ones remain open.

**Mechanically possible:** 34 of 34 historical releases were `workflow_dispatch`;
both secrets are present.

---

## 8. Honest summary

The capability surface roughly doubled and the operations story went from nothing
to ten commands, several of them proven on real hardware on three platforms. That
is real, and `index`, `backup`, `goal` and `sandbox` are the strongest of it.

Against that: every graded phase failed its own goal; 3 of 10 channels have ever
been driven at the real platform; two adapters were caught claiming a delivery
guarantee they did not have; and the single largest discovery of the window was
that most of our gates could not fail.

The most valuable thing built in these 19 days is not a feature. It is that we can
now tell the difference between working and appearing to work — and the reason we
can tell is that we kept finding cases where we could not.
