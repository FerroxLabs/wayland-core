# Phase 25 — PHASE VERDICT

**Remote Reach, Nodes, and Plugin Lifecycle**

| | |
|---|---|
| **Graded by** | lane `grade-25` |
| **Date** | 2026-07-29 |
| **Base commit** | `861d1b1a716240165209336b1fa38d36f9445716` |
| **Branch** | `lane/grade-25` |
| **Method** | Goal-backward. Every criterion re-derived from `.planning/ROADMAP.md`; every number re-measured from evidence captures with `/usr/bin/` tools; two findings re-verified against `crates/` source. No arithmetic inherited from any SUMMARY. |
| **Prior verdict file** | **None.** Phase 25 had never been graded either way. This is the first. |

---

## VERDICT

> ### Phase goal **NOT FULLY ACHIEVED**.
>
> **1 MET · 1 MET-WITH-STATED-EXCEPTIONS · 2 PARTIAL · 0 NOT MET**

| # | Success Criterion (abbreviated) | Grade |
|---|---|---|
| 1 | Same task runs local / container / SSH / hibernating cloud, with equivalent policy, receipts, **cancellation**, and **cleanup** | **PARTIAL** |
| 2 | Nodes pair, advertise, revoke, recover offline, mixed versions, without losing authority attribution | **MET-WITH-STATED-EXCEPTIONS** |
| 3 | Plugins scaffolded → tested → signed → installed → approved → inspected → updated → rolled back → removed → published → recovered | **MET** |
| 4 | Compromised keys/plugins/backends and denied secret/**egress** paths fail closed with no orphaned execution | **PARTIAL** |

The phase's own `25-PHASE-STATUS.md` header table claims **all four MET**. That table is
**not supported by the evidence it cites** and is superseded by this file. The competitive
ledger's claim — that `REACH-*` carries **exactly one** MET Success Criterion — is
**corroborated**: Criterion 3, and only Criterion 3, is MET as written.

**What the phase genuinely earned, and it is a lot.** Nine HIGH defects were found and
fixed, every one a *false answer* rather than a crash — the class a green suite cannot see.
Three separate structural false-zeros were caught (Windows `ps -eo`, the cloud nonce filter,
the local scan). Two lanes wrote up their own harness defects rather than burying them. This
is not a phase that failed; it is a phase whose *bookkeeping* outran its measurements on two
criteria.

---

## Criterion 1 — four backends, one task — **PARTIAL**

> *"The same task runs locally, in a container, over SSH, and on one hibernating cloud backend
> with equivalent policy, receipts, cancellation, and cleanup."*

The criterion has two independent halves: **four surfaces run the task**, and **four
properties are equivalent across them**. The four-surface half substantially holds. The
property half does not.

### What is proven

| surface | run | receipt | evidence |
|---|---|---|---|
| local | ✓ | ✓ | `evidence/25-01-equivalence-ledger.txt:1`, `evidence/25-01/receipt-local.json` |
| container | ✓ | ✓ | `evidence/25-01-equivalence-ledger.txt:2` |
| ssh | ✓ | ✓ | `evidence/25-01-equivalence-ledger.txt:3` |
| cloud | ✓ | ✓ | `evidence/25-cloud-ledger.txt` `F25-SC1-CLOUD-RUN: PASS exit=0 machine=8ed9d7dc3e5618 terminal=Success wall_ms=9745` |

`NORMALIZED DIFF: EQUIVALENT` at both commits. The cloud leg is genuinely strong: hibernation
is proven a **suspend, not a stop**, by a four-clause discriminator driven on **one machine**
minutes apart (`evidence/25-cloud-suspend-vs-stop-control.txt` — `/dev/shm` witness KEPT vs
MISSING, `boot_id` `080bbc1b…`→`080bbc1b…` vs →`751ec4fb…`, `previous_state` `"suspended"` vs
`"stopped"`). A separate **provenance gate** (a task reading `/etc/alpine-release`, a file the
Ubuntu controller does not have) proves the work ran on the machine — which the equivalence
diff structurally cannot detect, and which caught a real defect where the controller was
echoing the input back.

### Gap 1a — cancellation on the cloud surface was NEVER exercised, at any commit

The criterion names cancellation explicitly. Measured, with the instrument proven alive in
the same invocation:

```
grep -c "F25-SC1-CLOUD-RUN"  evidence/25-cloud-ledger.txt   ->  1     (instrument alive)
grep -c "CLOUD-CANCEL"       evidence/25-cloud-ledger.txt   ->  0
grep -rn -i "cancel" evidence/25-cloud-{live-run,provenance,orphan-control}.txt  ->  0
grep -rc "machine"   evidence/25-cloud-live-run.txt         ->  3     (instrument alive, same files)
```

At 25-01 the marker exists and is negative: `F25-SC1-CLOUD-CANCEL: NOT-RUN … the cloud leg
never started, so there was nothing to cancel` (`evidence/25-01-equivalence-ledger.txt:8`).
At the cloud commit the marker does not exist at all. `25-CLOUD-SUMMARY.md` concedes it
directly: *"Did not exercise cloud cancellation live."*

Cancellation was proven on local, container and ssh
(`evidence/25-01-equivalence-ledger.txt:5-7`). It is **0 of 1** on the surface the criterion
was written to add.

### Gap 1b — cleanup is DEFECTIVE on the ssh surface, and the defect is still open

`25-HOSTS-SUMMARY.md` FINDING 5 (MEDIUM, **BACKLOG, unfixed**): the ssh remote runner leaves
its task root behind on failure — `set -e` aborts at `wait`, so `rm -rf "$root"` never runs
and **`input.bin`, the task's input bytes, is left on the far end**. Six such roots were found
on the real Windows node and purged by hand. The lane names the consequence itself: *"Touches
Criterion 1's 'cleanup'."*

### Stated qualification (not itself a gap) — the four-surface claim spans two commits

`evidence/25-cloud-ledger.txt` records `F25-SC1-SSH-AT-THIS-COMMIT: NOT RUN` —
`WAYLAND_EXEC_SSH_TARGET` unset, the `f25-ssh-target` host entry gone. Local + container +
cloud were diffed together at `5e620ef0`; ssh was proven at 25-01's commit. I re-verified the
env var is **still unset** on `hetzner-dsm` today. Local and container appear in *both* diffs
and agree, which bridges the composition credibly. I accept it as a stated qualification
rather than a failure — the lane disclosed it rather than implying one run covered four.

Likewise `F25-SC1-SSH-TARGET: CONTAINERIZED-SSHD` — same physical host, `backend.instance_id`
identical across all three receipts, disclosed by 25-01 rather than glossed. The criterion
says "over SSH", not "to a second machine", so the transport claim stands.

### Why PARTIAL and not MET-WITH-STATED-EXCEPTIONS

Two of the four enumerated equivalence properties are compromised: one is 0% demonstrated on
a required surface, the other is a **reproduced, still-open data-residue defect**. That is
past the width of a carve-out. Equally it is not NOT MET — all four surfaces genuinely run
the same task and diff to EQUIVALENT, with the hardest sub-claim (real hibernation) proven
against a control that could have failed.

---

## Criterion 2 — nodes — **MET-WITH-STATED-EXCEPTIONS**

> *"Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions
> without losing authority attribution."*

**Every named property was demonstrated through the shipped binary across two genuinely
separate physical hosts** — `hetzner-dsm` (Linux controller) and `SeanD@seandesktop`
(Windows node). Source: `evidence/25h-node-ledger.txt`, `25-HOSTS-SUMMARY.md`.

| property | ledger verdict | what makes it real |
|---|---|---|
| PAIR | `F25H-C2-PAIR: PASS` (`25h-node-ledger.txt:51`) | node key `fa808c4de95c…` vs controller `ffdcb20256a0…` — a loopback proof would have shown one key on both sides |
| PAIR (negative) | `PASS` (`:66`) | unreachable far end refused exit 1, **left no record** |
| ADVERTISE | `PASS` (`:107`) | genuinely different real-probe capability sets: node `appcontainer` available / `container` unavailable; controller `bubblewrap` / `container` available |
| MIXED VERSIONS | `PASS` (`:139`) | a **second Windows binary** built with `NODE_CONTRACT_MAJOR = 99` — not a flag, not a hand-edited record; refused exit 1 while the supported node on the same physical host still accepts exit 0 |
| REVOKE | `PASS` (`:159`) | refused with `NOT falling back to another node`; far end could not re-pair itself despite a genuine proof |
| OFFLINE | `PASS` (`:203`) | a **real network partition** — `iptables -I OUTPUT -d 100.109.207.54 -p tcp --dport 22 -j DROP`; work refused, not rerouted |
| RECOVER | `PASS` (`:218`) | rule removed, node reads LIVE, accepts work |
| ATTRIBUTION | HOLDS after all five disruptions (`:68-72`) | see the discriminating control below |

**Attribution is proven discriminating, not merely green.** The negative control
(`evidence/25h-attribution-negative.txt`) is correctly constructed: a byte-copy of the node's
real state dir with **only** `keys/node.key` removed, so the backend key is
byte-identical (`d01d8180bbac` both sides) and node identity is the single variable.
Positive `EXIT=0` "attribution HOLDS … key fa808c4de95c"; negative `EXIT=1` "attribution
BROKEN … key 8462f515c20e but the pinned record … carries fa808c4de95c". A HOLDS therefore
means something. The lane discarded **two** earlier attribution runs of its own as invalid and
said so — that is the discipline this grade rewards.

### Stated exception 2a — the controller cannot verify a node-minted receipt's identity

Verified in source, not inherited (`grep -c "" crates/wcore-cli/src/node.rs` → **623**,
instrument alive):

- `crates/wcore-cli/src/node.rs:507` — `fn backend_key_from(receipt: &ExecutionReceipt)`
- `crates/wcore-cli/src/node.rs:512` — `"this host does not hold the signing key for backend '{}'"`

Ledger: `F25H-C2-ATTRIBUTION-ON-CONTROLLER: CANNOT VERIFY (exit 1)`
(`evidence/25h-attribution-ledger.txt:45`). Attribution can only be audited **on the machine
that did the work**. `25-HOSTS-SUMMARY.md` FINDING 4, MEDIUM, open.

The ledger annotates this *"this is the criterion's clause, so a failure here is the finding,
not a harness problem"* — a more conservative reading than its own SUMMARY took. I weighed
both and land on MET-WITH-STATED-EXCEPTIONS: the criterion's clause is *"without **losing**
authority attribution"*, and attribution is demonstrably not lost — it is stamped in the
receipt, survives five disruptions, and is cryptographically checkable against the
controller's pinned record. The criterion does not say *the controller* performs the check.
A reader who reads it that way should treat this criterion as PARTIAL; I am recording both
readings rather than only the one that grades well.

### Stated exception 2b — `node probe` never refreshes the advertisement

Verified in source: `crates/wcore-cli/src/node.rs:381` computes
`NodeAdvertisement::observe(...)`, and `:395` discards it — **`let _ = ad;`**. `node probe`
prints `advertised before:` and `advertised now:` which are therefore always identical, while
the module doc says the refresh "is the point". `25-HOSTS-SUMMARY.md` FINDING 7, LOW, open.
The criterion's "advertise capability" clause is satisfied at **pairing**, which is where it
was measured; the refresh surface is dead.

---

## Criterion 3 — plugin lifecycle — **MET**

> *"Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated,
> rolled back, removed, published, and recovered."*

### Arithmetic re-derived — the criterion names ELEVEN verbs, not twelve

scaffolded · tested · signed · installed · approved · inspected · updated · rolled back ·
removed · published · recovered = **11**. The phase calls this a "twelve-verb lifecycle";
the twelfth (`verify`) is a **superset addition**, not a criterion item. The phase therefore
proved *more* than the criterion asks, not less.

All eleven criterion-named verbs are `PASS` through the shipped release binary on
`hetzner-dsm`, each with an independently observed state change
(`evidence/25-02-lifecycle-ledger.txt`, 18 lines, all PASS), plus four negative cases:
tampered-refused, unapproved-refused, approved-loads, rollback-digest-equal.

### The `test` verb is NOT vacuous — checked, because this is the phase's own failure class

`evidence/25-02-verb-test.txt:14`:

```
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Two real tests executed, **`0 ignored` and `0 filtered out` both present and both zero** —
the two fields the anti-vacuity rule requires and that the `rtk` cargo proxy strips. Read from
a committed capture, so the proxy could not have removed them. This matters especially here
because 25-02's own FINDING 4 was *"both shipped templates were unusable; their smoke tests
skipped, so it looked green"* — the exact vacuity defect, found and fixed inside this phase.

### Live driving found four HIGH defects a green suite could not

`plugin sign` wrote the signature where the verifier never looks · `plugin install` had **no
path at all** for a Wayland-native plugin · `plugin remove` could not remove a marketplace
install · both shipped templates unusable with skipped smoke tests. All four are false
answers, all found by running the binary.

### Recorded, but not deductions from this criterion

- **Windows is a bonus, not a requirement.** Criterion 3 names no platform (contrast Phase 24
  SC5, which explicitly names macOS/Linux/Windows). Windows ran 11 of 12 verbs —
  `F25-SC3-WIN-VERB-NEW: NOT-RUN` because `cargo-generate` is absent, a **toolchain absence,
  not a product defect** — with all four negative cases holding and one PARTIAL
  (`F25-SC3-WIN-NEG-APPROVED-LOADS`).
- **A ledger line overstates its own cited evidence.** The Linux ledger's last line reads
  `F25-SC3-WINDOWS: PASS … reason=full-twelve-verb-drive-ran`, while the Windows ledger it
  points at records `new` as NOT-RUN. Bookkeeping defect; does not touch the Linux grade,
  but it is the same class as the two Criterion 4 defects below and should be corrected.

---

## Criterion 4 — fail closed, no orphans — **PARTIAL**

> *"Compromised keys/plugins/backends and denied secret/egress paths fail closed with no
> orphaned execution."*

Two halves. **The no-orphan half is MET and is the best-instrumented work in the phase. The
fail-closed half has a named clause that was never demonstrated, on either host.**

### Half B — no orphaned execution: **MET**

Measured on every reference surface, each checked in **both** directions:

| surface | negative (unused nonce) | positive | evidence |
|---|---|---|---|
| local | `0`, scanner/manual AGREE | planted=1, scanner=1, manual=1 | `evidence/25-04-enum-linux-ps{,-planted}.txt` |
| container | MEASURED | — | `evidence/25-04-enum-container-docker.txt` |
| cloud | `0 (MEASURED)` | **a REAL leaked machine** `82d1d97b062338`, `count 1 (MEASURED)`, exit **1** | `evidence/25-cloud-orphan-control.txt` |
| ssh — POSIX far end | `0 (MEASURED)` | **a REAL orphan the product left**, pid 1170 alive, `2 (MEASURED)`, exit **1** | `evidence/25h-ssh-orphan-ledger.txt` |
| ssh — real Windows host | `0 (MEASURED)` | pid 2481 reparented to 1, `1 (MEASURED)`, exit **1** | `evidence/25h-win-ssh-orphan-ledger.txt` |
| ssh — far end with no `ps` | `NOT MEASURED` (refuses to emit a number) | same binary still MEASURES against the POSIX far end | `evidence/25h-nops-control.txt` |

**Neither positive control was planted for the scanner's benefit.** The cloud one was a real
billable machine leaked by a `tail -1` parse defect in the lane's own script. The ssh one was
produced by killing the controller `-9` mid-task so the `setsid` remote child was genuinely
orphaned. This half found three structural false-zeros, including a scan that reported
`0 (MEASURED)` on Windows **while an orphan ran**.

Two limitations recorded and accepted: when the sweep cannot run, `backend scan` exits 0 even
if the primary signal found an orphan (the row is carried into the reason text); and the
Windows ssh measurement used planted orphans in the runner's exact shape, because the ssh
backend cannot run *any* task on a Windows far end (`setsid` absent — FINDING 6, open).

### Gap 4a — the "denied EGRESS path" clause was never demonstrated, on either host

The criterion names **five** compromise classes. Only **four distinct mechanisms** were
exercised.

**On Linux, denied-secret and denied-egress are the same command exercising the same
mechanism**, run twice with different output paths:

```
evidence/25-04-fail-closed-linux.log:64   $ … backend run --backend cloud --receipt-out …/receipt-cloud.json    [exit=1]
evidence/25-04-fail-closed-linux.log:156  $ … backend run --backend cloud --receipt-out …/receipt-egress.json   [exit=1]
```

Both refuse with the identical verdict — `backend cloud has no credential
(WAYLAND_F25_CLOUD_TOKEN); refusing to run and NOT falling back (probe CredentialAbsent)`.
That is a **credential-absence** refusal, not an egress-policy denial. The phase's own
`25-04-FAIL-CLOSED-EVIDENCE.md` §6 concedes exactly this:

> *"**A real egress policy denial.** No egress policy is installed on either proof host, and
> the only credentialed egress surface has no credential, so no outbound request is ever
> attempted. What is proven is that the surface fails closed; what is NOT proven is a
> policy-level deny of an attempted request."*

### Gap 4b — on Windows the denied-egress ledger entry CONTRADICTS its own capture

This is a distinct, harder defect and it is the one finding here that a reader must not miss.

The Windows ledger records:

```
evidence/25-04-fail-closed-windows-ledger.txt:5
F25-SC4-CASE-DENIED-EGRESS: REFUSED host=seandesktop exit=1 verdict=egress-surface-refused-without-credential
```

The capture it cites records the opposite:

```
evidence/25-04-win-case-denied-egress.txt
COMMAND: wayland-core.exe backend probe cloud
…
EXIT: 0
```

I enumerated **every** command in the Windows fail-closed log — there are six, and the
denied-egress one is a `probe`:

```
$ /usr/bin/grep -n "^\$ \|^### " evidence/25-04-fail-closed-windows.log
 6: $ wayland-core.exe backend run --backend local --receipt-out C:\f25-04-receipt.json
20: $ wayland-core.exe backend receipt verify C:\f25-04-receipt.json     (positive control)
32: $ wayland-core.exe backend receipt verify C:\f25-04-tampered.json
44: $ wayland-core.exe backend receipt verify C:\f25-04-attest.json
56: $ wayland-core.exe backend run --backend cloud …                     (denied-secret)
69: $ wayland-core.exe backend probe cloud                               (denied-EGRESS — exits 0)
80: $ wayland-core.exe backend run --backend local …
96: $ wayland-core.exe backend receipt verify C:\f25-04-rotatedout.json
```

**There is no `backend run` egress leg on Windows.** The lane caught this exact defect on
Linux and wrote the correction up verbatim — *"Claiming a nonzero exit that did not happen is
precisely the engineered green this program forbids"* — then re-ran **only the Linux leg**.
The Windows ledger line was left asserting an exit code that never occurred. This is
§6b-ii of the lane brief in its purest form: **a defect that was documented and then recurred
because the instrument was not repaired everywhere it lived.**

### Gap 4c — the compromised-KEY refusal is live-proven only incidentally

Both hosts refuse the rotated-out receipt at exit 1, but via **body digest mismatch**, not via
key identity. `backend receipt verify` is integrity-only *by design* and prints so:
`IDENTITY: NOT ESTABLISHED by this command. A receipt cannot authenticate itself…`
(`evidence/25-04-fail-closed-windows.log:19-28`). The lane corrected its own label to
`rotated-out-signature-refused-via-body-digest-cli-verify-is-integrity-only` and states the
identity half rests on the unit test
`case_rotated_key_is_refused_against_the_new_pinned_identity`, **not on the CLI**. That is
honest and I am not treating it as a failure — but the *live* proof that a compromised key
fails closed is incidental to the digest check, and it compounds with Criterion 2's
exception 2a (the controller structurally cannot check identity at all).

### What half A does prove

Both hosts carry a **vacuity control** — *"verify the intact receipt (must PASS or the cases
are vacuous)"* — passing at exit 0 before any hostile case runs. Tampered-bundle,
attestation-mismatch and denied-secret all refuse at exit 1 with distinct named verdicts and
produce no receipt. The **plugins** clause is covered live by 25-02's negative cases
(tampered refused, unapproved refused) on both hosts.

### Why PARTIAL

Half B is fully MET and exceptionally well instrumented. Half A covers plugins, backends and
denied-secret cleanly; keys incidentally; and **egress not at all** — with a ledger entry on
Windows asserting a refusal that the evidence shows did not happen. A criterion cannot be MET
while one of its five named clauses is unexercised on every host and a second host's ledger
misstates its own capture.

---

## Cross-record reconciliation

Three mutually inconsistent records existed before this file. They resolve as follows.

| record | claim | disposition |
|---|---|---|
| `25-PHASE-STATUS.md` header table (2026-07-28) | **4 MET** | **NOT SUPPORTED.** Inflated on C1 and C4. Superseded by this file. |
| `25-PHASE-STATUS.md` verbatim section (2026-07-27) | 1 MET (C3, "on Linux") | Directionally right; C2 and C4's "NOT MET" reasons (SSH trust "reserved to Sean") were themselves wrong and were correctly withdrawn. |
| Competitive ledger, `REACH-*` SOURCE → REACHED | **exactly one MET** | **CORROBORATED.** Criterion 3, and only Criterion 3. |
| `evidence/25-01-equivalence-ledger.txt:10` `F25-SC1-VERDICT: NOT-MET` | C1 not met | Superseded — the cloud leg later ran. But the successor's `MET` overshot; PARTIAL is correct. |
| `evidence/25-cloud-ledger.txt` `F25-SC1-VERDICT: MET` | C1 met | **Overstated** — the same lane concedes it never exercised cloud cancellation, a clause the criterion names. |

The failure mode is consistent and worth naming: **individual lanes measured honestly and
graded their own work generously.** Every gap in this verdict was disclosed *somewhere* in the
phase's own evidence — in a §"What this lane did NOT do", in a ledger annotation, in a
BACKLOG finding. What was missing was anyone reading the disclosures back against the
criterion text. Three ledger/summary lines assert more than their captures support (Windows
twelve-verb, Windows denied-egress exit code, C1 MET).

### Scope note — F25-05 has no criterion and no plan

`.planning/ROADMAP.md:127` lists Requirements **F25-01 … F25-05**, but the phase defines four
Success Criteria and four plans (25-01 … 25-04). F25-01 and F25-02 are addressed by 25-01 and
explicitly *not* marked complete by it (`25-01-SUMMARY.md` §8). **F25-05 is claimed by no plan
and graded by no criterion.** Flagged, not graded — this needs a roadmap owner's answer, not a
verifier's guess.

---

## Costed gap list

Estimates are **lane-sessions** (one focused lane run). "Credential" means a Sean-reserved
secret or authorization is required before work can start.

| # | Crit | Missing capability (one line) | Sessions | Credential? |
|---|---|---|---|---|
| **G1** | 1 | **Exercise `backend cancel` against a live cloud machine** and record the terminal Cancelled state + post-cancel orphan scan | 1 | **NO — see below** |
| **G2** | 1 | **Fix the ssh runner's cleanup on the failure path** so `rm -rf "$root"` runs and `input.bin` is not left on the far end (FINDING 5) | 1 | NO |
| **G3** | 1 | Re-run the ssh leg at a common commit so all four surfaces diff in **one** invocation — needs an sshd target reprovisioned (`WAYLAND_EXEC_SSH_TARGET` verified UNSET on `hetzner-dsm` today); 25-01 used a disposable container, so any lane can rebuild it | 0.5 | NO |
| **G4** | 4 | **Install a real egress policy and prove a policy-level DENY of an attempted outbound request** — the actual missing clause, on both hosts | 1–2 | NO |
| **G5** | 4 | **Re-run the Windows denied-egress leg as a `backend run`, and correct the false ledger entry** asserting `exit=1` for a probe that exited 0 | 0.5 | NO |
| **G6** | 4 | Prove compromised-key refusal **on identity** live through the CLI, not only via body digest + unit test | 1 | NO |
| **G7** | 2 | **Controller-side receipt identity verification** (`backend_key_from` bails) so an auditor can check a node's work from the controller (FINDING 4) | 2 | NO |
| **G8** | 2 | Wire the discarded `node probe` advertisement refresh (`node.rs:395` `let _ = ad;`) (FINDING 7) | 0.5 | NO |
| **G9** | 4 | `setsid` dependency removed so the ssh backend can run a task on a Windows far end; would upgrade the Windows orphan proof from planted to product-leaked (FINDING 6) | 1–2 | NO |
| **G10** | 3 | Install `cargo-generate` on `seandesktop` so `plugin new` runs there; closes the one Windows NOT-RUN | 0.25 | NO |
| **G11** | — | Correct the three overstating ledger/summary lines and retire the four-MET table in `25-PHASE-STATUS.md` | 0.25 | NO |
| **G12** | — | Roadmap owner: does **F25-05** belong to this phase? No plan claims it | — | Sean/roadmap decision |

**Total to move C1 and C4 to MET: ~5–7 lane-sessions. No new credential is required.**

### The cloud-backend credential question, answered directly

**The cloud half is BUILD WORK, not a Sean item.** The Fly credential Sean minted on
2026-07-28 is **still provisioned on the proof host**. Verified today without printing or
transmitting any value, with the instrument proven alive in both directions:

```
$ ssh hetzner-dsm '…'
EXISTS: -rw------- 1 root root 716 Jul 28 03:36 /root/.wayland-f25-cloud.env
grep -c "WAYLAND_F25_CLOUD_TOKEN" …  ->  1      (named)
grep -c "WAYLAND_F25_CLOUD_ORG"   …  ->  1      (named)
grep -c ""                        …  ->  3      (lines)
grep -c "ZZZ_NOT_A_REAL_VAR"      …  ->  0      (known-negative: instrument can return zero)
```

The file exists, is `0600`, and names both required variables. **G1 needs no Sean action** —
unless the token has expired since 2026-07-28, which is a one-command check at the start of
G1 (`backend probe cloud` → `vendor_api_call` vs `credential_absent`) and would then be a
credential refresh, not a new account. App scope remains `wayland-f25-test` inside org
`sean-donahoe`, which is Sean's **personal** org and is *not* disposable — noted so no future
lane asserts org-wide emptiness.

---

## Peer comparison — recorded honestly, not graded

**Hermes ships seven execution backends behind one contract** — `local`, `docker`, `ssh`,
`singularity`, `modal`, `managed_modal`, `daytona`, plus `file_sync`. **Wayland Core ships
four** — `local`, `container`, `ssh`, `cloud` — of which **three ran the same deterministic
task at any single commit**, and four across two commits.

**This is a breadth gap, not a correctness gap, and it does not affect any grade above.**
Phase 25's criteria name four surfaces and Wayland Core built four; the contract
(`crates/wcore-exec-backend`) is provider-neutral with one conformance harness, so additional
backends are additive work against an existing seam rather than new architecture.

Two things Wayland Core's four demonstrably carry that a backend count does not express: a
**signed receipt with Ed25519 attestation** whose `key_id` is the SHA-256 of the pinned
verifying key, and a **hibernation discriminator proven against a stop/start control on one
machine**. Whether Hermes' seven carry equivalent receipt and cleanup semantics is **not
established here** and should not be assumed in either direction — I did not measure Hermes,
and an unmeasured comparison is the failure class this program keeps finding.

One asymmetry that *is* a real gap in our favour to close: our conformance harness reported
**local PASS (15 checks), container PASS (15 checks), ssh and cloud UNEXERCISED with reasons**
(`25-01-SUMMARY.md` §9). Four backends behind one harness is the claim; **two** actually
passed the harness. That is worth stating alongside the 4-vs-7 count, because it means our
effective harness-proven breadth is two, not four.

---

## Cross-audit panel — the two contestable calls

Two grades were genuinely arguable and were put to the panel with the measured facts, the
grading vocabulary, and no indication of my own leaning. Both members converged with my
call, independently and for the criterion-text reason rather than the vibe.

**Question A — Criterion 1: MET-WITH-STATED-EXCEPTIONS or PARTIAL?**

| member | position | reasoning (verbatim, condensed) |
|---|---|---|
| codex `gpt-5.6-sol` | **PARTIAL** | *"Cloud cancellation is entirely unproven, so a required behavior cannot receive an exception-based pass. The reproduced SSH cleanup leak directly violates cleanup and leaves sensitive task inputs behind."* Also flagged the cross-commit composition as not proving four surfaces equivalent in one coherent implementation. |
| gemini `3.1-pro-preview` | **PARTIAL** | *"The complete lack of cancellation testing on the cloud backend means a core dimension of the criterion was never measured, preventing it from reaching the threshold for an exception-based pass."* |

**Question B — Criterion 2: MET-WITH-STATED-EXCEPTIONS or PARTIAL?**

| member | position | reasoning (verbatim, condensed) |
|---|---|---|
| codex `gpt-5.6-sol` | **MET-WITH-STATED-EXCEPTIONS** | *"Controller-side verification is an important architectural limitation, but the criterion requires attribution not be lost — not that the controller perform the audit."* |
| gemini `3.1-pro-preview` | **MET-WITH-STATED-EXCEPTIONS** | *"The literal clauses of the criterion were empirically satisfied... The limitations regarding central controller verification and dead refresh code represent documented defects that do not violate the strictly stated text of the goal."* |

**Basis: unanimous, both questions.** Codex took the LAST emitted block (it repeats its final
answer); gemini was invoked with `--skip-trust` or it returns nothing. Both were probed with
the full fact set, not a one-word question.

**The internal adversarial position, which did not win but is preserved.** Against Criterion 2:
a phase that cannot audit a node's work *from the controller* has not really shipped
distributed authority attribution — it has shipped a receipt stamp that only the producer can
check, which is close to self-certification, and the phase's own ledger says *"a failure here
is the finding, not a harness problem."* The panel's answer is that the criterion's text
governs and its text says *losing*, not *auditing*. I record the dissent because a reader
building on Criterion 2 should know that controller-side audit does not exist (gap **G7**),
and because if the roadmap ever meant the stronger reading, this criterion is PARTIAL and
G7 becomes blocking rather than a 2-session improvement.

---

## Ungradeable

| item | why |
|---|---|
| **F25-05** | Listed in ROADMAP Requirements; claimed by no plan, covered by no Success Criterion. Cannot be graded against a criterion that does not exist. Routed to the roadmap owner. |
| **Windows Job Object reaping** | Explicitly **not claimed** by this phase; the orphan result is an observation and is independent of the escalated `live_future_drop_reaps_descendant_job_tree`. Correctly out of scope. |
| **macOS** | `25-MACOS.md` exists but no Phase 25 criterion names a platform. Not graded; not a deduction. |

---

## Confidence and instrument notes

- **Nothing in this verdict rests on a suite exit status.** Every grade traces to a committed
  capture with an explicit `EXIT:`/`[exit=N]` line or to source read directly from the tree.
  The known-bad instruments therefore do not degrade any grade here: nextest fd-exhaustion
  "flakiness" is irrelevant because no grade rests on a suite run, and the silently-ignored
  `no-tests = "fail"` is irrelevant because the one place a test count is load-bearing
  (Criterion 3's `plugin test`) was read from a capture retaining `0 ignored; 0 filtered out`.
- **One grade does lean on a unit test and is discounted for it:** Criterion 4's
  compromised-key *identity* refusal (Gap 4c). Flagged rather than credited.
- **Every absence claim in this file carries a known-positive in the same invocation**, stated
  inline so a reader can re-run it. The zero-count claims are: cloud cancellation markers
  (control: `F25-SC1-CLOUD-RUN` → 1, `machine` → 3), the Windows egress `backend run`
  (control: six other commands enumerated by the same pattern), and the credential-file var
  names (control: `ZZZ_NOT_A_REAL_VAR` → 0 alongside two → 1).
- All load-bearing reads used `/usr/bin/grep`, `/usr/bin/sed`, `/usr/bin/git`. `rtk` was not
  in the path of any number reported here.

## Fence exposure vs `861d1b1a`

```
$ /usr/bin/git diff --name-only 861d1b1a HEAD
.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-GRADE-NOTES.md
.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-PHASE-VERDICT.md

$ /usr/bin/git diff 861d1b1a HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs | wc -l   -> 0
$ /usr/bin/git diff --name-only 861d1b1a HEAD -- crates .github | wc -l                                  -> 0
$ /usr/bin/git status --porcelain --untracked-files=all | grep '^??'                                     -> (none)
```

**Zero exposure.** No `crates/` change, no `.github/` change, zero bytes in either shared-fence
file. This lane read source to verify two findings and wrote only its own two documents.

---

_Graded 2026-07-29 · lane `grade-25` · first verdict file for Phase 25_
