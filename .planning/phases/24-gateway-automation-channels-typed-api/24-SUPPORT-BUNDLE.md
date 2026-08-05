---
lane: support-bundle
branch: lane/support-bundle
base: 861d1b1a716240165209336b1fa38d36f9445716
finding-addressed: "F24-C4-H1 (HIGH) — wcore_gateway::support_bundle had zero production call sites and no CLI verb"
finding-held: "YES. Re-measured before building: 3 grep hits total (mod decl + 2 in its own test file), zero production call sites. Concept-level search of wcore-cli for bundle/diagnostic/doctor found only a TUI /doctor panel and two unrelated senses of 'bundle'. Both searches carried a known-positive liveness control in the same invocation."
grade-24-C4: "MET on Linux. The recovery half was already proven on HTTP/SSE; the support-evidence half now has an operator verb, live-driven on hetzner-dsm, with redaction proven by planted-secret positive control and a pre-fix/post-fix binary differential. NOT exercised on macOS or Windows — see Open."
redaction-proof: "PASS. Real sweep 0 hits over the bundle with a known-positive returning 1 in the SAME invocation, for both plants. manifest.redactions=2 is a positive non-zero, so the scrubber demonstrably acted rather than finding nothing. Binary differential: pre-fix build LEAKS the config secret (sweep rc=1, value visible in recent-log.txt), fixed build redacts it (rc=0, [REDACTED]). scripts/f24-secret-sweep.sh --selftest 5/5."
new-finding: "F24-SB-M1 (MEDIUM, non-blocking) — the `gateway` --help HEADER in crates/wcore-cli/src/main.rs:742 still enumerates the old verb set and omits support-bundle. Under-advertising, not advertised-but-dead; left unfixed to hold fence exposure at zero. One-line seam request below."
instrument-defects-mine: 3
instrument-defects-repaired: 3
fence-exposure: "ZERO. `git diff --stat 861d1b1a HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` is EMPTY, with a live control in the same invocation (crates/wcore-cli/src/gateway.rs = +32). No .github/ files touched. Line delta on both fence files: 0 added, 0 removed."
tests-added: 5
gates-activated: 1
status: complete
---

# 24-SUPPORT-BUNDLE — closing `F24-C4-H1`

Lane `support-bundle`, base `861d1b1a`. Built and live-driven on `hetzner-dsm`.

---

## 0. Bottom line

**The finding held, and it is now closed on Linux.** `wcore_gateway::support_bundle` — 543
lines of collector with a redactor, structural elision, a manifest and a mutation-gated test
suite — had **no way to run it**. It now has `wayland-core gateway support-bundle`, and the
bundle it produces has been opened, read, and swept.

**Two defects were found while wiring it that the naive wiring would have shipped**, and both
matter more than the missing verb did, because both produce a bundle that is *confidently
wrong* rather than absent.

`24-C4` is **MET on Linux**. It is **not** met on macOS or Windows, and I did not run there.

---

## 1. The finding, re-measured before building

The brief said findings on this programme have been wrong in both directions and told me to
re-derive. I did.

```
/usr/bin/grep -rn "support_bundle" --include="*.rs" crates/
```

**3 hits, none in production:** `crates/wcore-gateway/src/lib.rs:19` (`pub mod support_bundle;`)
and two inside `crates/wcore-gateway/tests/support_bundle_redaction.rs` (its own test, and a
doc-comment naming the test command).

**Liveness control, same invocation** (§3b-i — an absence is free on a dead instrument):
`/usr/bin/grep -rln "pub fn" crates/wcore-gateway/src` → **8 files**, non-zero.

**Concept search, not one keyword** (§3b-i.3): `bundle|diagnostic|doctor` across
`crates/wcore-cli/src/`. Every hit was a TUI `/doctor` panel, `self_update.rs`'s "bundled
release trust root", or `tui/auth.rs`'s OAuth "token bundle" — three different senses of the
word, none of them a support bundle. Liveness control: `grep -rln "gateway"` over the same
tree → **7 files**.

**Verdict: the grading lane was right.** I built rather than graded.

---

## 2. What shipped

`wayland-core gateway support-bundle [--out DIR] [--json] [--profile P] [--home H]`

| member | content | protection |
|---|---|---|
| `config-keys.txt` | `config.toml` KEY NAMES | structural elision — values never read in |
| `credential-keys.txt` | credentials store KEY NAMES | structural elision |
| `environment-keys.txt` | env var NAMES, secret-marked ones flagged | structural elision |
| `recent-log.txt` | 256 KiB tail of `gateway.log` | exact-secret scrubbing (the backstop) |
| `gateway-status.json` | **liveness-checked** state, pid, uptime, turns in flight, deliveries pending | derived, never copied — see §3 |
| `ledger-summary.json` | pending / abandoned / quarantined / dropped counts | bounded summary, not the unbounded journal |
| `manifest.json` | created_at, home, os, arch, binary path + version, member list, `known_secrets`, `redactions`, `absent_sources` | — |

Written only if the sources exist; anything expected and absent is **named** in
`absent_sources` rather than silently skipped.

**It is a directory, not an archive, by the library's design** — the operator can read exactly
what they are about to attach to a ticket. The verb prints the path and the `tar czf` command
rather than archiving on their behalf.

**Judged against "what does a support engineer actually need":** version, platform, whether the
thing is running *right now*, why it is not, what it was doing when it stopped, what
configuration exists (without the secrets), and what deliveries are outstanding. All present.
Deliberately excluded: the raw `deliveries.jsonl`, which is unbounded and whose counts say
everything the file does.

---

## 3. The two defects found while wiring, both of which produce a confidently-wrong bundle

### (a) The redactor never learned config-file secrets — the backstop had a hole

`learn_from_environment()` was the only bulk learn. Structural elision protects the *config
member*, so an `api_key` in `config.toml` never reaches `config-keys.txt`. **But the log member
is free text**, written by call sites that had that credential in scope — and the scrubber had
never heard of it. So a log line quoting the config's API key **shipped it verbatim**. The hole
sat exactly where the backstop is the only defence.

Fixed with `Redactor::learn_secret_values_from_file()`, which learns values whose KEY NAME
marks a secret. Values are held in memory to be scrubbed *for*; they are never written.

**Proven by binary differential, not assertion** — §4.3.

### (b) `gateway-status.json` outlives the process that wrote it

The running gateway republishes it every tick and **nothing removes it when that process
dies**. `read_live_projection()` (`gateway.rs:402`) exists precisely for this: it checks
`process_is_alive(record.pid)` FIRST and refuses to return a projection for a dead pid.

A bundle that copied that file verbatim would ship a `Running` claim with a pid that is gone —
**and a support bundle is created precisely when the gateway has died.** This is not an edge
case, it is the modal case, and it would be the first file a support engineer opens.

The status member is therefore **derived through the liveness check** and carries its own note
saying so. Observed in D1 (§5).

---

## 4. Redaction proof

`0` hits means "redacted" — and a missing bundle, an unwritable path, an errored grep, a
mangled variable and a bundle that collected nothing **all also produce `0`**. The sweep alone
proves nothing. Four arms, each with a distinct exit code, in
`evidence/24-support-bundle/sb-proof.sh`. Driven against the **real binary** on `hetzner-dsm`.

`scripts/f24-secret-sweep.sh --selftest` first: **5/5 PASS**.

### 4.1 The four arms — all green

| arm | guards | measured |
|---|---|---|
| **3** secret really IS in the input (exit 71) | redaction of something never present is free | config plant in **2** input files, env plant in **1** |
| **1** bundle exists, non-empty (exit 72) | absent-because-nothing-generated | **5** files; **1861 bytes** by THREE independent methods — `cat\|wc -c`, `du -sb`, `stat -c%s` summed — **all three agree**, excluding the `wc -c`-returns-0-for-a-72-byte-file defect |
| **4** bundle actually COLLECTED (exit 74) | a bundle that collects nothing passes every redaction test | non-secret marker `COLLECTION-MARKER-8fa31c` present **1×** in `recent-log.txt`; `api_key` NAME present **1×** in `config-keys.txt` |
| **2** the sweep can FIND this needle (exit 73) | dead instrument | known-positive over a control dir → **rc=1 (found)** for BOTH plants, **in the same invocation** as the real sweep |

**Real sweep: hits = 0, rc = 0 CLEAN, for both plants**, with the known-positive returning 1
alongside it.

### 4.2 The redaction is positively evidenced, not merely absent

`manifest.redactions = 2` — a **non-zero** count. The scrubber demonstrably replaced two values
rather than finding nothing to do. The scrubbed log, read out of the bundle:

```
INFO  gateway starting COLLECTION-MARKER-8fa31c      <- non-secret marker SURVIVED
ERROR auth rejected for key [REDACTED] (401)         <- config-file secret
ERROR upstream refused bearer [REDACTED]             <- environment secret
```

`config-keys.txt` carries `api_key  [value elided]` and `base_url  [value elided]` — names
kept, **no value survives, not even the harmless-looking URL**.
`environment-keys.txt` carries `WL_SUPPORT_TOKEN  [value elided: name marks a secret]`.

### 4.3 The third assertion, at the binary level

The brief asks for evidence the pre-fix state would have missed it. For the **verb** that is
trivially true — it did not exist, so there was no bundle to sweep and no `0` to report. Stated
precisely rather than waved.

For the **redaction fix** it is not trivial, so I measured it. Built a pre-fix simulation
binary (the two `learn_secret_values_from_file` calls removed, `learn_from_environment()`
retained — exactly the prior surface), against a config secret held in **no environment
variable**:

| | `known_secrets` | `redactions` | `recent-log.txt` | sweep |
|---|---|---|---|---|
| **pre-fix** | 1 | **0** | `key sk-cfgplant-A1b2C3d4E5f6G7h8 (401)` | **rc=1 LEAK** |
| **fixed** | 2 | **1** | `key [REDACTED] (401)` | **rc=0 CLEAN** |

Guards on the differential itself: the two binaries were proven **distinct** (different `cksum`
*and* different size — a comparison between two things that both failed to be produced would
otherwise pass), the secret was proven **absent from the environment**, and a **collection
control** (`MARK-9d21` present) ran on both legs so neither result could come from an empty log.

**The first run of this differential was wrong and I caught it:** the "fixed" leg reused
`target/debug/wayland-core`, which was still the pre-fix artifact I had just built into it. It
reported the fixed build leaking. Rebuilt, re-verified the cksums differ, re-ran.

### 4.4 A dead gate, activated

`live_bundle_canary` in `crates/wcore-gateway/tests/support_bundle_redaction.rs` was
`#[ignore]`d — *"live: requires a bundle produced by a running gateway"*. It was written for
this plan and **had never been runnable, because nothing could produce a bundle.**

It runs now, against a bundle the new verb produced:

```
running 1 test
test live_bundle_canary ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out
```

**And it can fail** — pointed at the unredacted seeded inputs:

```
panicked: the LIVE support bundle leaked the canary in:
  ["/root/sb-canary/h/config.toml", "/root/sb-canary/h/gateway.log"]
test result: FAILED. 0 passed; 1 failed
```

---

## 5. Degraded cases — observed, not asserted

This is diagnostic machinery; it runs *after* something has already failed.

| case | result |
|---|---|
| **D1 stale status file + dead gateway** | Planted `gateway-status.json` claiming `running / uptime 98765 / turns_in_flight 7` with dead pid 999999. Bundle reports `running=false`, `state=uninstalled`. **The stale `98765` and `turns_in_flight: 7` appear nowhere in the bundle**, with a liveness control proving the same grep DOES find both in the planted file. **4/4 PASS** |
| **D2 empty home, no config, no log** | Bundle still produced, rc=0. **3** absent sources NAMED in the manifest. No empty log member invented. **3/3 PASS** |
| **D3a unwritable — parent is a regular file** | rc=1, *"Not a directory (os error 20)"*, **no success banner**, blocking file untouched. **4/4 PASS** |
| **D3b unwritable — genuinely unprivileged writer** | rc=1, *"Permission denied (os error 13)"* **from the product**, no partial bundle. **6/6 PASS** |
| **D4 non-empty output directory** | Refused rc=1 naming the reason; pre-existing file untouched; no members written. **3/3 PASS** |

---

## 6. Three instrument defects in MY OWN harness, all repaired in-lane

§6b-ii: a written-up instrument defect is a defect you have agreed to keep.

**(i) A pipe stole the exit status.** My first fmt check was
`cargo fmt --all -- --check 2>&1 | tail -5; echo $?` — which reported `tail`'s status.
Re-run unpiped: **rc=0, 0 diff lines.** §3.2's first named class, hit within an hour of reading it.

**(ii) D3 measured root's `CAP_DAC_OVERRIDE`, not the product.** `chmod 500` while running as
root means the write *succeeds*, so D3 reported three product failures that do not exist. I
did not note-and-move-on. Repaired, with a **control that proves the bypass** — a plain `touch`
also succeeds in the same 500 dir as root — and replaced with two causes root cannot bypass
(ENOTDIR, and a real unprivileged user).

**(iii) D3b's first run SELF-PASSED, and this is the sharpest one.** It returned `rc=126`,
`env: '/root/.../wayland-core': Permission denied` — `nobody` could not **execute** the binary
under `/root`, so **the product never ran at all**. All three assertions passed anyway:
`rc != 0` ✓, "names permission denied" ✓ (that was `env`'s own message, not the product's), "no
partial bundle" ✓ (nothing ran). Repaired with **CONTROL A** (prove `nobody` can execute the
binary — `--version` rc=0) and a **SANITY arm** (prove the same user + same verb *succeeds* on
a writable path, rc=0), without which `rc != 0` could just mean the verb never works for that
user.

Plus the differential stale-artifact error in §4.3, caught by its own assertion going red.

---

## 7. Gates

All on `hetzner-dsm`, `/root/wayland-support-bundle`, worktree `hz/support-bundle`. Counts read
back explicitly — never exit status alone.

| gate | result |
|---|---|
| `cargo build -p wcore-cli --bin wayland-core` | **rc=0** |
| `cargo test -p wcore-gateway --lib support_bundle` | **10 passed; 0 failed; 0 ignored; 34 filtered out** (8 pre-existing + 2 new) |
| `cargo test -p wcore-gateway --test support_bundle_redaction` | **4 passed; 0 failed; 1 ignored; 0 filtered out** |
| `live_bundle_canary` (was unrunnable) | **1 passed; 0 filtered out**, and proven able to fail |
| `cargo test -p wcore-cli --lib gateway::` | **12 passed; 0 failed; 0 ignored; 1836 filtered out** (9 pre-existing + 3 new) |
| `cargo clippy -p wcore-gateway -p wcore-cli --all-targets -- -D warnings` | **rc=0** (sole warning line is the pre-existing `imap-proto` future-incompat notice on a dependency) |
| `cargo fmt --all -- --check` (Mac) | **rc=0, 0 diff lines** — unpiped |
| `scripts/f24-secret-sweep.sh --selftest` | **5/5 PASS** |

Every count above is non-zero where it should be, and `0 ignored` / `N filtered out` were read
from unproxied output over ssh — not through the local `cargo` proxy, which strips exactly
those two fields.

---

## 8. Fence exposure

```
BASE=861d1b1a716240165209336b1fa38d36f9445716
/usr/bin/git diff --stat "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
```
→ **EMPTY.** Liveness control in the same invocation:
`git diff --stat "$BASE" -- crates/wcore-cli/src/gateway.rs` → **+32**, non-empty.

**Line delta on both fence files: 0 added, 0 removed.** No `.github/` file touched.

This was achievable because `Gateway` is **already registered** in `main.rs:745`, so the verb is
a subcommand, and the implementation lives in `crates/wcore-cli/src/gateway/support.rs` — a
child module declared in `gateway.rs`, not in the shared `lib.rs`.

### Files changed

| file | delta |
|---|---|
| `crates/wcore-gateway/src/support_bundle.rs` | **+119**, 0 removed (one method, three tests) |
| `crates/wcore-cli/src/gateway.rs` | **+32**, 0 removed (module decl, enum variant, dispatch arm, doc note) |
| `crates/wcore-cli/src/gateway/support.rs` | new, **298** |

Total `crates/` delta vs base: **449 insertions, 0 deletions, 3 files** — no file outside these
three was touched, and nothing anywhere was deleted.
| `.planning/phases/24-.../` | notes, this file, 3 evidence drivers |

### Seam request for the orchestrator (do NOT let me touch it)

`crates/wcore-cli/src/main.rs:742` — the `gateway` `--help` header still reads
*"install / uninstall / start / stop / restart / status / drain, plus the `run` verb"* and omits
`support-bundle`. **One line, fence file, cosmetic.** The verb itself IS listed in the
`Commands:` block directly below, so it is discoverable; this is *under*-advertising, which is
the safe direction. Logged as `F24-SB-M1` (MEDIUM, non-blocking) rather than fixed, to hold
fence exposure at zero for the merge queue.

---

## 9. Verdict on `24-C4`

> *"Typed authenticated clients recover event gaps and produce useful redacted health/log/
> support evidence."*

**MET on Linux.** The recovery half was already proven on HTTP/SSE by prior lanes. The support
half now has an operator verb that produces a bundle which is **useful** (§2), **actually
redacts** (§4, with a positive control and a binary differential), and **works when things are
broken** (§5) — which is the only condition under which anyone will ever run it.

### What I did NOT do

- **macOS and Windows: not exercised.** Zero runs. Everything here is Linux on `hetzner-dsm`.
  The code is platform-neutral (`std::fs`, `std::env::consts`) and the only platform-specific
  call is `is_registered()`, which was already tri-platform — but *platform-neutral by reading*
  is not *proven*, and I am not claiming it. If `24-C4` must be MET on all three families, this
  lane does not close it there.
- No soak, no volume: the bundle was driven against small planted fixtures, not a real
  long-running gateway's multi-megabyte log. `MAX_LOG_BYTES` tail-bounding is exercised by unit
  test, not at scale.
- Did not fix `F24-SB-M1` (fence).
- Did not merge, PR, tag, release, close an issue, or run `wcore-contract generate`.
- No credential of any kind was used. The planted values are synthetic canaries, deliberately
  legible; nothing real was read, printed, or transmitted.
