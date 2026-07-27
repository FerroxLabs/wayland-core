# The "AppContainer is unavailable over SSH" trap is wrong — it is a stale test lease

**Status: the lore is FALSIFIED on the measured box, with a repeatable red/green.**
Measured 2026-07-27 on `SeanD@seandesktop` by the Phase 22 lane, against
`C:\p22-target\release\wayland-core.exe` (built from `2ecdfdf5`).

The standing environment note — repeated in Phase 20/21/22 plan briefs as

> "These ACL and containment behaviors CANNOT be observed over a non-interactive
> SSH logon — a session-0 logon reports the AppContainer backend unavailable and
> every gated test panics regardless of correctness ... never conclude a red from
> an SSH run"

is **not true on this machine**. A real AppContainer spawn succeeds over a
session-0, non-interactive SSH logon. What actually disables the sandbox is a
leftover lease file written by the sandbox crate's *own test helper* into the
*real* production lease directory. It never self-heals, and its symptom reads
exactly like a platform limitation.

This matters beyond one box: the note has been used to dismiss sandbox reds as
environment artifacts. Any red dismissed that way needs re-reading.

---

## 1. The two claims, kept apart

They are different, and only the first is the lore.

| # | Claim | Verdict |
|---|---|---|
| A | A session-0 / non-interactive logon makes the AppContainer backend report unavailable. **This is the lore.** | **FALSE here.** 4/4 clean probes spawned successfully under `session_id=0`, `UserInteractive=False`, SSH. |
| B | `is_available()` can return false for reasons that have nothing to do with the logon — specifically an unreconcilable lease on disk. | **TRUE, and this is what was actually happening.** 2/2 wedged probes returned "sandbox disabled". |

The reason the two got confused is in the source: **on Windows `is_available()` is
not a capability query at all — it is a real spawn.**

`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:300-352`
runs `cmd.exe /c exit 0` through the full sandbox path and maps *any* failure to
`false`, logging:

```
AppContainer real-spawn probe failed; sandbox disabled. If the failure is
transient (AV, disk contention), the probe re-runs after the negative-cache TTL.
```

That line is emitted for a genuine platform limit and for a corrupt lease file
alike. It even *suggests* transience ("AV, disk contention"), which points the
reader away from persistent on-disk state. An operator who only ever reaches the
box over SSH sees "sandbox disabled" on every SSH run and concludes SSH is the
cause. That is the whole mechanism by which the lore formed.

---

## 2. What is actually wedged, and what clears it

**Wedged by:** any file in
`%LOCALAPPDATA%\Wayland\Core\AppContainerLeases\v1\` whose recorded
`sid_sha256` does not match the SID derived from its own `profile_name`, once
its `owner_pid` is dead.

**Cleared by:** deleting that file. Nothing else. There is no automatic
quarantine path and no TTL that helps — the negative probe cache is in-process
only (`OnceLock<Mutex<ProbeCache>>`, `process.rs:85-88`), so every new process
re-reads the same bad file and fails again. Proved: two separate `wayland-core`
processes both failed while the file was present (`wedged-1`, `wedged-2` below).

**The failing code path**, in order:

1. `ExecutionIdentity::start_with_apply` → `recover_dead_leases_locked(&lease_dir)`
   — `acl_lease.rs:227`. This runs *before* any spawn, on every execution.
2. For each lease whose owner PID is dead and whose state is not already
   revoked/cleaned, it derives the SID from `profile_name` via
   `DeriveAppContainerSidFromAppContainerName` and compares hashes —
   `acl_lease.rs:631-652`.
3. On mismatch it returns `Err("AppContainer ACL lease SID/profile mismatch in ...")`
   — `acl_lease.rs:647-652`. **Fail-closed, permanently.**
4. That error propagates out of the availability probe, which logs
   "sandbox disabled" and returns `false` — `process.rs:343-352`.

### Where the poisoning lease comes from — this is the actual defect

The two files found on the box were **written by the sandbox crate's own test
suite, into the production lease directory.**

`crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs:633-643`:

```rust
fn test_lease(tag: u64, state: LeaseState) -> LeaseFile {
    let mut lease = LeaseFile::new(
        format!("WCore-storage-{:08x}-{tag:016x}", std::process::id()),
        b"storage-test-sid",
        Vec::new(),
    )
```

Two things make this unreconcilable by construction:

* the `profile_name` is synthetic — **no AppContainer profile with that name is
  ever created**, so step 2 above can never derive a matching SID;
* the SID is the fixed literal `b"storage-test-sid"`, so every lease the helper
  writes carries an identical `sid_sha256`.

And the tests write to the real directory, not a temp one — `storage.rs:649` calls
`lease_directory()`, which resolves `%LOCALAPPDATA%` with no test override
(`storage.rs:36-52`).

**Proof rather than inference.** `sha256(b"storage-test-sid")` is

```
5b22ee051799cf8aa6783a40faf32ce5bc9a7f7817bae7ab4076db3279005155
```

which is byte-for-byte the `sid_sha256` recorded in **both** wedging files found
on the machine:

```
WCore-storage-00002d20-00000000000000f2.toml   owner_pid = 11552  (dead)
WCore-storage-00006314-00000000000000f2.toml   owner_pid = 25364  (dead)
both: sid_sha256 = 5b22ee051799cf8aa6783a40faf32ce5bc9a7f7817bae7ab4076db3279005155
      state      = "prepared"
      mtime      = 2026-07-26 08:30:24
```

Both are archived at `C:\p22-evidence\stale-leases-backup\` — copied, not
destroyed.

The tests are `#[ignore]`d and gated on `WAYLAND_SANDBOX_LIVE_WINDOWS=1`
(`storage.rs:626-631, 646-648`), so they only run in the explicit native Windows
acceptance suite. That suite ran on this box on 2026-07-26, which matches the
file mtimes. So the blast radius is **developer and CI machines that run the
native acceptance suite** — not end users who never run it. But on those
machines the effect is total and silent: every subsequent sandboxed command is
refused until a human deletes a file nobody knows to look for.

---

## 3. Repeatable procedure — red before, green after

Everything below ran inside **one** SSH session, so the logon is a constant and
the only variable is the presence of the lease file.

Logon context captured by the experiment itself:

```
LOGON-CONTEXT: shell_pid=43232 session_id=0
LOGON-CONTEXT: interactive=False
LOGON-CONTEXT: ssh_env=SSH_CLIENT,SSH_CONNECTION
```

That is precisely the condition the lore says cannot work.

Probe = the real product doing a real sandboxed spawn:

```powershell
wayland-core.exe swarm --workers 1 --worker-command "cmd.exe /c exit 0" `
                 --repo <throwaway repo> --base-branch main --timeout 60s
```

Cycle: clear the lease dir → probe; restore one archived stale lease → probe;
clear again → probe.

```
APPC label=clean-1     leases=0 succeeded=1 disabled=0 mismatch=0
APPC label=clean-2     leases=0 succeeded=1 disabled=0 mismatch=0
APPC label=clean-3     leases=0 succeeded=1 disabled=0 mismatch=0
APPC label=wedged-1    leases=1 succeeded=0 disabled=1 mismatch=1
APPC label=wedged-2    leases=1 succeeded=0 disabled=1 mismatch=1
APPC label=clean-final leases=0 succeeded=1 disabled=0 mismatch=0
```

`wedged-*` failure text, verbatim:

```
ERROR AppContainer real-spawn probe failed; sandbox disabled. If the failure is
transient (AV, disk contention), the probe re-runs after the negative-cache TTL.
error=sandbox child execution failed: AppContainer ACL lease SID/profile mismatch
in \\?\C:\Users\seand\AppData\Local\Wayland\Core\AppContainerLeases\v1\
WCore-storage-00002d20-00000000000000f2.toml
```

**4/4 green with no lease, 2/2 red with one, deterministic, session 0 throughout.**

Reproduction script: `.planning/intel/appcontainer-lease-wedge-probe.ps1`.
Run it with `powershell -NoProfile -EncodedCommand <UTF-16LE base64>`; a naive
quoted SSH one-liner mangles it, and PowerShell 5.1 on this box rejects `&&`.

### One honest wrinkle

The very first cycle I ran showed `clean-1` with `succeeded=0` but *no*
`sandbox disabled` and *no* lease mismatch — a different, unexplained one-off. It
did not reproduce across the four subsequent clean probes. I am recording it
rather than dropping it, but it is not evidence for the lore: the lore predicts
"backend unavailable", and that run showed the backend was not disabled.

---

## 4. What this does and does not license

**Established:**

* A real AppContainer spawn works over a session-0 non-interactive SSH logon on
  `seandesktop`, provided the lease directory is clean.
* An unreconcilable lease disables the sandbox permanently, with a message that
  reads like a platform limitation.
* The wedging leases were produced by the crate's own gated test helper writing
  into the production lease directory, proven by the SID-hash match.

**NOT established — do not over-read this:**

* I did not test other Windows hosts. The *mechanism* is host-independent
  (nothing in that code path consults the logon), but the *observation* is one
  box. A different host with a different policy could still fail for a real
  reason.
* I did not prove session 0 is irrelevant to **every** sandbox behavior. I proved
  it does not prevent a basic AppContainer spawn. Interactive-desktop-dependent
  behavior, and the hard-containment paths, were not exercised here.
* Anything gated on `WAYLAND_SANDBOX_LIVE_WINDOWS=1` was not run by this lane.

---

## 5. Recommended actions (not taken by this lane — outside its files)

Owner: whoever owns `wcore-sandbox`. Filed here rather than fixed because Phase
22 does not own that crate.

1. **HIGH — stop the test suite writing into the production lease directory.**
   `test_lease` should write beneath a temp root. As written, running the native
   acceptance suite can permanently disable the sandbox on that machine.
2. **HIGH — make lease recovery quarantine instead of wedging.** An
   unreconcilable lease should be moved aside and logged, not returned as a hard
   error forever. Fail-closed is right for a lease that might still be live;
   permanently refusing all execution because of an unparseable dead one is a
   denial of service against the user.
3. **MEDIUM — separate the two failure classes in the log line.** "sandbox
   disabled" plus a hint about AV and disk contention actively misdirects. A
   lease-reconciliation failure should say so and name the remedy.
4. **Update the briefs.** The "cannot observe over SSH / never conclude a red
   from an SSH run" note should be replaced with: *check the lease directory
   first; if it is empty, an SSH-observed red is a real red.* The current wording
   converts every Windows sandbox red into an unfalsifiable environment excuse.
