# lane `25-c4-windows` — running NOTES

Append-only working record. Committed early and re-committed after every measurement,
per LANE-BRIEF §6b-i. If this lane dies, resume from the last section.

Lane branch: `lane/25-c4-windows`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-25-c4-windows`
Merge-base captured once: `632ad619baa35f786c403756f527edac05604a9a`

---

## Minute 0-15 — what the Linux proof actually was

Read `25-C4-SUMMARY.md` and `evidence/25-c4/25-c4-egress-2x2.txt` before touching anything,
because the task requires reproducing the **shape** of the Linux proof, not inventing a new one.

**Shape of the Linux proof (lane `25-c4-egress`, commit `b64695df`, hetzner, 2026-07-29):**

- One command, run twice: `wayland-core backend orphans --nonce <32-hex-nonce>`
- **One variable only**: `XDG_CONFIG_HOME` pointing at two configs differing in a single key —
  `[security] egress_allow = ["api.machines.dev"]` (allow arm) vs `= []` (deny arm).
- Credential sourced from `/root/.wayland-f25-cloud.env` (`WAYLAND_F25_CLOUD_TOKEN` +
  `WAYLAND_F25_CLOUD_ORG`), never printed.
- **Allow arm** cloud row: `machine listing returned HTTP 404: {"error":"app not found"}`
  ← Fly's own servers answered. This is the positive control: a vendor HTTP status is not
  producible by a broken build, an absent policy, or a no-op.
- **Deny arm** cloud row: `egress denied: GET with a long or high-entropy path/query to a
  non-allowlisted host. Egress to \`api.machines.dev\` is blocked by the security policy.`

So the Windows leg must produce the same two rows from the same command with the same single
config variable.

## Source facts established (unproxied `/usr/bin/grep`, from the merged tree)

1. The fix is already on integration: `crates/wcore-cli/src/backend.rs:136 fn arm_egress_policy()`,
   called at `:155` as the first statement of `backend::run`. Nothing to re-implement; the
   Windows leg is a **build + run**, not a code change — unless a Windows-specific divergence
   shows up.
2. `crates/wcore-exec-backend/src/backends/cloud.rs`:
   - `TOKEN_ENV = "WAYLAND_F25_CLOUD_TOKEN"` (:43), `ORG_ENV = "WAYLAND_F25_CLOUD_ORG"` (:52)
   - `API_BASE = "https://api.machines.dev/v1"` (:56)
   - `CloudCredential::from_env` (:124) accepts **any non-empty** token; it does not validate it.
   - `api_get` (:308) builds an `EgressClient`, attaches `bearer_auth`, `.send().await`, then
     reads `response.status()`. **The `machine listing returned HTTP {status}` string is only
     reachable after a real HTTP round trip completed.**
3. `machines_with_nonce` (:574) builds `/apps/{app}/machines?metadata.wayland_task_nonce={nonce}`
   — a 32-hex nonce is what trips the product's `get_carries_data` exfil rule.

### Consequence for the credential question (important)

The previous lane recorded the Windows blocker as "needs a cloud credential there (the F25
credential exists only on hetzner; the two hosts cannot reach each other)".

Reading the source says the **positive control does not need a *valid* credential — only a
*present* one.** The evidentiary bar is "a remote server answered". With a bogus-but-present
token, `api.machines.dev` answers **401**; with a valid token and an unknown app it answered
**404**. Both are Fly's servers replying over TLS; both are impossible for a broken build,
a no-op, or an unarmed policy to fake. The `CredentialAbsent` short-circuit — the thing that
made the old Windows evidence vacuous — is escaped by presence alone.

So: **no secret needs to cross the wire, and none will.** If the run shows anything other than
a genuine vendor HTTP status in the allow arm, I report the positive control as NOT achieved
rather than grading the denial arm.

## Platform-divergence sweep — `wcore-egress` has ZERO platform branches

Absence claim, so with a liveness control in the same tree (LANE-BRIEF §3b-i):

```
$ /usr/bin/grep -rc "fn " --include="*.rs" crates/wcore-egress/     # KNOWN-POSITIVE
crates/wcore-egress/src/request.rs:14 policy.rs:14 client.rs:32 error.rs:7
lib.rs:22 url_allow.rs:6 observer.rs:23                             # 118 matches, instrument alive
$ /usr/bin/grep -rn "cfg(windows)"   --include="*.rs" crates/wcore-egress/ ; echo rc=$?
rc=1                                                                # zero matches
$ /usr/bin/grep -rn "cfg(target_os" --include="*.rs" crates/wcore-egress/ ; echo rc=$?
rc=1                                                                # zero matches
```

The egress boundary is platform-neutral by construction, so AGENTS.md's "centralize platform
differences" rule is satisfied by adding **nothing**. Any `cfg!(windows)` I felt tempted to add
would be the violation, not the fix. Divergence, if any, is expected in the *harness*
(PowerShell exit-status collapse), not in the product.

## Host state

- `ssh -o BatchMode=yes SeanD@seandesktop 'hostname'` → `SeanDesktop`. Reachable.
- Work goes under `D:\lane-25c4-windows\`, never `C:\` root. `C:\actions-runner-*` untouched.

## Still to establish

- [ ] Windows toolchain present; build `wayland-core.exe` from this commit on `D:\`.
- [ ] Allow arm: vendor HTTP status from `api.machines.dev`.
- [ ] Deny arm: policy-level `egress denied`.
- [ ] `cargo fmt --all` + `cargo check --workspace --all-targets` (hetzner for the workspace check;
      the Mac may not build).
- [ ] Disk used under `D:\`, then clean up `target/`.

---

## Host survey (seandesktop, 2026-07-29)

```
Name UsedGB FreeGB      rustc 1.95.0 (59807616e 2026-04-14)
C      1193    669      cargo  C:\Users\seand\.cargo\bin\cargo.exe
D      2039   5413      git    C:\Program Files\Git\bin\git.exe
E         2   1861
```

Working tree: `D:\lane-25c4-win` (clone of the existing `C:\ferrox-win` objects, then
`git fetch origin lane/25-c4-windows`; HEAD = `280b9cdf`). Evidence + configs:
`D:\lane-25c4-ev`. Nothing created at the root of `C:\`; `C:\actions-runner-*` untouched.

### Credential: NOT present on this host, and none was moved here

```
WAYLAND_F25_CLOUD_TOKEN user_len=0 machine_len=0
WAYLAND_F25_CLOUD_ORG   user_len=0 machine_len=0
FLY_API_TOKEN           user_len=0 machine_len=0
C:\Users\seand\.wayland-f25-cloud.env exists=False
```
(names and lengths only — no value was read, printed or transmitted.)

Per the task's rule I am NOT improvising one and NOT importing hetzner's. Instead the
proof uses the source fact above: `CloudCredential::from_env` rejects only the EMPTY
string, so a **deliberately-invalid, clearly-labelled placeholder** gets past the
`CredentialAbsent` short-circuit and the product opens a real socket. `api.machines.dev`
then answers **401** where hetzner (holding a valid token, unknown app) got **404**.
Both are the vendor's servers replying over TLS; the evidentiary property the positive
control exists to establish — *the request physically left this machine and a remote
server answered* — is identical. This is stated as a divergence, not hidden.

---

## HARNESS DEFECT FOUND AND REPAIRED (LANE-BRIEF §6b-ii)

**The defect.** I launched the Windows build with
`Start-Process powershell -File build.ps1 -WindowStyle Hidden` from an ssh command, then
polled. The poller graded **"`build-status.txt` absent ⇒ STILL-BUILDING"**. It reported
`STILL-BUILDING` for **12 consecutive polls over ~9 minutes**, quoting a `build.log` frozen
at exactly `6002` bytes the whole time.

The build was **dead**. `Start-Process` detaches from the ssh session but does not survive
its teardown, so cargo was killed seconds after my ssh call returned. Confirmed positively,
not by absence: `Get-CimInstance Win32_Process` showed **zero** cargo/rustc processes whose
command line contained `lane-25c4-win` (the one live `rustc --crate-name wayland_core` on the
box belonged to another lane — `mine=False`), and my target tree sat at `files=1474 MB=236`
across a 60-second window with no change.

This is the §3b-i class exactly: **an absent status file is what a *running* build and a
*dead* build both look like**, so the poller could not fail.

**The repair, in this lane, not a note for the next one** — `D:\lane-25c4-win\poll.ps1`
never infers "running" from an absence. It requires a POSITIVE liveness signal (a
cargo/rustc process whose command line carries this lane's marker) and grades three states:
`DONE` (status file with `WLDONE`), `UNREADABLE` (status file without `WLDONE`), `BUILDING`
(liveness procs > 0), `DEAD` (no status file AND no procs).

**Three-assertion self-test, run on the box (the third is the one that proves the repair
does anything):**

```
--- A1 known-positive (real marker, build is live) ---
STATE=BUILDING liveness_procs=4
--- A2 known-negative (marker no process can carry) ---
STATE=DEAD no status file AND no cargo/rustc for marker 'lane-25c4-NOSUCH-MARKER' - the build is NOT running
--- A3 what the OLD matcher would have said in the A2 case ---
OLD=STILL-BUILDING   <- the old matcher cannot tell A1 from A2
```

**Second repair:** the build itself now runs in the **foreground** of a long-lived ssh
connection rather than via `Start-Process`, so the process cannot be orphaned by session
teardown in the first place.
