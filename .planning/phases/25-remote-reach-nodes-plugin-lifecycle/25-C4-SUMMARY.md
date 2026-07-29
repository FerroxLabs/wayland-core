# Phase 25 Criterion 4 — lane `25-c4-egress` SUMMARY

> *"Compromised keys/plugins/backends and denied secret/**egress** paths fail closed with no
> orphaned execution."*

Graded **PARTIAL** on 2026-07-29. Lane branch `lane/25-c4-egress`, based on `lane/grade-25`,
**merged onto integration `gh/plan/f20-unified-audit-repair` @ `4a872413`** mid-lane after an
orchestrator correction, with every result re-proven on the merged tree.

**Verdict: Criterion 4 is closer, and one of its two halves changed character entirely.
Still PARTIAL on Windows. Half A on Linux is now MET on all four classes.**

---

## The headline: the missing clause was a missing WIRE, not a missing TEST

The verdict recorded that "no egress policy is installed on either proof host" and costed
G4 as *install a policy and prove a deny*. Reading source first showed why no policy was
installed:

**`wayland-core backend` never armed the egress boundary at all.**

`TopCmd::Backend` returns an `ExitCode` directly from `main`'s dispatch (integration
`main.rs:1454`) — **442 lines before** `main`'s own `install_egress_policy` chokepoint
(`main.rs:1896`). That chokepoint's own comment lists the subcommands that early-return past
it — `acp`, `swarm`, `workflow`, `agent` — and each of those self-installs. **`backend` is a
fifth and was on nobody's list.** So the cloud backend's three
`wcore_egress::EgressClient::new()` sites ran under `GlobalDefaultPolicy`, which returns
`Allow` when nothing is installed.

The product had been disclosing this the whole time. Its receipts said, verbatim:

```json
"egress_decision": "allow-all-default-no-policy-installed"
```

Fixed (`crates/wcore-cli/src/backend.rs`, `arm_egress_policy()`), mirroring the fix `acp.rs`
and `workflow.rs` already carry. Proven by the product's own receipt, same command, only the
binary differing:

| binary | `egress_decision` |
|---|---|
| base | `allow-all-default-no-policy-installed` |
| fixed | `shared-egress-policy-installed` |

---

## The four fail-closed classes, each measured separately

| # | class | Linux | Windows | orphans after denial |
|---|---|---|---|---|
| 1 | **compromised keys** | **CLOSED — live, on IDENTITY, via the CLI** | not run (no build of the fix) | 0 |
| 2 | **compromised plugins** | already covered by 25-02 (tampered + unapproved refused) | already covered by 25-02 | 0 |
| 3 | **compromised backends** | already covered (attestation mismatch, exit 1) | already covered | 0 |
| 4 | **denied secret** | covered (`CredentialAbsent`, exit 1, no receipt) | **now covered by a real `backend run`, exit 1, no receipt** | 0 |
| 5 | **denied EGRESS** | **CLOSED — first real policy-level DENY in the phase** | **STILL OPEN** | 0 |

**Orphan count after every denial in this lane: 0**, measured with a clean instrument (see
the false-positive note below). No process survived any refusal I induced.

### Class 5 — the egress deny (G4), Linux

Two runs. Credential present and identical in both. Command identical in both. **One
variable**: a single key in an isolated `XDG_CONFIG_HOME` config.

| arm | `egress_allow` | product's own posture line | cloud row |
|---|---|---|---|
| **allow** | `["api.machines.dev"]` | `ENFORCING … allowlisted=38` | `machine listing returned HTTP 404: {"error":"app not found"}` |
| **deny** | `[]` | `ENFORCING … allowlisted=36` | `egress denied: GET with a long or high-entropy path/query to a non-allowlisted host. Egress to \`api.machines.dev\` is blocked by the security policy.` |

**The positive direction is proven first and cannot be faked: in the allow arm the request
physically left the machine and Fly's own servers answered HTTP 404.** A vendor response is
not producible by a broken build, an absent policy, or a no-op. That is exactly the property
the old evidence lacked — `CredentialAbsent` fires before any socket opens, so the previous
"denied-egress" case **would have passed against a build with `wcore-egress` deleted**.

Evidence: `evidence/25-c4/25-c4-egress-2x2.txt` + raw captures.

### Class 1 — compromised key on IDENTITY (G6)

`backend receipt verify` is integrity-only by design and says so, which left the key clause
resting on a body digest and a unit test. Added `--against-backend <name>`: the caller names
the backend whose **live** verifying key the receipt must verify against. Three-way live
proof through the shipped CLI:

| # | case | result |
|---|---|---|
| 1 | local receipt vs **local** key | `IDENTITY: OK`, exit **0** ← the check can pass, so 2 is not vacuous |
| 2 | container receipt vs **local** key | `INTEGRITY: OK` **and** `IDENTITY: REFUSED`, exit **1** |
| 3 | no flag | integrity-only, exit **0** — unchanged |

Case 2 is the point: **an untampered receipt with an intact body digest, refused purely on
signer identity.** Distinct key ids (`734fdf…` local vs `9c4b6d…` container) and stable
across processes (measured), so a pinned-key check is meaningful rather than always-red.

Evidence: `evidence/25-c4/25-c4-identity-proof.txt`.

### Class 4 / G5 — the false Windows ledger line, corrected

```
ledger:  F25-SC4-CASE-DENIED-EGRESS: REFUSED … exit=1
capture: COMMAND: wayland-core.exe backend probe cloud … EXIT: 0
```

Reproduced on `seandesktop` with the **same binary the original evidence used**, exit codes
read from a status file because non-zero collapses to `1` over ssh+PowerShell:

```
WLRC_PROBE=0        ← backend probe cloud       (ledger claimed exit=1)
WLRC_RUN=1          ← backend run --backend cloud   (leg never previously run)
WLDONE                receipt written: False
```

The `backend run` leg does fail closed — exit 1, no receipt — **but on credential absence,
the same mechanism as denied-secret.** So the ledger's exit code is corrected and the run leg
now exists, but **Gap 4a stands open on Windows.** I also found the rotated-key ledger line
still carrying the label the Linux leg had corrected — the same §6b-ii recurrence, second
instance in one file. Correction record (original left intact):
`evidence/25-c4/25-c4-WINDOWS-LEDGER-CORRECTION.md`.

---

## Second HIGH finding: a documented safety interlock that does not exist

Three places document a **two-key** requirement for disabling the egress boundary —
`[security] enabled = false` **plus** an explicit `--i-accept-exfil-risk` CLI flag —
including a merge comment asserting *"so the merge can't silently disable the boundary."*

Only one key exists. Established from the product, not from an absence grep:

```
$ wayland-core --i-accept-exfil-risk --help
error: unexpected argument '--i-accept-exfil-risk' found
```

`policy_from_config` branches on `config.security.enabled` alone. **A config file alone
silently disables egress enforcement process-wide.** This is not hypothetical: hetzner's
`/root/.config/wayland-core/config.toml` carries `[security] enabled = false`, which is the
actual reason the phase found no policy installed on its proof host.

**Not fixed** — adding a required CLI flag changes behaviour for every existing user and is
an owner's decision, not a lane's. Reported with the evidence.

---

## Three false findings I caught in my own instruments

Filed prominently because the lane brief is right that a false security finding is the most
expensive error available here.

1. **My harness filed a false orphan.** Both 2×2 arms reported `local found=1`. There was no
   orphan — my ssh command line contained the nonce, so the process-table scanner matched
   **my own shell**. My first control was *also* contaminated (nonce still in the outer ssh
   command; measured 2 argv occurrences). Only a nonce generated **on** hetzner
   (`openssl rand -hex 16`, 0 argv occurrences) gave the true answer: **`found=0`**.
   *Harness rule for anyone re-running Phase 25 orphan scans: never put the nonce on a
   command line.*
2. **My first secret sweep was vacuous.** The burn-key file line is `export FLUX_API_KEY=…`;
   my regex anchored `^FLUX_API_KEY=`, the needle came back **empty**, and `grep -F ""`
   matched everything — reporting **4 false hits** while the liveness control *also* passed,
   because an empty pattern matches anything. Repaired with a non-empty assertion plus a
   known-negative.
3. **I nearly reported a fail-open that isn't one.** Under egress denial the scan reports
   `found=0, exit 0` — but the product refuses the laundering explicitly: *"An un-enumerated
   surface is not a clean surface — a scan that could not run must never be read as zero
   orphans,"* with `enumerated=false` carried per row. Correct behaviour. Near-miss, not a
   finding.

Plus one instrument **repaired, not just noted** (§6b-ii): my build poll was
`pgrep -f "cargo build -p wcore-cli"`, which matched **its own `bash -c` wrapper** and
reported `BUILDING` for 12 polls after the build had finished. Replaced with a completion
marker. Three-assertion self-test, including the one that matters: *the old matcher would
have missed it* — measured, not argued.

### Secret sweep

| needle | len | liveness (planted) | known-neg (reversed) | artifacts | tracked diff |
|---|---|---|---|---|---|
| `FLUX_API_KEY` burn key | 51 | 1 ✅ | 0 ✅ | **0** | **0** |
| cloud credential (2 values) | 641, 12 | 2 ✅ | 0 ✅ | **0** | — |

**Burn-key hit count: 0**, with a sweep proven able to find a match in the same invocation.
Neither value was printed, echoed, committed or transmitted; the cloud credential was
consumed by sourcing the existing 0600 file on the host that already held it and never
crossed the wire.

---

## Gates

- **Fence exposure vs merge-base `4a872413`: 0 lines** in `wcore-cli/src/{lib,main}.rs`.
  My entire diff is **one file**, `crates/wcore-cli/src/backend.rs`. 0 untracked files.
- **Regression:** 57 binaries, **2355 passed, 1 failed, 19 ignored**. The failure
  (`registry::tests::…`) is in a crate I did not touch and passes in isolation at the same
  commit (`1 passed; 87 filtered out`) — documented shared-`/tmp` contention on hetzner.
- `cargo fmt --all -- --check` clean.
- Counts read with `/usr/bin/grep`, never the `rtk`-proxied `cargo`.

---

## What I did NOT do

- **No egress-policy denial on Windows.** Needs a Windows build of the fix (every
  `wayland-core.exe` on `seandesktop` predates it) **and** a cloud credential there (the F25
  credential exists only on hetzner; the two hosts cannot reach each other). Neither is
  Sean-reserved. ~1 lane-session.
- **Did not implement `--i-accept-exfil-risk`.** Owner's decision.
- **Did not create Fly infrastructure** to reach the machine-create POST. The exfil-class
  request I denied is the orphan-scan GET, whose nonce query trips the product's real
  `get_carries_data` rule — a genuine operator input, not a contrivance. I say plainly that
  the POST path remains un-denied.
- **Did not touch cancellation or ssh cleanup** (lane `25-c1-cleanup`). I read the orphan
  scanner but modified no shared cleanup code.
- **Did not** merge, open a PR, tag, release, close an issue, or run `wcore-contract generate`.
- Left the hetzner worktree `/root/wayland-25c4` and its `target/` in place (652G free) so a
  follow-up lane can reproduce; it should be removed when Criterion 4 is finished.

## Honest grade

**Criterion 4 remains PARTIAL, but for a smaller reason than before.** Half B (no orphaned
execution) was already MET and my work adds 0 survivors across every new denial. Half A now
covers keys **on identity**, plugins, backends and denied-secret on Linux, and — for the
first time in this phase — a **real policy-level egress denial with a vendor-answered
positive control**. The remaining gap is narrow and precisely stated: **egress denial on
Windows**, blocked only by a build and a credential placement, plus an owner decision on the
missing `--i-accept-exfil-risk` interlock.
