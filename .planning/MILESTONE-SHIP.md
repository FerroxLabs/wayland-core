# MILESTONE — SHIP. Wayland Core to a released, tested product.

Opened 2026-07-30. Integration `plan/f20-unified-audit-repair` @ `b2ddf113`.

**Definition of done, in one sentence:** a tagged release whose every advertised surface has been
executed on every platform it is offered for, with no open HIGH, and a defensible statement of where
the product stands against its peers.

This file is the ship checklist. `.planning/LANE-BRIEF.md` still outranks it for *how* lanes work.

---

## 0. The three gates that are Sean's alone

Nothing below can substitute for these. **They are the critical path — every one of them blocks work
that is otherwise finished.**

| # | What | Blocks |
|---|---|---|
| S1 | **A real release trust root / signing key.** Everything in the supply chain is proven against a throwaway Ed25519 key generated into a temp dir. No release has ever gone through any of it. | `SUPPLY-*` promotion; a signed tag |
| S2 | **A cloud credential** for the hibernating remote-execution leg | `25-C1`; `REACH-*` → EFFECTIVE |
| S3 | **Merge to `main`, tag, publish, close issues, reply to core#254** | the release itself |

Two more that are cheap for him and unblock real work:

- **S4 — the `24-C1` per-channel delivery-semantics declaration.** The engineering is done: every
  adapter fixable in code has been fixed. What remains is a product decision about what we *promise*
  per adapter. One page, his call.
- **S5 — a second physical host with an SSH trust relationship** (`25-C2`). Everything was exercised
  against a separate machine *identity*, never a second machine.

---

## 1. What "released" requires — RC blockers

Re-measured 2026-07-30 by `lane/release-rank`, replacing a stale 7-item list. **Was 7, now 2.**

| # | Item | State |
|---|---|---|
| ~~1~~ | ~~`BL-LOCKFILE-DRIFT`~~ | **CLOSED `b2ddf113`.** `serial_test` was missing from `Cargo.lock`, breaking every `--locked` build including `release.yml:310`. Integration-only, never on main, which is why CI looked clean |
| 2 | `27-C5` — two aarch64 targets NOT MEASURED | `lane/27c5-aarch64` |
| 3 | `27-C2(c)` — three policy baselines | `lane/27c2c-baselines` |
| 4 | `24-C1` — per-channel semantics declaration | **S4, Sean** |

Plus one open HIGH that must not ship: **`F29-02-H1`** — `.cargo/audit.toml` silences advisories on a
stated "sole path" when the graph has three, two through `wcore-tools`, which parses user-supplied
docx/pptx/xlsx. `RUSTSEC-0194` is reachable. → `lane/f29-h1-advisories`.

And one MEDIUM under the standing policy that severity sets repair *order*, not disposition:
**`BL-UNTRUSTED-RESOURCE-LIMITS`** → `lane/resource-limits-clamp`.

**Last action before any tag, always: contract regeneration #4.** Generate on hetzner, rsync
`crates/wcore-protocol/contracts/` back, commit and push from the Mac.

---

## 2. What "a fully functioning agent" requires — the parity gaps

Ten capability families. **Zero are at `EFFECTIVE`.** Six at `REACHED`, three `CONSTRUCTED`, one
`SOURCE`. The scale is `ABSENT → SOURCE → CONFIGURED → CONSTRUCTED → REACHED → EFFECTIVE →
OPERATOR_COMPLETE → PACKAGED_PROVEN`, and the ledger's own rule is *"source presence alone never
earns effectiveness or parity."*

| Family | Now | Gap being closed | Lane |
|---|---|---|---|
| `GATEWAY-*` | CONSTRUCTED | **The widest measured gap vs peers.** macOS binary provably lacks the code; Windows never exercised | `gateway-platforms` |
| `MEDIA-*` | **SOURCE** — lowest | 4 of 5 criteria NOT MET; voice absent from every `default` feature list; video/TTS have zero cost sites | `media-gen-voice` |
| `PORT-*` | REACHED | **Nothing has ever been imported.** Both peers migrate from each other; Core has discovery only | `port-import` |
| `CONT-*` | REACHED | Governed skills contested; cache economics never executed | `cont-skills-cache` |
| `SUPPLY-*` | CONSTRUCTED | Update identity, revocation, rotation, rollback rehearsal all unbuilt | `supply-29-34` |
| `TXN-*` | REACHED–**REOPENED** | `F21-04-03` Windows re-proof missing — the reason for the demotion | `windows-legs-sweep` |
| `AUTH-*` | CONSTRUCTED | Architectural lead, operationally unproven; Linux-only | `windows-legs-sweep` |
| `24-C3` | NOT MET | edit/delete **0 of 10**; everything Linux-only | `24c3-channels` |
| `REACH-*` | REACHED | Blocked on **S2** and **S5** | — |
| `NATIVE-*` | REACHED | 24 cells still RED at the certified candidate; matrix never re-run at a candidate carrying the fix | queued |

---

## 3. What "tested as a product" requires

**The one comparative trial that exists is void, and this is the single most important line in this
file.** Phase 30 produced **9 allowed claims and 10 refused**. Every allowed claim is about process
integrity — the protocol was pre-registered, peers were pinned, the verifier can fail. **Every claim
about being at or ahead of a peer was refused by the checker.**

The cause is one instrument defect: the canonical script emits a tool call named `write_file`, and
**two of the three harnesses scored 0/30 on it**. All three then spent an identical 20.00 cost units,
so the "tie" was equal spend for unequal work — refused as confounded. The correctness comparison was
refused because the interval `[-0.1135, 0.1135]` contains zero.

So **no parity claim is currently defensible in either direction.** Re-running that trial with a
harness-portable script is queued as `phase30-retrial` and is what converts "we think we're good"
into something sayable.

---

## 4. Standing rules that outlive this milestone

Earned by measurement, each after a false green:

- **Run every control in both directions.** Can it fail, *and* can it pass. A permanently-red gate is
  worse than a permanently-green one, because it also hides real progress.
- **A skip is not a pass.** Count and report unrun cells.
- **`rtk` fabricates machine-readable counts, and the absolute path does not save you.** `--numstat`
  reported `162 0` for a diff deleting 40 lines. Redirect to a file, read with the Read tool.
- **Before testing for the absence of X, assert nothing you ran earlier could have created X.**
- **Grade off code and executed tests, never a SUMMARY.** Rows have been graded off a *finding*
  lane's summary while the *repair* sat in the branch's own ancestry.
- **A comment asserting a safety property the code does not implement** is this codebase's most
  common security defect class — three instances in `config.rs` alone.
- **No audit is immune to the failure mode it is auditing for.** The ledger re-grade lane, whose job
  was catching permanently-red instruments, was itself defeated by one.
