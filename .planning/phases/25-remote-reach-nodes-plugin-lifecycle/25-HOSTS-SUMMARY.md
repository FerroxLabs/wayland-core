# Phase 25 — two-real-hosts lane summary

Closes **Criterion 2** and **Criterion 4**, the two the phase status recorded as blocked on
Sean. Neither was blocked.

**Branch:** `lane/25-hosts` · **Base:** `14905684` · **Proved at:** `2da46485`
**Controller:** `hetzner-dsm` — Ubuntu 24.04, machine-id `eded23e3526d4d74a642b0904ed8fc71`
**Node:** `SeanD@seandesktop` — Windows, machine-id `seandesktop`
**Date:** 2026-07-28 · **Ledgers:** `evidence/25h-*.txt`

---

## The correction that unblocked this lane

`25-PHASE-STATUS.md:102` recorded Criteria 2 and 4 as blocked on SSH trust between the two
physical hosts, "Reserved to Sean". **That blocker did not exist.** Re-measured at the start
of this lane, before touching anything:

```
$ ssh hetzner-dsm 'ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop hostname; echo RC=$?'
SeanDesktop
RC=0
```

No credential, no new machine, no Sean action was needed for either criterion. The status
file is corrected at its head (`25-PHASE-STATUS.md`), along with the separate contradiction
where its header claimed "two of four MET" while its own verbatim grading showed only
Criterion 3.

---

## The headline: running it on a second real host found three HIGH defects

None is a crash. Two would have passed silently.

| # | Sev | Defect | Fails loudly? |
|---|---|---|---|
| 1 | **HIGH** | `backend scan --task-id 'x;id>/tmp/w;echo y'` **executed `id` as root on the far end**. `ssh host cmd a b c` carries no argv — the client joins its arguments with spaces and the far end's *login shell* re-parses them. | **No — it "worked"** |
| 2 | HIGH | An **empty task input vanished from the wire**, shifting task argv left, so the base64 input was read as the program name. The ssh backend could not run a task with empty input at all. | Yes — exit 1 |
| 3 | **HIGH** | The orphan sweep reported **`0 (MEASURED)` while an orphan ran** on the Windows far end: msys `ps` rejects `-eo`, stderr went to `/dev/null`, the pipeline ended `\|\| true`. | **No — silent false zero** |

All three are fixed on this branch. Defect 1 is the one that matters most: the module's own
doc opened with *"ARGV MODE ONLY … argv mode never lets a metacharacter reach an
interpreter … there is NO shell-string path in this file at all"*, and its unit test
`the_module_contains_no_shell_string_execution_path` **passed unchanged the entire time**,
because it greps the file's own source and says nothing about ssh's wire behaviour. The
claim was true of the local spawn and false across the connection. The doc now says so.

### Defect 1, proved through the shipped binary, with a negative control

```
$ wayland-core backend scan --task-id 'f25hx;id>/tmp/f25h-PWNED-BY-SCAN;echo done'
  backend    ssh
  count      2 (MEASURED)
  row        392 386 bash -c sh -s -- f25hx;id>/tmp/f25h-PWNED-BY-SCAN;echo done
  row        done
SCAN-EXIT=1

=== did the far end execute it? ===
F25H-SSH-REMOTE-INJECTION: CONFIRMED — the far end executed injected shell
  uid=0(root) gid=0(root) groups=0(root)

=== negative control ===
CONTROL OK — a benign task-id produces no witness, so the witness above
             was caused by the payload and not by the harness
```

Same payload, same command, after the fix: `F25H-SSH-REMOTE-INJECTION: NOT REPRODUCED`,
count `0`, and the spurious `done` row (the injected `echo`, counted as an orphan) is gone.

The fix quotes every value crossing the connection (`posix_quote`). The behavioural test is
`crates/wcore-exec-backend/tests/ssh_far_end_quoting.rs`, which hands the built command
string to a real shell and compares the argv that comes back — and **carries its own
positive control**: the same round-trip *without* quoting must lose the empty value, split
the spaced one, and execute the metacharacter one. If that control ever goes green, the
other test is measuring nothing.

---

## Criterion 4 — "no orphaned execution", the SSH surface

> **Verdict: MET.** Measured on **two** far ends, each checked in **both** directions.

SSH was the last unmeasured surface. It is now measured, and the measurement is
demonstrably capable of returning non-zero.

### Far end A — containerised sshd (full POSIX), `evidence/25h-ssh-orphan-ledger.txt`

| gate | independent enumeration | scanner | exit |
|---|---|---|---|
| negative — a nonce nothing carries | 0 | `0 (MEASURED)` | 0 |
| **positive — a REAL orphan the product left** | 1 row + pid 1170 alive | **`2 (MEASURED)`** | **1** |
| after the reap, same nonce | 0, pid dead | `0 (MEASURED)` | 0 |
| no target configured | — | `NOT MEASURED` | 0 |

**The positive control was not planted.** The remote runner starts work with `setsid`, in
its own remote session — the module's own doc warns that such a child survives a dead
controller. So the controller was killed `-9` mid-task and the remote work was left
genuinely orphaned. The scanner was then asked to find an orphan the product itself had
leaked, and found it with its raw row.

**One thing the naive instrument missed, and it is worth recording.** The independent
`ps | grep <nonce>` read **0 while pid 1170 was alive**, because the orphan is the task's
own argv (`sleep 600`) and carries no nonce. The scanner's `.pid` signal saw it and the
grep did not — so the `ps | grep` enumeration used as the manual check in 25-04 is the
*weaker* instrument on this surface and is not sufficient alone.

### Far end B — the real Windows host, `evidence/25h-win-ssh-orphan-ledger.txt`

Two signals, isolated, because one combined number cannot say which carried it.

| gate | msys `ps -ef` | `Win32_Process` | scanner | exit |
|---|---|---|---|---|
| negative — unused nonce | 0 | 0 | `0 (MEASURED)` | 0 |
| positive A — primary `.pid` signal | pid 2481 alive, reparented to 1 | 0 (argv has no nonce) | **`1 (MEASURED)`** | **1** |
| positive B — secondary `ps` sweep, **before** the fix | 1 | 1 (row shown) | **`0 (MEASURED)`** | 0 |
| positive B — secondary `ps` sweep, **after** the fix | 1 | 1 | **`1 (MEASURED)`** | **1** |

The `before` row is defect 3, caught exactly as the cloud lane's was: a scan that could only
ever report zero, reported zero, and looked clean. Two independent instruments disagreed
with it.

### The NOT MEASURED path is reachable, not dead code

The fix falls back `ps -eo` → `ps -ef` and only reports NOT MEASURED when neither works. A
claim about a branch nobody has run is not evidence, so a far end with **no `ps` at all**
was built (`evidence/25h-nops-control.txt`):

```
ps present? no
ssh count: NOT MEASURED
ssh reason: the surface could not be enumerated (... the far end's process table could not
            be enumerated (no supported ps invocation on this far end), so a count would
            omit an unknown number of processes), so no count exists — this is NOT zero orphans

--- the SAME binary against the POSIX far end must still MEASURE ---
ssh count: 0 (MEASURED)
```

Both directions: the unmeasurable surface refuses to produce a number, and the measurable
one still produces one.

### One limitation of the fix, stated rather than buried

When the sweep cannot run, the surface reports NOT MEASURED and `backend scan` therefore
exits **0** even if the primary signal found an orphan. The found row is carried into the
reason text so nothing is hidden, and the CLI already prints *"an un-enumerated surface is
not a clean surface"* — but the **gate is weaker in that specific case than a measured
non-zero would be.** That is the deliberate trade: a count that omits an unknown number of
processes is not a measurement, and a false zero is invisible where a NOT MEASURED is not.

---

## Criterion 2 — nodes, plural, meaning genuinely distinct machines

> **Verdict: MET on every property the criterion names**, across two real hosts, with one
> limitation recorded below that a strict reader may weigh differently.

25-03 exercised all six properties against a **container on the controller's own machine**.
The unmet clause was *nodes*, plural. Re-run here with `hetzner-dsm` and `SeanD@seandesktop`
as the two hosts, entirely through the shipped binary. `evidence/25h-node-ledger.txt`.

| property | verdict | what was observed |
|---|---|---|
| **PAIR** | PASS | `os windows`, `machine seandesktop`, node key `fa808c4de95c…` — against the controller's own `ffdcb20256a0…` / `ubuntu-2404-noble-amd64-base`. A loopback proof would have shown one key on both sides. |
| **PAIR (negative)** | PASS | An unreachable far end is refused (exit 1) and left **no record**. |
| **ADVERTISE** | PASS | Genuinely different capability sets from real probes: node `local` = `appcontainer` available, `container` unavailable (Docker Desktop pipe absent); controller `local` = `bubblewrap`, `container` available (server 29.2.1). **This is the cross-OS advertisement 25-03 could not make.** |
| **MIXED VERSIONS** | PASS | A **second Windows binary** built from the same tree with `NODE_CONTRACT_MAJOR = 99` — not a flag, not a hand-edited record. Named at `node list` (`99.0 … unsupported`), refused at submission (exit 1), while the supported node **on the same physical host** still accepts (exit 0). |
| **REVOKE** | PASS | Refused with `NOT falling back to another node` (exit 1); the far end could **not re-pair itself** despite presenting a genuine proof (exit 1); only `revoke --clear` plus a deliberate operator pair reopens it. |
| **OFFLINE** | PASS | A **real partition between two physical hosts**: `iptables -I OUTPUT -d 100.109.207.54 -p tcp --dport 22 -j DROP` on the controller. Independent ssh probe timed out; `node probe` read OFFLINE with the reason; work refused, not rerouted. |
| **RECOVER** | PASS | Rule removed, `node probe` reads LIVE, node accepts work again. |
| **ATTRIBUTION** | HOLDS after all five disruptions | REVOKE, REPAIR, DISCONNECT, RETURN, VERSION-MISMATCH — all `attribution HOLDS — work attributed to node 'windesk' (key fa808c4de95c)`. |

The partition was chosen over stopping the node's sshd deliberately: `seandesktop` is shared
and another lane may be using it. Severing the controller's own route is a genuine loss of
reach, reversible, and touches only this path. The rule was verified installed, verified
removed, and `iptables -S OUTPUT` mentions the node IP **0** times at the end.

### Attribution: the control pair, and one honest correction

`evidence/25h-attribution-ledger.txt`, `evidence/25h-attribution-negative.txt`.

**My first attribution run was void and is discarded.** It minted the node's receipt under
`WAYLAND_EXEC_BACKEND_STATE_DIR=C:\ferrox-25h\state` while the *pairing* had used the node's
default state dir. Two state dirs mean two node seeds, so every verdict came back BROKEN on
a key mismatch **my harness had created**. That was my error, not the product's, and the
whole table was re-run from the paired identity.

**The second negative control was also wrong and was replaced.** It minted the bad receipt
in a fresh state dir, which gave it a fresh *backend* key too, so attribution refused at the
backend-key step and never reached the node comparison — red for the wrong reason. The
replacement copies the node's real state dir and deletes **only** `keys/node.key`, so the
backend key is byte-identical and the node identity is the single variable:

```
good receipt: backend d01d8180bbac  node fa808c4de95c
alt  receipt: backend d01d8180bbac  node 8462f515c20e
SETUP OK — the backend key is IDENTICAL and only the node key differs

POSITIVE: attribution HOLDS — work attributed to node 'windesk' (key fa808c4de95c)   EXIT=0
NEGATIVE: attribution BROKEN — receipt attributes work to node 'windesk'
          (key 8462f515c20e) but the pinned record for 'windesk' carries
          key fa808c4de95c                                                           EXIT=1
```

Attribution discriminates on **node identity**, and can produce both answers. A HOLDS is
therefore worth something.

### The limitation, and why a strict reader might grade Criterion 2 differently

**The controller cannot verify the node's receipt.** This is what an auditor would actually
do — take the node's receipt to the controller that pinned the node — and it fails:

```
$ wayland-core node attribution windesk --receipt <receipt minted on the node>
wayland-core node: this host does not hold the signing key for backend 'local'
(receipt key d01d8180bbac vs local cd83b86781cd), so the receipt's IDENTITY cannot
be verified here — only its integrity, which is not the same claim                [exit=1]
```

The message is honest — it refuses rather than pretending integrity is identity — but the
consequence is that **attribution can only be verified on the machine that produced the
work.** Every HOLDS above was obtained on the node, against the controller's pinned record
copied there.

Two readings, and the evidence supports both:

- **Criterion 2 as written** — "*without losing authority attribution*" — is **MET**:
  attribution is not lost, survives all five disruptions, and is cryptographically
  checkable against the controller's pinned record.
- A reader who takes "authority attribution" to mean *the controller can audit the node's
  work* should read this as **NOT MET**, because that operation does not exist in the
  shipped CLI.

I am recording the fact and both readings rather than picking the flattering one. It is
filed as FINDING 4 below, and it was **only discoverable with two real hosts** — which is
exactly the argument for the criterion demanding plural nodes.

---

## What ran where

| host | role | what it did |
|---|---|---|
| `hetzner-dsm` (Linux) | controller | built `wayland-core` at `2da46485`; ran every `node` and `backend scan` command; hosted the containerised sshd far end and the no-`ps` control |
| `SeanD@seandesktop` (Windows) | node / ssh far end | ran `node identity` for pairing; minted receipts; hosted the planted orphans; built the `NODE_CONTRACT_MAJOR = 99` binary |
| container `f25h-sshd` (Debian) | ssh far end | the full-POSIX orphan control pair |
| container `f25h-nops` (Debian, `ps` removed) | ssh far end | the NOT MEASURED control |

Binaries, named so a reader can tell which build produced which line:

- controller: `wayland-core 0.12.25`, commit `2da46485`
- node: `C:\ferrox-25h\wayland-core.exe`, sha256 `96E29E52…`
- node, mixed-version: `C:\ferrox-25h\wayland-core-v99.exe`, sha256 `1FDEC259…`

**A qualification on the node binaries.** Both were built on Windows from commit
`9b2ed829`, not from this lane's base `14905684`, because the Windows clone did not carry
the lane branch. That is defensible **only** because the crates involved are byte-identical
between the two commits, which was checked rather than assumed:

```
$ git diff --stat 9b2ed829 14905684 -- crates/wcore-exec-backend \
      crates/wcore-cli/src/node.rs crates/wcore-cli/src/backend.rs
(empty)
```

The node binary therefore predates this lane's ssh fixes. That does not affect Criterion 2
— the node only ever runs `node identity` and `backend run --backend local`, neither of
which touches the ssh backend — but it means the **node side** of the Criterion 4 runs
exercised the *controller's* fixed ssh code against an unmodified far end, which is the
correct direction: the far end runs only a constant shell script sent over stdin.

---

## Suite results

At `2da46485` on `hetzner-dsm`:

```
cargo nextest run -p wcore-exec-backend  ->  123 tests run: 123 passed (1 leaky), 1 skipped
cargo clippy -p wcore-exec-backend --all-targets --all-features -- -D warnings  ->  clean
cargo fmt --all -- --check  ->  clean (run on the Mac)
```

The `1 leaky` is `fail_closed_matrix::the_local_scan_finds_an_orphan_that_no_registry_remembers`
— a pre-existing test that deliberately leaves a process for the scanner to find.

**One pre-existing test was updated, not weakened.**
`the_remote_scan_and_kill_exclude_their_own_process` asserted the literal
`'$1 != s && $2 != s'`. The `ps -ef` fallback moves the pid/ppid columns, so the assertion
now checks `$p != s && $q != s` **and additionally** that the column variables are set for
both readers — a stricter check than before, because a self-exclusion comparing the wrong
field silently stops working.

---

## Findings

| # | Sev | State | Finding |
|---|---|---|---|
| 1 | **HIGH** | FIXED | Remote command injection: task/CLI values crossing ssh were re-parsed by the far end's login shell. `backend scan --task-id '…;id>…'` ran `id` as root on the far end. |
| 2 | HIGH | FIXED | An empty value vanished from ssh's remote command string, shifting task argv left. The ssh backend could not run a task with empty input. |
| 3 | **HIGH** | FIXED | The ssh orphan sweep reported `0 (MEASURED)` on a far end whose `ps` rejects `-eo` — a structural false zero, invisible because stderr was discarded and the pipeline ended `\|\| true`. |
| 4 | MEDIUM | **BACKLOG** | The controller cannot verify a node-minted receipt's identity (`backend_key_from` bails), so attribution can only be audited on the machine that did the work. See the Criterion 2 limitation above. |
| 5 | MEDIUM | **BACKLOG** | The ssh remote runner leaves its task root behind on failure. `set -e` aborts at `wait`, so `rm -rf "$root"` never runs and `input.bin` — the task's input bytes — is left on the far end. Six such roots were found on the node and purged. Touches Criterion 1's "cleanup", not Criterion 4. |
| 6 | MEDIUM | **BACKLOG** | The ssh backend cannot run **any** task on a Windows far end: the remote runner requires `setsid`, which Git-for-Windows does not ship. It fails loudly (exit 1), so this is a capability gap, not a false green. |
| 7 | LOW | observation | `node probe` prints `advertised before:` and `advertised now:` but never refreshes the record — the controller-side `NodeAdvertisement::observe` result is computed and discarded (`let _ = ad;`), and the far end's `node identity --advertise` answer is parsed as an identity. The two lines are therefore always identical, while the module doc says the refresh "is the point". Not exercised by Criterion 2, which reads the advertisement at pairing. |
| 8 | LOW | observation | `crates/wcore-cli/src/node.rs::far_end_call` has the same unquoted-argv shape as finding 1. It is **not** exploitable — every attacker-reachable value is a validated identifier or a locally generated hex nonce — but a legitimate `--remote-bin` containing a space (e.g. `C:\Program Files\…`) would break. Not fixed here: the node far end may be PowerShell, so POSIX quoting is the wrong fix and would have broken the very leg this lane was proving. |

---

## Cleanup, proven rather than asserted

`evidence/25h-cleanup.txt`, plus a final independent re-check:

| resource | final state |
|---|---|
| containers `f25h-sshd`, `f25h-nops` | **0** remaining; images removed |
| container `f25-03-farnode` (left running by lane 25-03 since 2026-07-27) | **0** — removed, as its own SUMMARY §8 sanctioned |
| iptables partition rule | absent; `iptables -S OUTPUT` mentions the node IP **0** times |
| `/root/f25h` (lane ssh key + config) | removed |
| node task roots `/tmp/wayland-f25-*` | **0** (6 found and purged) |
| node processes carrying `f25hwin-` | **0** (`Win32_Process`, self-ancestry excluded) |
| scheduled tasks `ferrox25h*` | **0** — deleted |
| other lanes' scheduled tasks | `ferroxP21A` **untouched** |
| node `nodes.json` this lane wrote | absent, as found; no backup file left |

**Deliberately retained:** `C:\ferrox-25h` on the node (2.9 GB — the two binaries and the
build clone), so this evidence is re-runnable. It is a directory this lane created.
`C:\ferrox-win` (lane 26's tree) was **only read**, via `git clone`, never written.

---

## Defects in my own harness, listed because they are the same class

Four self-passing or invalid gates were written and caught during this lane. Each is the
class this program keeps finding, and each would have produced a false PASS:

1. **Two gates compared against an empty string.** `node identity` takes `--name`, not a
   positional, and `backend list --json` keys the id as `backend`, not `backend_id`. Both
   extractors returned empty, so `[ "$CTRL_KEY" != "$NODE_KEY" ]` passed for free. Both now
   report **NOT MEASURED** when an input is empty rather than passing.
2. **A verdict line claimed a false zero when nothing had been planted.** The argv-mode
   plant used `sh -c 'sleep 900 # nonce'`, which execs `sleep` and drops the nonce from the
   process table. Scanner 0 and independent 0 was reported as "the scanner missed it". Now
   reports NOT MEASURED when the plant produced nothing.
3. **A `Win32_Process` instrument matched its own ancestry** — the nonce travels on the
   query's command line and on the ssh session's. Now excludes the whole ancestor chain,
   and self-tests that it can see its own command line (a NULL command line without
   privilege would reproduce the false zero with a different tool).
4. **A far-end cleanup killed itself.** `pkill -f <nonce>` run on the far end matches its
   own command line, so it died before the following `rm -rf`. Now kills the recorded
   session leader by pid.

Plus the two attribution-run corrections described above.

---

## What this lane did NOT do

- Did **not** fix finding 4 (controller-side attribution), 5 (task-root residue), 6
  (`setsid` on Windows), 7 (`node probe` advertisement refresh) or 8 (`node.rs` quoting).
  All are MEDIUM or below and go to BACKLOG per the severity policy.
- Did **not** build the node binaries from this lane's base commit — see the qualification
  above, with the byte-identity check that makes it defensible.
- Did **not** touch Criteria 1 or 3, or any other lane's files.
- Did **not** re-run the container or cloud orphan surfaces beyond a regression check;
  those belong to 25-04 and `lane/25-cloud`.
- Did **not** exercise the ssh backend end-to-end against the Windows far end, because
  `setsid` is absent there (finding 6). The Windows Criterion 4 measurement used planted
  orphans in the exact shape the runner leaves, with both signals isolated.
