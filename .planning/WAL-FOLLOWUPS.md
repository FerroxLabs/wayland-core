---
lane: wal-followups
base: 5ce245be (lane/wal-nfs)
spec: ".planning/WAL-NFS.md — the three gaps that lane declined or logged"
gap-1: "CLOSED — the product corrupts memory.db on a network mount. 37,121 write errors, integrity_check corrupt, rc=0"
gap-2: "CLOSED — macOS SMB and Windows mapped drive both classified live, both sides, on real mounts"
gap-3: "CLOSED — five implementations, not three; consolidated onto one honestly-named module"
new-finding: "the two copies in wcore-tools were redundant in front of a better fifth copy nobody was using"
fence-exposure: "zero lines in crates/wcore-cli/src/{lib,main}.rs vs 5ce245be"
status: complete
---

# WAL/NFS follow-ups — the three gaps, closed

## Verdict

All three closed, and each turned out to be bigger than briefed. The headline: **the product
does corrupt its own long-term memory database**, so the fix the prior lane shipped is defending
against a failure that bites through real code paths, not only through a synthetic hammer.

---

## Gap 1 — the product-level proof (the important one)

**CLOSED. It corrupts.**

### What I built

`crates/wcore-memory/examples/nfs_memory_hammer.rs`. It calls `Memory::open()` — the same
constructor production bootstrap uses — and writes through `record_episode` /
`update_user_model`, the same calls the memory tool makes. It is the product's storage layer
under a harness `main`; it is not `rusqlite` in a loop. `memory.db` is the database at stake,
and it holds long-term user memory, so corruption there is unrecoverable user data loss.

Two writers, one per incoherent NFS client (`nosharecache`, `dev=183` / `dev=184`, same backing
file), 4 tokio tasks each, 150s. Journal mode read back **from the database**, never inferred
from the environment (§3b-ii).

### The result

| arm | filesystem | mode (read back) | writes ok | writes err | own rows visible | `integrity_check` |
|---|---|---|---|---|---|---|
| 1 **defect** | NFS ×2 incoherent | `wal` (forced) | 19,655 | **37,121** | A: *the query itself failed* · B: 3,834 vs 3,238 | **corrupt** |
| 2 **fix** | NFS ×2 incoherent | `truncate` (**selector**) | 47,170 | **0** | 11,507/11,507 · 12,078/12,078 | `ok` |
| 3 **control** | local ext4 | `wal` (forced) | 285,104 | **0** | 76,012/76,012 · 66,540/66,540 | `ok` |

Writer A could not count its own rows: `own_episodes_visible=<query failed: database disk image
is malformed>`. Writer B saw **more** rows than it had written. **Both processes exited
normally — no signal.** The prior lane's correction to the original bug report holds at the
product level too: the process lives, the data dies.

Arm 2 is the fix working on the same mount that destroyed the database in arm 1. Arm 3 is what
stops arm 1 being explained by "the driver is racy" — identical code, identical concurrency,
WAL, 285k writes, zero errors — and it re-earns WAL on local disks rather than assuming it.

### Why the prior lane's attempt did not reproduce — and why my first attempt didn't either

The prior lane blamed duration, and duration is part of it. But **my first run also failed to
reproduce, for a different reason, and it looked like a clean pass**:

```
HAMMER label=A FATAL open_failed err=memory DB: database is locked
HAMMER label=B RESULT ... writes_ok=45342 writes_err=0 integrity_check=ok
```

Writer A never started. The schema migration takes a long exclusive lock and on NFS the second
opener loses it, so the arm measured a *single* writer — which on one client has a coherent page
cache and is perfectly safe — and reported `integrity_check=ok` with 45,342 clean writes. Had I
read that as the answer, I would have published "the product does not corrupt" on the strength
of a run in which the concurrency never existed. Pre-creating the database so neither writer
loses the race at open is what made them actually overlap.

**So a concurrency test on this path has a second way to self-pass: a participant that never
starts.** Assert every writer reached `START`, not merely that the run exited. This is a new
flavour of the §3.2 class and I have not seen it recorded in this program before.

### What this does not show

The driver is a harness `main`, not an LLM-driven session. It exercises the product's storage
API and the real `memory.db`; it does not prove a *user-paced* session reaches that write rate.
The exposure argument is unchanged from the prior lane — `wcore-memory` holds a connection for a
whole session — but the *rate* here is far above a human session's. What is now proven is that
the product's own code paths corrupt the database when they overlap on a network mount; what is
not proven is how long a real session must run to get there.

---

## Gap 2 — Windows and macOS, live on real network mounts

**CLOSED, both hosts, both sides.**

New test `crates/wcore-config/tests/live_fs_class.rs`. `#[ignore]`d because CI has no such
mounts, and when the environment is absent it **panics rather than returning** — an env-gated
early `return` is the measured "printed `5 passed` for zero work" defect, and this cannot do it.

**macOS** — this Mac (`Darwin arm64`). Samba on hetzner bound to `127.0.0.1` only, reached
through an ssh tunnel, mounted with `mount_smbfs` as an ordinary user. No sudo was needed and
nothing was exposed publicly. Write-through verified: a file written on the Mac was read back
out of `/srv/walsmb` on hetzner. The mount carries **no `local` flag** — no `MNT_LOCAL`, the
exact signal the classifier keys on.

```
LIVEFS os=macos network=/Users/seandonahoe/walfu-smb -> Network / Truncate
LIVEFS os=macos local=/private/tmp                   -> Local / Wal
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

I also ran `sqlite_journal::tests::mnt_local_matches_libc`, which is `#[cfg(target_os = "macos")]`
and had therefore **never executed on any host in this program**. 1 passed — `MNT_LOCAL` really
is `0x1000`.

Both runs used the LANE-BRIEF §0 **Darwin-behaviour exception** (single crate, single test file,
never a workspace build or clippy). They qualify because no permitted host runs macOS and the
`MNT_LOCAL` arm is behaviour only Darwin can demonstrate.

**Windows** — `SeanD@seandesktop`, everything under `D:\walfu`, nothing at the root of `C:\`.
`net use Z: \\localhost\D$` maps a real drive through the SMB redirector. `fsutil` is the
independent control: `Z: - Remote/Network Drive`, `D: - Fixed Drive`.

**`Z:\walfu\probe` and `D:\walfu\probe` are the same directory.** Nothing differs between the
two arms except the access path, so the answer is attributable to `GetDriveTypeW` and nothing
else. This is the arm no amount of string inspection can reach — `Z:\` is spelled exactly like a
local drive letter, which is precisely why it needed a live mount to test.

```
LIVEFS os=windows network=Z:\walfu\probe -> Network / Truncate
LIVEFS os=windows local=D:\walfu\probe   -> Local / Wal
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Both hosts, both failure controls fire** (§3.2 — a gate I cannot make fail is not a gate):

| control | macOS | Windows |
|---|---|---|
| local path handed in as the network path | `FAILED. 0 passed; 1 failed` | `FAILED. 0 passed; 1 failed` |
| no environment at all | `FAILED` + "must FAIL rather than silently pass" | same |

---

## Gap 3 — the divergent `is_network_path` copies

**CLOSED, and there were five, not three.**

The brief named three. A concept sweep rather than a keyword sweep (§3b-i rule 3) found two
more, and the fifth is better than any of the three briefed:

| # | site | shape |
|---|---|---|
| 1 | `wcore-config/shell/executable_readiness.rs:540` | string prefix; **hard `false` off Windows** |
| 2 | `wcore-tools/vision_tools.rs:167` | `Component::Prefix` — a constant `false` on Unix |
| 3 | `wcore-tools/media_intake.rs:185` | string prefix |
| 4 | `wcore-config/sqlite_journal.rs:349` `is_windows_unc` | string, excludes `\\?\`/`\\.\` |
| 5 | `wcore-tools/path_validation.rs:172` `looks_like_unc` | separator-normalising, prefix-authoritative on Windows, keeps UNC distinct from device/verbatim |

They disagreed on real inputs, not cosmetics:

| input | #1 | #2 | #3 | consolidated |
|---|---|---|---|---|
| `\\?\C:\Users\x` — verbatim path to a **local disk** | network | not | network | **not** |
| `\\.\pipe\x` — device namespace | network | not | network | **not** |
| `//server/share` on Unix | not | not | network | **network** |

**The new finding: #2 and #3 were redundant.** Both `vision_tools::load_local_image` and
`media_intake::admit_path` call their local copy and then call `validate_user_path`, which runs
#5. The local copies only changed which error came back. So the workspace had two hand-rolled,
partly-wrong checks standing in front of a correct one that was already in the same crate.

### What I did

`crates/wcore-config/src/network_path.rs` — one implementation, promoted from #5 (the best of
the five), with the kernel-backed `classify_path` re-exported beside it so a reader lands on
**both** questions at once and can see which one they want. All five call sites now route
through it. `is_network_path` no longer exists in the tree.

### The judgement call, and why I did not do what the brief suggested

The brief says "route them through the new centralised selector. If any caller genuinely wants
UNC-only semantics, keep that but name it honestly." I concluded **all** of them want UNC-only
semantics, so the change is a rename plus a consolidation rather than a mechanism swap. That is
close enough to declining the headline instruction that I cross-audited it (§4).

**Panel: 3/3 `PANEL_POSITION=SYNTACTIC`** (codex gpt-5.6-sol, gemini-3.1-pro, kimi K3), plus an
internal adversarial pass. The reasons, two of which I had not thought of:

1. **The kernel check is self-defeating against this threat.** To classify
   `\\evil-host\share\x.png` with `statfs`/`GetDriveTypeW` you must *touch* it — and touching it
   is the dial-out the guard exists to prevent. The guard must be a pure function of the string,
   before any I/O.
2. **Call site #3 makes it categorically impossible.** `executable_readiness` *simulates* a
   target platform — it is routinely asked about Windows semantics while running on Linux. There
   is no Windows kernel to ask from Linux.
3. **It would be a new policy, not a fix.** An NFS-mounted `$HOME` and an admin-mapped `Z:` are
   ordinary, user-trusted storage. Refusing them would break legitimate use and stop nothing:
   the attacker's vector is an unmapped UNC name, which has no drive letter to classify.

Adversarially, the strongest counter is that a network-backed path can hang or change under a
reader. Both call sites already handle that elsewhere — `media_intake` opens once and bounds the
read, and `executable_readiness` already runs metadata probes behind a timeout for exactly the
"a network/autofs path cannot hang the session" case. So the counter is real but already paid.

### Behaviour changes, stated plainly

- `\\?\C:\Users\x` is no longer called a network path. **It is still refused** — by
  `validate_user_path`'s device/verbatim guard, which is the accurate reason. Asserted
  executably in `a_verbatim_local_path_is_still_refused_but_no_longer_as_a_network_path`, not
  reasoned about in a comment.
- `vision_tools` now flags a UNC string on Unix as well as Windows. Its old test asserted the
  UNC case under `#[cfg(windows)]` only, so the guard was never exercised by the Linux/macOS
  runs that CI actually does. That gate is deliberately gone.
- Net effect: **no input that was refused before is accepted now.** The changes are one
  reclassification and one widening.

---

## Gates

| gate | result |
|---|---|
| `cargo test -p wcore-config --lib network_path` | **6 passed; 0 failed; 0 ignored; 561 filtered out** |
| `cargo test -p wcore-config --lib sqlite_journal` | **11 passed; 0 failed; 0 ignored; 556 filtered out** |
| `cargo test -p wcore-config --lib executable_readiness` | **18 passed; 0 failed; 0 ignored; 549 filtered out** |
| `cargo test -p wcore-config --lib` (parallel) | 565 passed; **2 failed** — see below |
| `cargo test -p wcore-config --lib -- --test-threads=1` | **567 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-tools` | 993 + 4 suites, **0 failed** (3 ignored, pre-existing) |
| `cargo test -p wcore-memory` | 348 + 26 suites, **0 failed** |
| `cargo clippy -p wcore-config -p wcore-tools -p wcore-memory --all-targets -D warnings` | clean |
| `cargo fmt --all -- --check` | diff-free |
| live macOS SMB / live Windows mapped drive | **1 passed** each; both failure controls FAILED as required |
| fence vs merge-base `5ce245be` | **zero lines**; control diff shows 11 files / 982 insertions, so the instrument is live |

Counts read from unproxied `/usr/bin/env cargo`; the `0 ignored` / `filtered out` fields are
present, so the suites genuinely executed rather than exiting 0 on zero tests.

### The two parallel-run failures — not mine, with the mechanism

`config::tests::test_resolve_cli_max_tokens_marks_explicit` and
`config::tests::test_resolve_without_project_dir_uses_cwd` fail in a parallel run and **pass in
isolation**, and the whole 567-test suite is green single-threaded. The mechanism is visible:
`config.rs`'s test module calls `std::env::set_var` / `remove_var`, which is process-global, so
those tests interfere with each other. My new module mutates no global state — verified by
grep — so what my six added tests changed is the *scheduling*, not the behaviour. I did not
rebuild at base to confirm pre-existence; the causal chain above is my evidence and I am
flagging the gap rather than asserting more than I measured. Non-blocking; BACKLOG candidate.

---

## What I did NOT establish

- **Whether a real, user-paced session reaches the corrupting write rate.** The driver writes
  far faster than a human session would. The product's code paths corrupt when they overlap;
  how long a genuine session must run to get there is unmeasured.
- **Pre-existence of the two `config::tests` parallel failures**, by building at base. Argued
  from mechanism, not measured.
- **Windows and macOS were tested against loopback network mounts** (SMB to `\\localhost\D$`;
  SMB to a tunnelled Samba on hetzner) — the same shape of stand-in the prior lane used for NFS,
  and genuinely remote from the API's point of view (`fsutil` says `Remote/Network Drive`, the
  Mac mount has no `MNT_LOCAL`). Not a physically distinct fileserver over a real LAN.
- **No AFP mount** — macOS has been deprecating AFP for years and the box had no AFP server.
  SMB is the form users actually have.
