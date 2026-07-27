---
phase: 25-remote-reach-nodes-plugin-lifecycle
plan: "03"
subsystem: nodes
tags: [f25-03, node, pairing, revocation, attribution, liveness, mixed-version]
status: complete
termination_state: 2
requires:
  - wcore-exec-backend ExecutionBackend contract + receipt attestation (25-01)
provides:
  - wcore-exec-backend::node — pairing, capability, registry, version, attribution
  - attested node identity inside the signed receipt body
  - the `wayland-core node` operator surface (8 verbs)
affects:
  - crates/wcore-exec-backend (new node module; one Option field on ReceiptBody)
  - crates/wcore-cli (one TopCmd variant, one lib module — both additive)
tech-stack:
  added: []
  patterns: [attested-attribution, layer-above-not-subclass, fail-closed-gate, proof-of-possession, live-product-exercise]
key-files:
  created:
    - crates/wcore-exec-backend/src/node/{mod,pairing,capability,registry,version,attribution}.rs
    - crates/wcore-exec-backend/tests/node_contract.rs
    - crates/wcore-cli/src/node.rs
    - .planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-03-NODE-EVIDENCE.md
  modified:
    - crates/wcore-exec-backend/src/{lib,receipt,contract}.rs
    - crates/wcore-exec-backend/src/backends/mod.rs
    - crates/wcore-cli/src/{lib,main}.rs (one additive line each)
decisions:
  - "Node identity is attested INSIDE the signed receipt body; a caller-settable node_name would not be attribution."
  - "SubmissionVerdict has no Rerouted variant — the type cannot express the fallback that breaks attribution."
  - "Exactly three version verdicts; an OlderSupported with an empty reduced set is refused because it reads as silent down-negotiation."
  - "wcore-config/src/config.rs was NOT touched — no criterion needs a [nodes] config section."
metrics:
  tests_added: 42
  new_third_party_crates: 0
  defects_found_live: 2
  panel_members: 0
completed: 2026-07-27
---

# Phase 25 Plan 03: Node/Device Contract — Summary

The node contract landed above the backend contract, node identity is attested inside the
signed receipt, and all six F25-03 properties were driven through the shipped binary with
attribution re-verified after all five disruptions — every one of which held.

**Success Criterion 2 is NOT MET.** Everything the criterion asks for was exercised against
a genuinely separate machine identity, but **not against a second physical host**, because
no SSH trust relationship exists between `hetzner-dsm` and `SeanD@seandesktop` in either
direction and creating one is an authorization grant reserved to Sean.

**Termination state 2: complete with one named gap.**

---

## 1. What landed

`crates/wcore-exec-backend/src/node/` — layered ABOVE the backend contract in the same
crate. A node answers *where* and *by whose authority*; a backend answers *how*. It refers
to backends by name and does not subclass, wrap or re-declare the trait. Nothing here
discovers, schedules or meshes — fleet dispatch is Phase 22's and already exists.

- **`pairing.rs`** — proof of possession over a fresh nonce with a domain separator. Four
  independent refusals, each a real attack: replayed nonce, unparseable key, an identity
  whose `key_id` does not match the presenting key, and a signature that does not verify.
- **`capability.rs`** — advertisements produced by a real probe, never a cache, with a
  staleness check that can say yes AND no, and a leak check that fails loudly.
- **`registry.rs`** — paired-node state. Revocation refuses, retains, and cannot be undone
  by the far end.
- **`version.rs`** — three verdicts, no intersection.
- **`attribution.rs`** — the chain from receipt to pinned node key, re-derivable after any
  disruption.

Plus `wayland-core node` with `identity`, `advertise`, `pair`, `list`, `show`, `probe`,
`revoke`, `submit` and `attribution`.

**Zero new third-party crates.** `Cargo.lock` carries exactly one commit in this lane, the
coordinator's base fix `9a86b287`.

## 2. The three things that were easy to get wrong, and how they were not

**Attribution is attested, not asserted.** Node identity lives inside
`ReceiptBody`, which the backend's Ed25519 attestation signs. Altering it changes
`body_sha256`, breaks the signature, and fails `ExecutionReceipt::verify` exactly as
altering the backend identity does. The cheap alternative — a `node_name: String` beside
the body — would have satisfied every surface and been worthless, because any party that
can produce a receipt can produce any name.

The field is `Option` with `skip_serializing_if`, so a receipt sealed without a node
serializes to the *same bytes* it did before the field existed. Every receipt plan 25-01
produced still verifies, and
`adding_the_node_slot_did_not_invalidate_receipts_that_predate_it` pins that rather than
assuming it.

**Revocation cannot reroute, structurally.** `SubmissionVerdict` is
`Accepted | Refused`. There is no `Rerouted`. A controller that quietly ran the work on a
healthy node would turn every disruption test green while destroying the attribution the
criterion is about, so the type simply cannot express it.

**Mixed versions never intersect capability sets.** An accepted older node NAMES what it
cannot honour, and an `OlderSupported` verdict whose reduced list came out empty is
converted to a refusal — because "reduced by nothing" reads, from the operator's chair,
exactly like silent down-negotiation.

## 3. Live evidence

Detail: `25-03-NODE-EVIDENCE.md`. Ledger: `evidence/25-03-node-ledger.txt` (287-line
transcript).

| Property | Verdict |
|---|---|
| PAIR | PASS |
| ADVERTISE | PASS |
| REVOKE | PASS |
| OFFLINE | PASS |
| RECOVER | PASS |
| ATTRIBUTION | PASS |
| WINDOWS-PEER | **NOT-RUN** — no SSH trust, reserved to Sean |

| Attribution after | Verdict |
|---|---|
| REVOKE / REPAIR / DISCONNECT / RETURN / VERSION-MISMATCH | **HOLDS** (all five) |

Nothing was simulated with a flag:

- The far end is a **genuinely separate machine identity** — its own hostname
  (`f25-farnode-box`), its own filesystem, its **own minted node key**
  (`bfed45d376de…` vs the controller's `d66f9b31b176…`), its own process table and network
  namespace, reached over a real SSH connection with a real key.
- OFFLINE was a real `docker stop`, detected in **0s** via a real connection refusal.
- The version-mismatched node is a **second binary built from this tree** with
  `NODE_CONTRACT_MAJOR = 99`, not a flag or a hand-edited record.
- The advertisement difference is real: the host has a working container backend, the far
  end does not.

And the negative control that makes the HOLDS column mean something: the same receipt
attributed to the *wrong* node exits non-zero with
`receipt attributes work to node 'selfnode' … but the pinned record for 'farnode' carries key …`.

## 4. Gate results

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | clean |
| `cargo clippy -p wcore-exec-backend -p wcore-cli -p wcore-config --all-targets --all-features -- -D warnings` | clean |
| `cargo nextest run -p wcore-exec-backend` | **81 passed**, 1 skipped |
| `cargo nextest run -p wcore-exec-backend --test node_contract` | **18 passed** |
| `cargo build --release --locked -p wcore-cli` (Linux + Windows) | both exit 0 |
| `wayland-core node --help` on the shipped binary | works on `hetzner-dsm` AND `SeanD@seandesktop` |
| Shared-seam diffs | `wcore-cli/src/main.rs` +9 lines, `wcore-cli/src/lib.rs` +4 — additive, contiguous, nothing reordered |
| `wcore-config/src/config.rs` | **0 lines** — see §6 |

## 5. Two defects the live exercise found

Both were **false answers**, and neither could have been caught by a green suite.

1. **`node probe` reported a healthy node as OFFLINE.** It hardcoded the far-end binary
   name instead of using the path recorded at pairing, so any node paired with
   `--remote-bin` — the documented way — probed as offline. And an offline node then
   *refuses work*, so a wrong guess about a path became a refusal to run. `remote_bin` is
   now persisted in the record and used by `probe`.

2. **`node probe` refreshed the advertisement by probing the CONTROLLER's own backends.**
   It described the wrong machine entirely: a far node whose Docker daemon died would have
   kept claiming a container backend forever, because the "refresh" never asked it. It now
   asks the far end through a new `node advertise` verb, and when that call fails it says
   the stored advertisement is stale rather than quietly trusting it.

A third, smaller one, found the same way: **`machine_id` read `unknown-host` on every Linux
node.** `HOSTNAME` is a shell variable, not an exported environment one, so the non-login
SSH shell a controller actually uses saw none of the sources it consulted. The
discriminator meant to tell two similarly-named nodes apart was constant across all of
them. Now falls back to `/etc/hostname`.

## 6. Deviations from the plan

**[Scope — not done] `[nodes]` keys in `crates/wcore-config/src/config.rs`.** The plan lists
this file and its gate allows up to 200 changed lines. Nothing in F25-03 needs it: the node
registry lives beside the live-task registry under the existing
`WAYLAND_EXEC_BACKEND_STATE_DIR` / `wayland_config_dir()` resolution that plan 25-01
established, and no Success Criterion depends on a config section. Adding keys nothing reads
to a 325 KB file that all four parallel phases need edits in would be churn in the single
worst place to create a conflict. **Same call plan 25-01 made, for the same reason**, and
recorded as an explicit gap rather than silently dropped.

**[Rule 2 — missing critical functionality] a `node advertise` verb was added.** Not in the
plan's verb list (`pair`, `list`, `show`, `revoke`, `probe`). It is the far-end half of
`probe`, and without it the refresh described the wrong machine — see defect 2. It adds no
capability beyond the six F25-03 names; it makes one of them true.

**[Rule 3 — blocking] the far end is a container, not `SeanD@seandesktop`.** See §7.

## 7. The gap, stated plainly

**Success Criterion 2 says "two genuinely distinct real hosts". This run used one physical
host.**

Both named hosts are reachable at the network layer — SSH reaches authentication in both
directions — but neither holds a key authorizing the other:

```
hetzner-dsm → seandesktop : Permission denied (publickey,password,keyboard-interactive)
seandesktop → hetzner-dsm : Permission denied (publickey,password)
```

Closing that means writing an SSH public key into `authorized_keys` on one of Sean's
machines, granting one persistent shell access to the other. That is an authorization grant
on infrastructure this lane does not own. **I did not do it, and I am not claiming the leg.**

What that costs, precisely:

- **The cross-OS advertisement is unobserved.** Both nodes in this run are Linux. The two
  machines *do* advertise different capability sets from real probes, so the property is
  exercised — but a Linux/Windows pair is the sharper test and it did not happen.
- **The cross-machine network path is unexercised.** A real SSH connection over a real
  socket to a separate network namespace was used; a WAN hop between two physical machines
  was not.

`25-03-NODE-EVIDENCE.md` §7 carries the exact two commands and the exact expected
observation. Nothing else in this plan is blocked on it.

## 8. Other known gaps

- **In-flight termination is untested against a real remote task.** `terminate_in_flight`
  reads the same live-task registry `backend cancel` uses and was exercised against an
  empty one — the transcript says `no in-flight work was recorded against it` rather than
  claiming a termination that did not happen. Driving a long task *through the node layer*
  needs a node→backend submission path this plan deliberately did not build, because
  building one would be a second fleet dispatcher beside Phase 22's.
- **Node-attributed work is stamped from `WAYLAND_NODE_ID`.** A Core running as a paired
  node sets it; a controller running work locally has no node identity and the receipt
  records `None` rather than a placeholder. That is honest but manual — there is no
  automatic binding from "this Core was paired as X" to "stamp X".
- **No TUI surface**, on either platform.

## 9. Backlog candidates (MEDIUM and below, non-blocking)

- `[MED]` `[nodes]` configuration keys (§6).
- `[MED]` A node→backend submission path so `terminate_in_flight` can be proven against a
  real in-flight remote task, without duplicating Phase 22's dispatcher.
- `[LOW]` `WAYLAND_NODE_ID` could be derived from the local node registry instead of an env
  var once a Core knows which name it was paired under.
- `[LOW]` Cleanup on `hetzner-dsm`: container `f25-03-farnode`, `/root/f25-03-far`,
  `/root/f25-03-oldbuild`, `/root/f25-03-evidence`, the `f25node` / `f25self` blocks in
  `/root/.ssh/config`, `/root/.wayland/exec-backend/nodes.json`.

## 10. Requirements

- **F25-03 — NOT marked complete.** All six properties were exercised and attribution held
  after every disruption, but not against a second physical host.
- **Success Criterion 2 — NOT MET**, for the reason in §7 and no other.

## Self-Check: PASSED

All named files exist in the worktree; all commits are present on `lane/25`.
