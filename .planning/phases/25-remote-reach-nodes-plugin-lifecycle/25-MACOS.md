# 25-MACOS — Darwin evidence for Phase 25

Lane `lane/25-macos`, base `plan/f20-unified-audit-repair` @ `e77b44b0`. Run 2026-07-29.
Hosts: this Mac (Darwin 25.3.0 arm64, macOS 26.3 build 25D125, uid 501) and `hetzner-dsm`
(Linux 6.8.0-101-generic x86_64, root). Running log: `evidence/25-macos/25-MACOS-NOTES.md`.

---

## 1. Headline

The inventory's framing — *"Phase 25 has zero macOS evidence across all four criteria"* — is
**true as stated and misleading as a priority signal.** Measured against the code rather than the
evidence ledger, **most of Phase 25 has no Darwin-distinct surface at all.** Two legs genuinely
needed Darwin. Both are now measured. One produced a real Linux/macOS divergence.

The rest of the phase would re-run identical source over identical POSIX semantics — the inventory
priced a macOS run of the twelve plugin verbs at **2 sessions**, and on the Darwin/Linux axis it
buys nothing. That is the single most useful thing this lane can tell the programme.

**One divergence found, graded MEDIUM:** every macOS node reports the *constant*
`machine_id = "unknown-host"`, because the fallback that derives it reads two Linux-only paths.
It passes the contract's own validator, so nothing rejects it.

---

## 2. What the criteria actually require — read from source, not paraphrase

From `ROADMAP.md:128-132` and `REQUIREMENTS.md:237-241`:

> 1. The same task runs locally, in a container, over SSH, and on one hibernating cloud backend with equivalent policy, receipts, cancellation, and cleanup.
> 2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions without losing authority attribution.
> 3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated, rolled back, removed, published, and recovered.
> 4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no orphaned execution.

**None of the four criteria, and none of F25-01..05, names an operating system.** Siblings that do
name one prove this is a deliberate distinction, not an oversight: 24-C5 *"passes on macOS, Linux,
Windows"*, 27-C5 *"native macOS, Linux, and Windows"*, 28-C1 *"Native macOS, Linux, and Windows"*.
Phase 28 is *Native Cross-Platform Certification* and **depends on Phases 24-27**.

I recorded the consequence in NOTES **before running anything**, so it could not be tuned to the
result: **a criterion cannot be graded NOT MET for lacking macOS when it does not ask for macOS.**
The honest question this lane can answer is not "is the coverage complete" but **"does the
behaviour behind these criteria actually differ on Darwin"** — and a divergence *is* a defect
against the criterion's own words.

---

## 3. Which legs genuinely need Darwin, and which do not

The objective test used throughout: **a leg needs Darwin iff the code path it exercises has a
Darwin-distinct branch, or depends on a platform facility whose behaviour differs.** Not "is it
platform-adjacent".

### Legs that GENUINELY need Darwin — 2

| leg | criterion | why Darwin is required | ran? |
|---|---|---|---|
| `process_liveness` macOS arm | C4 (no orphans), C1 (cleanup/cancellation) | `#[cfg(target_os = "macos")]`, `sysctl KERN_PROC_PID` with **hardcoded `kinfo_proc` ABI offsets**. hetzner cannot compile it, let alone run it. | **YES** |
| `NodeIdentity::local()` machine_id | C2 (nodes advertise / operator identifies) | derivation depends on the **absence** of `/etc/hostname` and `/proc`, which only Darwin exhibits | **YES** |

### Legs that do NOT need Darwin — re-running them proves nothing

| leg | criterion | why re-running on macOS buys no evidence |
|---|---|---|
| twelve plugin verbs | C3 | **No `target_os = "macos"` anywhere in `crates/wcore-cli/src/plugin/`.** Every platform branch is `cfg(unix)`/`cfg(windows)`, and every `cfg(unix)` body is plain POSIX that Darwin and Linux implement identically: `set_permissions(0o600)` (`sign.rs:58`), executable-bit carry via `PermissionsExt::mode()` (`generations.rs:432`), `symlink` (`generations.rs:585`). Same source, same semantics. |
| pairing / revoke / mixed-version / attribution | C2 | ed25519 + serde over portable types. Zero platform surface. |
| policy equivalence, receipts, role gating | C1 | provider-neutral logic; the normalized-diff comparison is over serialized structures. |
| fail-closed cases (rotated key, tampered bundle, denied secret, denied egress, attestation mismatch) | C4 | policy decisions on portable types. |

**Two Darwin traps I chased and closed as NOT load-bearing** — recorded because the negative result
is what stops the next lane spending the 2 sessions:

- **APFS is case-insensitive; ext4 is case-sensitive.** Measured both sides in one probe
  (`Plugin.txt` resolves as `plugin.txt` on macOS: **CASE-INSENSITIVE**; on hetzner:
  **CASE-SENSITIVE**). A real divergence — but **unreachable** in C3, because
  `resolver::validate_plugin_name` restricts names to a leading `[a-z]` then `[a-z0-9-]`, enforced
  at **9 call sites** across `install.rs`, `resolver.rs` and `scaffold.rs`. Uppercase never reaches
  the filesystem, so no collision is constructible.
- **`ps -eo` — the flag whose msys rejection produced the Windows silent false zero.** Works on
  Darwin: `ps -eo pid,ppid,args` → rc=0, **852 rows**; hetzner → **1848 rows**. The C4 enumeration
  instrument is portable to macOS. (Cosmetic only: macOS heads the column `ARGS`, Linux `COMMAND` —
  a harness matching the literal header string would diverge; none observed doing so.)
- **`.dylib` / Gatekeeper quarantine** — does not arise. The word "quarantine" throughout the
  plugin code is *their* term for the isolated git clone of a foreign plugin source
  (`plugin/quarantine.rs`), not `com.apple.quarantine`. Checked rather than assumed; assuming
  would have manufactured a phantom leg.

---

## 4. Use of the §0 Darwin-behaviour exception — disclosed

**Invoked once, for one crate and one test file**, exactly as §0 permits:

```
/Users/seandonahoe/.cargo/bin/cargo test -p wcore-types       --test real_zombie
/Users/seandonahoe/.cargo/bin/cargo test -p wcore-exec-backend --test node_contract
```

No workspace build. No clippy. No release build. `cargo fmt --all -- --check` only, which §0
already permits.

**Why it qualifies, per leg:**

- `wcore-types --test real_zombie` — the code under test is `#[cfg(target_os = "macos")]`. It does
  not exist on hetzner. §0's stated grant case is precisely this: the arm's provenance was a **C**
  probe written to work around the rule, and the exception exists to get *"a result worth having
  in Rust"*.
- `wcore-exec-backend --test node_contract` — the behaviour is produced by the **absence** of
  `/etc/hostname` and `/proc`. hetzner has both, so it cannot exhibit the path at all.

Everything hetzner could prove ran on hetzner: the Linux half of the divergence, and the Linux
control for both filesystem probes.

---

## 5. Per-criterion results, with counts

Every count read back from an **unproxied** tool (`/usr/bin/grep`, `/usr/bin/git`, cargo by
absolute path). Executed-test counts are read from `N passed … 0 ignored … 0 filtered out`, never
inferred from exit status (§3.2).

### C1 — local / container / ssh / cloud equivalence · macOS: PARTIAL

| leg | host | result |
|---|---|---|
| local-backend liveness + cleanup substrate (`local.rs:386` → `process_liveness`) | Darwin | **5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** |
| four-backend equivalence, receipts, policy | — | **not Darwin-specific**; proven on Linux at `5e620ef0` |

The platform-specific substrate C1's cleanup clause rests on is now proven on Darwin. The
end-to-end four-surface equivalence is **not** proven on macOS and **cannot be** in this lane —
it needs the shipped binary, and §0 forbids building it on the Mac. Stated as a hard limitation,
not worked around. Unchanged from the inventory: C1 remains a composition across two commits.

### C2 — nodes pair, advertise, revoke, recover, mixed versions · macOS: **DIVERGENCE**

| leg | host | result |
|---|---|---|
| `NodeIdentity::local()` on Darwin | Darwin | **19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** → `machine_id=unknown-host os=macos validates=yes` |
| `NodeIdentity::local()` on Linux | hetzner | **19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** → `machine_id=ubuntu-2404-noble-amd64-base` |
| pairing / revoke / mixed-version / attribution | — | **not Darwin-specific** |

See §6. Authority attribution — C2's load-bearing clause — is **unaffected**; `key_id` carries it
and differs correctly.

### C3 — twelve-verb plugin lifecycle · macOS: NO DARWIN EVIDENCE REQUIRED

No leg of C3 is Darwin-distinct (§3). I ran **zero** verbs on macOS deliberately, and that is the
finding: the inventory's *"No macOS run of any verb — 2 sessions"* would re-execute identical
source over identical POSIX semantics. The two candidate Darwin divergences (case-insensitive FS,
Gatekeeper) were chased and closed with measurements, above.

**This does not make C3 fully proven on macOS** — it makes a macOS *re-run* uninformative on the
Darwin/Linux axis. A macOS run under Phase 28's native matrix still has value for
packaging/binary-level concerns (code signing, notarization, `.app` layout) that live outside
Phase 25's criteria.

### C4 — fail closed, no orphaned execution · macOS: substrate PROVEN

| leg | host | result |
|---|---|---|
| liveness probe, all four arms incl. **ARM D** | Darwin | **5 passed; 0 failed; 0 ignored; 0 filtered out** |
| mutation control (old `kill(pid,0)` shape reinstated) | Darwin | **FAILED. 3 passed; 2 failed** — as designed |
| `ps -eo` enumeration portability | Darwin / hetzner | rc=0, **852** / **1848** rows |
| fail-closed policy cases | — | **not Darwin-specific** |

---

## 6. The divergence — `machine_id` degenerates to a constant on Darwin

**Severity: MEDIUM.** Operator-facing correctness, no security consequence.

`crates/wcore-exec-backend/src/node/pairing.rs:102-113`:

```rust
/// Unix hosts publish the hostname on disk regardless of shell environment.
fn read_hostname_file() -> Option<String> {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
```

**The doc comment is false on macOS.** Darwin has neither file and no `/proc`; the hostname lives
in the SystemConfiguration store (`scutil --get LocalHostName` → `Seans-MacBook-Pro`).

Measured, both sides, unproxied, each with a live positive control in the same invocation so the
absences are measurements and not a dead instrument (§3b-i):

| path | Darwin 25.3.0 | Linux 6.8.0-101 |
|---|---|---|
| `/etc/hostname` | ABSENT | EXISTS → `Ubuntu-2404-noble-amd64-base` |
| `/proc/sys/kernel/hostname` | ABSENT | EXISTS → `Ubuntu-2404-noble-amd64-base` |
| `/proc` | ABSENT | EXISTS |
| `/etc/hosts`, `/etc/passwd` (positive control) | EXISTS, EXISTS | — |
| `HOSTNAME` / `COMPUTERNAME` / `WAYLAND_NODE_MACHINE_ID` in env | **all unset** (control: `PATH` matched) | unset over non-login ssh |

So the fallback chain runs off its end and returns the constant `"unknown-host"`. Executed, not
inferred:

```
DARWIN machine_id fallback: machine_id=unknown-host os=macos validates=yes key_id_differs=yes
LINUX  machine_id fallback: machine_id=ubuntu-2404-noble-amd64-base os=linux hostname_file=Ubuntu-2404-noble-amd64-base
```

**Why it is a defect and not cosmetics.** The field's declared job (`pairing.rs:39-41`) is
*"Stable per-host discriminator. Distinguishes two nodes an operator happened to give confusingly
similar names."* On Darwin it is a compile-time constant independent of the host, so it
discriminates nothing and **two macOS nodes cannot be told apart by it** — surfaced directly to the
operator by `wcore-cli/src/node.rs:243` and `:307` (`machine   unknown-host`). It also **passes
`NodeIdentity::validate()`**, so no downstream check rejects it.

**Why it is the shape the brief asked for.** `local_machine_id`'s own comment records this exact
bug already being found and fixed once — *"`HOSTNAME` is a SHELL variable… every host would report
`unknown-host`. Found by running the real binary over ssh."* **The fix was Linux-shaped. Darwin
kept the pre-fix behaviour.** Works on the surface we test; broken on a surface we ship.

**Why MEDIUM and not HIGH,** with the counter-argument stated rather than buried: `machine_id` is
explicitly *"a LABEL, not a security boundary — `key_id` carries the security"*, and the
measurement confirms `key_id` still differs. C2's load-bearing clause, authority attribution, is
untouched. A reader who takes "nodes advertise capability" to include "the operator can tell two
nodes apart" would grade this HIGH; I record both readings and take MEDIUM, consistent with the
inventory grading the comparable C2 attribution limitation MEDIUM. Per §5, MEDIUM → BACKLOG,
non-blocking.

**Fix, ~5 lines:** add a `libc::gethostname` / `scutil --get LocalHostName` arm to the fallback
chain before the `"unknown-host"` default. My committed test is a **characterization** test — it
asserts the *current* Darwin behaviour and says so in its failure message, so whoever fixes this
gets a deliberate red telling them to update it, not a silent pass.

---

## 7. ARM D — a Rust test that did not exist

The C probe (`.planning/evidence/zombie-probe/MACOS-PROBE-RESULT.txt`) recorded **four** arms.
Three had Rust tests. The fourth did not:

> `ARM D: live, other user (launchd) pid=1 kill(pid,0)_says_alive=0 … sysctl.p_stat=2 -> LIVE`

Absence verified per §3b-i — query stated so it can be re-run:
`/usr/bin/grep -n "launchd\|EPERM\|other_user\|ARM D\|arm_d" crates/wcore-types/tests/real_zombie.rs`
→ **rc=1, zero matches**, with a positive control in the same file (`grep -c zombie` → **4**).

**ARM D is the dangerous direction.** The other three ask "is a corpse mistaken for alive?" ARM D
asks the opposite — **is a live process mistaken for dead?** A liveness probe that reads a live
process as dead makes an orphan reaper believe there is nothing to clean up. That is a *false
clean*, and it is C4's exact clause ("no orphaned execution").

**It is unobservable on the Linux proof host**, which runs as root: `kill(1, 0)` succeeds for root,
so hetzner cannot demonstrate it. Darwin at uid 501 can:

```
ARM D reproduced on Darwin: uid=501 pid=1 new_probe=Live old_shape(kill(1,0))=alive:false errno=1 (EPERM=1)
```

Three assertions per §6b-ii — known-positive (pid 1 Live), known-negative (a reaped pid still Dead,
**in the same test**, so a probe answering Live to everything cannot pass), and **the old shape
would have missed it**. It `assert_ne!`s on euid 0 rather than skipping, so it cannot go green by
being unobservable.

**Also established:** the hardcoded `kinfo_proc` ABI offsets (`p_stat`=36, `p_pid`=40, `SZOMB`=5)
are **still correct on macOS 26.3**, several releases newer than the C probe's host. Had Apple
moved them, the `p_pid` readback self-check degrades to `Indeterminate` → "assume alive" → the
corpse test goes red. It did not.

**Mutation control (§3.2 — a gate is worthless until seen to fail).** The macOS arm was temporarily
replaced with the pre-repair `kill(pid,0)` shape:

```
test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
  a_live_process_owned_by_another_user_reads_as_live_and_the_old_shape_called_it_dead ... FAILED
  a_real_unreaped_corpse_reads_as_dead_and_the_old_shape_would_have_missed_it ... FAILED
```

Both directions red without the sysctl arm. `process_liveness.rs` restored byte-identically
afterwards (`git diff --stat` empty; `grep -c "MUTATION CONTROL"` → 0); the mutation never left the
working tree.

---

## 8. Instrument finding — `rtk` rewrites `cargo` as well as `git log` / `grep` / `git diff`

§3b lists `git log`, `grep` and `git diff`. **Add `cargo`.** The first run of the liveness suite came
back as:

```
cargo test: 4 passed (1 suite, 0.01s)
```

That is not cargo's output. The count happened to be right, but the re-render **strips
`0 ignored` and `0 filtered out`** — precisely the two fields §3.2 mandates for detecting a suite
that exits 0 having run nothing. **The proxied form is structurally incapable of supporting the
check the brief requires.** Repaired in-lane rather than merely noted (§6b-ii): every cargo
invocation in this lane uses the absolute path `/Users/seandonahoe/.cargo/bin/cargo`, and every
count in this report is read from a raw `test result:` line.

---

## 9. Honest verdict

- **Goal of this lane — achieved.** Phase 25 now has macOS evidence on **both** legs that genuinely
  required it, and a measured, defensible negative for the legs that did not.
- **The gap was mis-sized, not mis-reported.** "Zero macOS evidence across four criteria" is
  literally true; "the worst platform gap in the slice" over-weights it, because ~most of Phase 25
  has no Darwin-distinct surface. The expensive item in the inventory (C3, 2 sessions) is the one
  worth **not** buying.
- **One divergence, MEDIUM**, filed to BACKLOG per §5, with a ~5-line fix specified.
- **Phase 25's own grade is unchanged at 3.5/4.** Nothing here moves a criterion: the criteria are
  platform-silent, the divergence is MEDIUM, and C2's load-bearing attribution clause is unaffected.
- **What I could not do:** live-exercise the shipped `wayland-core` binary on macOS. §0 permits one
  crate and one test, not a binary build, and no permitted host runs macOS. So the **library-level**
  Darwin behaviour is proven and the **binary-level** behaviour is not. That is a real, unclosed
  gap and it belongs to Phase 28's native matrix, which is where the criteria put it. I am naming
  it rather than redefining success around it.

## 10. Artifacts

| file | bytes |
|---|---|
| `evidence/25-macos/25-MACOS-NOTES.md` | running log, committed at t+9min then appended |
| `evidence/25-macos/25m-real-zombie-darwin.txt` | 813 |
| `evidence/25-macos/25m-armd-darwin.txt` | 1103 |
| `evidence/25-macos/25m-mutation-control-darwin.txt` | 2035 |
| `evidence/25-macos/25m-machine-id-darwin.txt` | Darwin node_contract run |
| `evidence/25-macos/25m-machine-id-linux.txt` | 8620 (hetzner) |

Code: `crates/wcore-types/tests/real_zombie.rs` (+ARM D),
`crates/wcore-exec-backend/tests/node_contract.rs` (+Darwin and Linux machine_id twins).
Neither shared fence file (`wcore-cli/src/lib.rs`, `main.rs`) was touched.
