# Exact position, and the plan to a release candidate

**As of** 2026-08-01, integration `plan/f20-unified-audit-repair` @ `02575b6f`.
**Baseline** `v0.12.25` = `61b79c4f`, 2026-07-13. 19 active days, 3,012 commits.

Written after a five-agent sweep of git history, `.planning/`, and the code.
Every claim here is code- or command-verified, or labelled as a document's claim.

---

## THE ANSWER IN ONE PARAGRAPH

We are **days from a release candidate, not weeks** — and materially closer than
the phase verdicts say, because several of those verdicts were produced by
instruments we already know are broken. The functionality is largely there. What
is missing is (a) one green CI run across three platforms at a single SHA, (b) a
native certification that saw the actual candidate rather than a tree 2,097
commits old, and (c) a short, finite list of eight concrete broken promises. None
of those is a missing feature. All of them are finishable.

---

## WHAT'S GOOD

**Real capability that did not exist on 13 July, proven on real hardware:**

| Surface | Proof |
|---|---|
| `index` build/status/search/verify | Live, **all three platforms**, nonce-bound, SHA-asserted binaries |
| `goal` durable objectives + Fleet | Survives `kill -9` mid-wave, Linux **and** Windows, effects 12/12/12 |
| `backup` create/verify/restore/recover | Live, including a real Windows long-path fix |
| `sandbox` status/exec | 50 greens, each with a differential activeness observation |
| `backend` local/container/ssh/cloud | Live on 3 of 4 |
| `plugin` 10-verb lifecycle | Live Linux + Windows — the only MET criterion in Phase 25 |
| `migrate` Hermes/OpenClaw/grok/gemini | Live against real installs |
| glibc reach | Floor 2.39 → **2.34**, live on five distro families |

**Ten new top-level CLI commands. Two new crates. 13 new protocol commands, 24
new events.** Zero regressions of the "we deleted something" kind — no crate
removed, no file under `crates/` deleted.

**Security, closed with live proof:** a root RCE in the ssh backend (caught before
it ever shipped); an egress hole letting any cloned repo disable the boundary; a
cross-user prompt leak proven on the wire; Windows AppContainer deny that was
silently inert on hardware; `PolicyGate` with zero production callers. `cargo deny`
5 → 0, `cargo audit` 4 → 0 with no ignore list.

**And the sweep that cuts against the pessimism:** all 53 CLI fields, 24 protocol
commands and 32 slash commands have real handlers. **No dead flags.** Docs are
~250KB with 10 overclaims — five of which *understate* the product.

---

## WHAT'S BAD

**Eight concrete broken promises.** Finite, specific, mostly small:

| # | We say | Truth | Fix |
|---|---|---|---|
| 1 | Grep returns matches or errors | Silent "no matches" on unreadable Windows targets — **the agent then deletes code**. Regression test is `#[cfg(unix)]`, so it cannot run where the bug lives | code |
| 2 | `README:266` `--i-accept-exfil-risk` | Flag does not exist. Code comments already say so; README never updated | text |
| 3 | `advanced.md:134` OAuth tokens "encrypted" | Plaintext; on Windows the perms are no-ops too | code |
| 4 | Edit `plugins.toml` (4 docs, 3 error strings) | **Never loaded from disk** | code |
| 5 | `--json-stream` emits `ready` | Missed on Windows — 15s probe vs 10s deadline. **Breaks Desktop first launch** | code |
| 6 | `channels.md:325` Slack/Discord exactly-once | Live replay produced two messages | text |
| 7 | `[default] read_only = true` | Accepted, enforced nowhere | code |
| 8 | Sub-agent turn limit 10 | Code default **200** — 20× on the only documented spend bound | text |

**Channel coverage.** 3 of 10 driven at the real platform. Slack and Discord both
claimed exactly-once on a mock and produced two messages live; corrected to
at-most-once. Telegram, WhatsApp, SMS, email, Signal, iMessage, MS Teams:
implemented, never measured.

**Credential ladder is unproven.** Built today, no real OS keyring exercised on
any platform. Two of seven sinks still deferred (OAuth storage is one — same as #3).

---

## WHAT'S UGLY

**Our instruments, in both directions.**

61 of 86 gate scripts had never been shown to fail. The anti-vacuity control was
itself a no-op. `.config/nextest.toml` carried `no-tests = "fail"`, which nextest
**silently ignores** — fail-closed came from a CLI default, the exact dependency
the key existed to remove.

We fixed the gates that falsely said PASS. **We never swept the ones that falsely
say FAIL** — and that is why the programme looks worse than it is:

- **23A-C1 "it ships to nobody" is FALSE.** Skills revoke/rollback/list ship in
  `wayland-core` today (`--skills-revoke`, `--skills-rollback`, `--skills-govern`
  → `wcore_cli::skill_govern`). The gate greps `release.yml` for the name of a
  *temporary workaround binary* that the wiring made redundant. It can never pass.
- **22-C1 is probably stale too.** `GoalCancelCommand` exists in the protocol and
  the control work is recorded complete. Needs confirming, not asserting.

Same defect class as the vacuous greens, pointing the other way. Every red verdict
produced by a grep needs re-checking against code before it is believed.

**Records that will actively mislead:** `STATE.md` and `HANDOFF.json` were
abandoned ~25 July and still say *"Phase 20, executing, 0 phases complete."*
`ROADMAP.md` says Phase 30 NOT STARTED; its verdict landed 29 July.
`CRITERIA-STATUS.md` is stale by its own rule.

**The certification.** Phase 28 certified `32e2f57d`; HEAD is **2,097 commits**
past it. Its own drift note: *"A release cut today would carry a passing
certification that never saw the shipped binary."*

**And the process cost.** 704 docs commits against 248 feat commits. 58% of the
diff is `.planning/` prose. Phase 21 graded four times, Phase 22 three times. We
spent more effort describing and re-grading the work than doing it.

---

## WHAT'S DONE

- 10 new CLI command groups; 8 of 10 with live receipts
- Two new crates (`wcore-exec-backend`, `wcore-gateway`)
- ~10 HIGH/critical security defects closed, most with live proof and controls
- Supply-chain: deterministic SBOM, signed release manifest, role-scoped trust
  root, `cargo deny`/`audit` clean
- glibc floor lowered and live-proven on five distros
- Two-tier danger model; untrusted-workspace default-off for hooks/MCP/skills
- **Today:** Windows CI unblocked (bsdtar + clippy), Linux TUI timing defect fixed,
  credential ladder landed, contract corpus verified fresh
- **23 of 51 success criteria MET-family**

---

## WHAT'S NOT DONE

1. **No three-platform green run at one SHA.** Two of three legs fixed today;
   Windows executing its first full suite as of writing.
2. **macOS: 18 failures.** 11 of them **cannot pass** — 8 `wcore-swarm` needing a
   Docker backend on a leg with `live-docker` off, 3 `voice_live_capture_mac`
   needing a microphone on a hosted runner that has none. 7 are real:
   session_journal path identity (3), user_model wire (3), deterministic loop (1).
3. **Native re-certification** against the actual candidate.
4. **The eight broken promises above.**
5. **Verdict truth pass** — re-check every NOT MET/PARTIAL against code.
6. **`CHANGELOG.md`** — moved 7 lines in 3,012 commits. Needs writing from scratch.
7. **`release-please.yml`'s release job is never executable** — `on:` is
   dispatch-only while its condition reads `github.event.head_commit.message`.
8. Channels 4–10 never measured; cloud backend leg unexercised.

---

## THE PLAN

Sequenced, with the gate that closes each step. Estimates assume the current
two-to-three concurrent lane cadence.

### Gate A — one green run, three platforms, one SHA  *(~1 day)*
- Windows: confirm the two fixes landed (in flight).
- macOS 11 impossible tests: gate on capability, or route to `sean-mac-arm64`
  which has audio. **Not by deleting assertions** — by making the requirement
  explicit so a skip reads as a skip.
- macOS 7 real failures: fix. `session_journal` path identity is the one that
  could be a genuine macOS product defect; treat it as such until disproven.
- **Exit:** one run id, three platforms, zero unexplained failures.

### Gate B — the eight broken promises  *(~1–2 days)*
- Three are text; fix immediately.
- Five are code. #1 (grep) first — it is the only one that makes the agent
  confidently *wrong* rather than merely limited, and its test must be made able
  to run on Windows.
- #3 routes OAuth storage through the credential ladder, closing a deferred sink
  and making the doc true in one change.
- **Exit:** each item has a test that fails without the fix.

### Gate C — verdict truth pass  *(~0.5 day)*
- Re-check every NOT MET and PARTIAL against code, not against documents.
- Where the criterion is satisfied: close it and **delete the lying gate**.
- Where the gate can never pass: fix or remove it.
- Refresh `STATE.md`, `ROADMAP.md`, `CRITERIA-STATUS.md` or delete them.
- **Exit:** no verdict in the tree rests on a gate that cannot pass.

### Gate D — re-certify native against the real candidate  *(~1 day)*
- Re-run the Phase 28 acceptance on the actual RC binary. This is the single
  biggest item and the one that makes the tag mean anything.
- **Exit:** a signed receipt whose candidate digest equals the shipped binary's.

### Gate E — release mechanics  *(~hours)*
- Re-verify contract digest at the final SHA (fresh as of now).
- Write `CHANGELOG.md` from the census.
- Fix or document the dead `release-please.yml` job.
- **Sean tags** `v0.12.26` via `gh workflow run release.yml -f tag_name=v0.12.26`
  (34 of 34 historical releases went this way; both secrets present).

**Realistic total: 3–5 working days**, with Gate A and Gate D the long poles and
the macOS 7 the main unknown.

---

## HONEST GRADE

Against the question *"how much actual work did we do?"* — a great deal, and more
of it landed than the scoreboard shows. The capability surface roughly doubled and
eight of ten new command groups carry live receipts on real hardware.

Against the question *"are we near a release candidate?"* — nearer than the
verdicts imply, and further than we told ourselves two weeks ago. The gap is not
missing functionality. It is proof that matches the binary we would actually ship,
plus eight promises to make good on.

The one thing that must not repeat: we graded ourselves with tools we had not
tested in both directions, and then believed the results in whichever direction
they pointed.
