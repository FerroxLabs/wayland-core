# 25-03 — Node contract: live evidence

Full unedited capture: `evidence/25-03-node-linux.log` (287 lines). Machine-parseable
verdicts: `evidence/25-03-node-ledger.txt`. Per-leg captures: `evidence/25-03-*.txt`.

- **Controller:** `hetzner-dsm` (Linux), 2026-07-27
- **Binary:** `/root/wayland-25/target/release/wayland-core` — `wayland-core 0.12.25`
- **Commit:** `c336576b` (plus the probe fix `c913f3cc`, re-run)
- **Driver:** `/root/f25-03-live.sh`, one continuous run

---

## 1. What the far end actually was — stated before the results, not after

The plan asked for two genuinely distinct hosts, naming `hetzner-dsm` and
`SeanD@seandesktop`. **That pairing could not be run, and the reason is not a technical
one.** Both hosts are reachable at the network layer — the TCP connection completes and
SSH reaches authentication — but there is **no SSH trust relationship in either
direction**:

```
hetzner-dsm → seandesktop : SeanD@seandesktop: Permission denied (publickey,password,keyboard-interactive)
seandesktop → hetzner-dsm : root@<hetzner>: Permission denied (publickey,password)
```

Creating one means adding an SSH public key to `authorized_keys` on one of Sean's
machines, which grants that machine persistent shell access to the other. That is an
authorization grant on infrastructure this lane does not own, and it sits squarely in the
reserved set. **I did not do it**, and the ledger records
`F25-SC2-WINDOWS-PEER: NOT-RUN` with that exact reason.

What was used instead is a **genuinely separate machine identity**, and the difference
from a loopback is concrete:

| property | controller (`selfnode`) | far end (`farnode`) |
|---|---|---|
| `machine_id` | `ubuntu-2404-noble-amd64-base` | `f25-farnode-box` |
| node `key_id` | `d66f9b31b176…` | `bfed45d376de…` |
| filesystem | host root | its own |
| process table / netns | host | its own |
| reached by | — | a real SSH connection with a real key, over a real socket |
| can be stopped | no (it is this host) | **yes — and it was** |

It is a **container**, not a separate physical host. Its node key was minted on its own
filesystem, so the identity is genuinely distinct rather than the controller's key wearing
a second name — which is exactly the failure a loopback proof would have. What it does
NOT prove is the cross-machine, cross-OS case. That shortfall is named here and repeated
in the SUMMARY rather than glossed.

---

## 2. The six F25-03 properties

| Property | Verdict | What was observed |
|---|---|---|
| **PAIR** | PASS | The far end proved possession of a key that is verifiably **not** the controller's, before any record was written. |
| **ADVERTISE** | PASS | The two machines advertise **different** capability sets from real probes: the host has a working container backend, the far end does not (no Docker inside it). Not a hardcoded list. |
| **REVOKE** | PASS | Work to the revoked node exits non-zero with `NOT falling back to another node`; the healthy node did not absorb it. |
| **OFFLINE** | PASS | The far end was genuinely **stopped** (`docker stop`), detected in **0s**, and refuses work. |
| **RECOVER** | PASS | The far end came back, reads `LIVE` again, and takes work again. |
| **ATTRIBUTION** | PASS | The chain from receipt to pinned node key verifies — and **breaks when it should**. |

## 3. Attribution, re-asked after every disruption

| After | Verdict |
|---|---|
| REVOKE | **HOLDS** |
| REPAIR | **HOLDS** |
| DISCONNECT | **HOLDS** |
| RETURN | **HOLDS** |
| VERSION-MISMATCH | **HOLDS** |

None broken. That is the criterion's real content: revoking a node withdraws its authority
for FUTURE work while leaving work it already did fully attributable, which is what an
audit needs.

### The negative control, without which the row above proves nothing

The SAME receipt, attributed to the WRONG node:

```
$ wayland-core node attribution farnode --receipt receipt-selfnode.json
attribution BROKEN — receipt attributes work to node 'selfnode' (key d66f9b31b176)
                     but the pinned record for 'farnode' carries key bfed45d376de
[exit=1]
```

Attribution can go red, and does. Capture: `evidence/25-03-attr-wrong-node.txt`.

---

## 4. Selected transcript

### Pairing refuses an unprovable far end and leaves nothing

```
$ wayland-core node pair ghostnode --target root@203.0.113.253 --remote-bin ...
wayland-core node: reaching node 'ghostnode' ...: far end did not answer within 25s
[exit=1]
$ wayland-core node list        # ghostnode is ABSENT
```

### Revocation refuses and does not reroute

```
$ wayland-core node revoke farnode --reason "operator withdrew authority ..."
REVOKED 'farnode' (...)
  subsequent work to it will be REFUSED and will NOT be rerouted
  the record is retained; the far end cannot re-pair itself

$ wayland-core node submit farnode
wayland-core node: REFUSED: node 'farnode' is REVOKED (...); refusing to run and
NOT falling back to another node (node 'farnode')                        [exit=1]

$ wayland-core node submit selfnode
ACCEPTED: node 'selfnode' may take work                                   [exit=0]
```

The healthy node is still healthy — and it did not take farnode's work. `SubmissionVerdict`
has no `Rerouted` variant, so the type cannot express the fallback.

A revoked far end presenting a **genuine** proof still cannot re-pair itself; only
`node revoke --clear` followed by a deliberate operator `node pair` reopens it.

### A stopped far end, detected and bounded

```
STATE: stopping the far end for real: docker stop f25-03-farnode
STATE: far end still listening on 2222? 0

$ wayland-core node probe farnode
node 'farnode' is OFFLINE: far end exited 255: ssh: connect to host 127.0.0.1 port 2222: Connection refused
  no in-flight work was recorded against it
  work submitted to it will now be REFUSED, not rerouted

STATE: the probe returned in 0s — bounded, not hung
```

`node list` then shows `offline (…Connection refused)` for `farnode` while `selfnode` reads
`unknown (not probed since pairing)` — the two are deliberately distinct states, because
"we have not looked" and "we looked and it was gone" are different facts.

### A genuinely different build, refused rather than negotiated down

The mismatched node is not a flag or a hand-edited record: it is a **second binary built
from the same tree with `NODE_CONTRACT_MAJOR = 99`**, running on the far end.

```
$ wayland-core node pair oldnode --target f25node --remote-bin /usr/local/bin/wayland-core-old
paired 'oldnode'
  verdict   unsupported (node 99.0: major version 99 is not 1; the node contract changed incompatibly)

$ wayland-core node list
oldnode  linux  paired  99.0  ...  unsupported (node 99.0: major version 99 is not 1; ...)

$ wayland-core node submit oldnode
wayland-core node: REFUSED: node 'oldnode' advertises an unsupported contract version: ...  [exit=1]
```

Named at `node list`, refused at submission, and no capability intersection anywhere.

---

## 5. Unexercised, with reasons

- **The Windows peer.** `F25-SC2-WINDOWS-PEER: NOT-RUN`. No SSH trust in either direction;
  establishing one is reserved. See §7 for the exact command.
- **A cross-OS advertisement.** Follows from the above: both nodes in this run are Linux.
  The contract test `two_nodes_on_different_operating_systems_advertise_differently` covers
  the shape; the live cross-OS observation does not exist.
- **A separate physical host.** The far end is a container on the controller's own machine.
- **In-flight termination on a real remote task.** `terminate_in_flight` reads the live-task
  registry and was exercised against an empty one — the transcript says
  `no in-flight work was recorded against it` rather than claiming a termination that did
  not happen. Driving a long remote task through the node layer needs the node→backend
  submission path that this plan deliberately did not build (that is Phase 22's fleet
  dispatch). Recorded as a gap.
- **No TUI.** Nothing in this surface reaches the TUI.

## 6. Test results

- `cargo nextest run -p wcore-exec-backend` — **81 passed**, 1 skipped (18 in
  `node_contract`, 24 node unit tests, plus the pre-existing 25-01 suite).
- `cargo clippy -p wcore-exec-backend -p wcore-cli -p wcore-config --all-targets
  --all-features -- -D warnings` — clean.
- `wayland-core node --help` runs on the shipped release binary on `hetzner-dsm` **and** on
  `SeanD@seandesktop` (built `--release --locked`, exit 0, all eight verbs listed).

## 7. What Sean should run to close the Windows leg

One of these two, whichever direction he prefers. Nothing else in this plan is blocked.

```bash
# Option A — let hetzner-dsm act as controller for the Windows node:
#   append hetzner's public key to the Windows authorized_keys
ssh hetzner-dsm 'cat ~/.ssh/id_ed25519.pub'
#   → paste into C:\Users\SeanD\.ssh\authorized_keys on seandesktop

# then, on hetzner-dsm:
export PATH=/root/.cargo/bin:$PATH
/root/wayland-25/target/release/wayland-core node pair winnode \
  --target SeanD@seandesktop \
  --remote-bin 'C:\ferrox-win\target\release\wayland-core.exe'
/root/wayland-25/target/release/wayland-core node show winnode
```

The expected observation: `winnode` pairs with `os windows`, a `machine_id` distinct from
the Linux host's, and an advertised backend set that differs from the Linux node's — which
is the cross-OS advertisement this run could not make.

## 8. Cleanup

Named for this plan and safe to remove on `hetzner-dsm`: container `f25-03-farnode`,
`/root/f25-03-far`, `/root/f25-03-oldbuild`, `/root/f25-03-evidence`, the `f25node` and
`f25self` blocks in `/root/.ssh/config`, and `/root/.wayland/exec-backend/nodes.json`.
