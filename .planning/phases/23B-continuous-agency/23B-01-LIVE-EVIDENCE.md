# 23B-01 LIVE EVIDENCE — F23-02, Success Criterion 2 operator verbs

**Commit under test:** `15971d1b9f2fad79a87f89939e3c2d7e60558f9a` (base) plus this
worktree's three commits `c81eabd5`, `a875a8fc`, `30153232`.

**Binary provenance caveat, stated up front.** The Linux binary was built on
`hetzner-dsm:/root/wayland-p23b`, a worktree detached at the base commit with this
phase's sources rsynced over it. `build.rs` embeds `git rev-parse HEAD`, so
`--build-info` reports the BASE sha, not a sha containing the work. The driver's
provenance assertion therefore proves "this binary was built from a tree at the base
commit", not "this binary contains exactly these commits". That is weaker than the
plan asked for. The reason is structural: this phase runs in an isolated worktree and
is forbidden to push, so no host can fetch a sha that contains the work. Recorded
here rather than papered over.

---

## 1. Linux — `hetzner-dsm`, run nonce `446156892e72cf2a` (and prior confirming runs
`c7ac3ec01c882827`, `e7ee1a9bb0aaf5d8`)

Driver: `scripts/f23-session-operator-drive.sh`. Full captured log:
`evidence/23B-01-linux-drive.log`.

| Verb | Invocation | Observed stdout token | Exit | On-disk consequence | Verdict |
|---|---|---|---|---|---|
| list | `session --dir D list` | `F23_SESSION=list id=aaaa<nonce>` | 0 | — | PASS |
| search (hit) | `session --dir D search aardvark` | `F23_SESSION=search id=aaaa<nonce>` | 0 | — | PASS |
| search (miss) | `session --dir D search zzz-absent-<nonce>` | `F23_SESSION=search_total … count=0` | 0 | — | PASS |
| show | `session --dir D show <id>` | `F23_SESSION=show id=<id> … interrupted=N` | 0 | — | PASS |
| checkpoint | `session --workspace W checkpoint f1 f2` | `F23_SESSION=checkpoint id=<cp>` | 0 | blobs + `meta.json` written | PASS |
| rewind | `session --workspace W rewind <cp>` | `F23_SESSION=rewind id=<cp> restored=true` | 0 | `REWIND_BYTES_EQUAL=true`; `REWIND_LATER_FILE_REMOVED=true` | PASS |
| fork | `session --dir D fork <id>` | `F23_SESSION=fork … parent_unchanged=true` | 0 | `FORK_PARENT_BYTES_EQUAL=true` (cksum before vs after) | PASS |
| show (lineage) | `session --dir D show <child>` | `parent=<parent id>` | 0 | — | PASS |
| retry | `session --dir D retry <id> turn-absent-<nonce>` | — | 3 | nothing written | PASS |
| export | `session --dir D export <id> --out P` | `F23_SESSION=export id=<id> path=P` | 0 | `EXPORT_NONCE_OCCURRENCES=0` | PASS |
| retain | `session --dir D retain <id> --days 7` | `state={"state":"retained",…}` | 0 | `retain_until` in session file | PASS |
| retain (expired) | `session --dir D retain <id> --days -7` | `state={"state":"expired",…}` | 0 | session still present on disk | PASS |
| reconcile (list) | `session --dir D reconcile <id>` | `F23_SESSION=reconcile id=<id> outstanding=N` | 0 | — | PASS |
| reconcile (resolve) | `session --dir D reconcile <id> --resolve <ref> --as-outcome not-started` | `F23_SESSION=reconcile_resolved …` | 0 | terminal receipt appended to journal | PASS |
| cancel | `session --dir D cancel <id>` | `F23_SESSION=cancel_turn …`, `cancelled=1` | 0 | `TurnCancelled` appended | PASS |

**Nonce proof of export redaction.** A value generated at run time
(`f23nonce<nonce>`) is planted into the seeded session. The driver asserts it IS in
the stored session bytes before exporting (exit 71 otherwise, so the proof cannot be
vacuous), then asserts `grep -c` over the exported artifact returns
`F23_01_EXPORT_NONCE_OCCURRENCES=0`. This holds by construction, not by filtering:
the envelope carries digests and typed state, never message text.

---

## 2. Live Windows UAT defect **D2** — closed end to end on Linux

The 20A live UAT recorded: *a crash-interrupted session is permanently unresumable;
`--continue` fails naming `resume, reconcile, or cancel` and no such command exists in
any of the 20 subcommands.*

Reproduced and closed with the real binary, no hand-authored fixtures:

| Step | Marker | Result |
|---|---|---|
| Seed a genuine crash: run a turn against a socket that accepts and never answers, then `kill -9` mid-dispatch | `F23_01_D2_FIXTURE_INTERRUPTED=true` | a real interrupted turn |
| Observe the refusal from the shipped binary, pre-repair: `wayland-core --resume <id> -- "next message"` | `F23_01_D2_REFUSAL_OBSERVED=true` | `error: Session persistence authority unavailable: session has an interrupted turn at journal cursor Some(9); resume, reconcile, or cancel it before starting a new message` |
| `wayland-core session reconcile <id>` | `F23_01_D2_RECONCILE_ITEMS_REPORTED=1` | names the blocking `provider_attempt`, its turn, its state and `resolvable=true` |
| `wayland-core session reconcile <id> --resolve <ref> --as-outcome not-started` | `F23_01_D2_RECONCILE_RESOLVED=1` | terminal receipt written |
| `wayland-core session cancel <id>` | verb `cancel` PASS, exit 0 | `TurnCancelled` written |
| Re-read from a fresh process | `F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=true` | `interrupted=0` |
| `wayland-core --resume <id> -- "next message"` again | `F23_01_D2_CONTINUE_UNBLOCKED=true` | the interrupted-turn refusal is gone |

**A vacuous gate was caught and removed here.** An earlier revision of the driver used
`--session-id <existing id>` for the resume legs. That errors `Session ID '<id>' already
exists` and never reaches the recovery check, so `D2_CONTINUE_UNBLOCKED` was `true`
both before and after the repair — a gate that could not go red. Switching to
`--resume` made the before-state go red, which is what made the after-state mean
anything.

---

## 3. FINDING — HIGH, pre-existing: a cleanly-exited run can write a journal the
product cannot read back

Not caused by this phase, not fixed by it, and materially worse than D2.

**Symptom.** After one ordinary headless run that exits normally (no crash, no kill),
`wayland-core --resume <that id>` fails:

```
Error: journal checksum mismatch at sequence 16
```

`wayland-core --list-sessions` still lists the session, so a user sees a session they
cannot re-enter, with no repair path — `session show`, `retry`, `export`, `reconcile`
and `cancel` all fail the same way, because every one of them reads the journal.

**Reproduction** (`hetzner-dsm`, base `15971d1b`, release binary):

| Burst | Runs | `--resume` OK | checksum mismatch |
|---|---|---|---|
| 1 | 8 | 0 | 8 |
| 2 | 10 | 1 | 9 |
| 3 (interleaved, low load) | 3 | 3 | 0 |

Intermittent and load-sensitive rather than deterministic — burst 3 ran while the host
was quiet and did not reproduce; bursts 1 and 2 ran under concurrent compile load from
other phases (15-minute load average 28). Journal size at failure ≈ 203 KB versus
≈ 71 KB on the passing shape, so the failing runs get further through the turn.

**Why this is attributed to the product and not to this phase.** The three changed
files on the write path are: two accessor methods and one visibility change in
`session.rs`, a new module the engine never calls, and a new CLI subcommand. Nothing
in this phase touches journal append, checksum computation, or flush ordering. The
failing read is the ENGINE's own resume path, which this phase does not modify.

**The one verification I did NOT do:** build a pristine binary from `15971d1b` with no
phase sources applied and re-run the burst. That would convert "very high confidence
pre-existing" into proof. It is the single highest-value next step on this finding.

---

## 4. Platform coverage — honest statement

| Platform | Status |
|---|---|
| Linux (`hetzner-dsm`) | **DRIVEN.** Full driver PASS, all fifteen verb rows, plus the D2 chain. |
| macOS | **OPEN — not driven.** The plan's decision was to build the binary on this Mac via `scripts/f23-macos-binary.sh`. The phase's controlling instruction forbids running Cargo on the Mac (`cargo fmt --all -- --check` excepted), which directly contradicts that decision. I honoured the controlling instruction, so no macOS binary exists, `scripts/f23-macos-binary.sh` was not written, and no macOS row is claimed. This conflict is escalated rather than resolved unilaterally. |
| Windows (`SeanDesktop`) | **OPEN — not driven.** The host was reachable and probed (PowerShell 5.1). The PowerShell port `scripts/f23-session-operator-drive.ps1` was not written and no Windows leg ran. No Windows row is claimed. |
| TUI (any platform) | **OPEN — not driven.** `/checkpoint`, `/fork` and `/export` were NOT added to the TUI command registry, and no PTY leg ran. Only the command surface is proved. |

No row above is closed by grepping a file this executor wrote. Every PASS traces to a
process exit status plus a nonce-bound marker in a captured log.

---

## 5. F23-02 disposition

**INCOMPLETE.**

Met: all fifteen verb invocations pass against the shipped binary on Linux; export
provably omits a run-time nonce; rewind restores byte-identical content and removes a
later-created file; fork leaves the parent byte-identical; checkpoint restore refuses a
destination outside the workspace root and writes nothing; reconcile and cancel close
live UAT defect D2 end to end with the disposition surviving a restart.

Unmet clauses, named:
1. macOS is not driven (no binary; instruction conflict, escalated).
2. Windows is not driven (PowerShell port not written).
3. The TUI verbs `/checkpoint`, `/fork`, `/export` were not added and no TUI leg ran.
4. `retry` is proved only on its refusal path (unknown turn → exit 3). The
   admitted-retry and expired-approval paths have unit coverage but no live row.
5. Binary provenance is base-sha only, not work-sha (see the caveat at the top).
