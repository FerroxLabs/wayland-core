# The restricting-SID quarantine token is refuted by measurement

Closes the last open candidate in wayland-core#415, which was split from #389 c1.
Measured 2026-09-03 on real Windows 10.0.26200.9168 (SeanDesktop), nine restricted-token
configurations plus two controls, all in ONE run.

## Verdict

**DOES NOT WORK.** The two halves of #415 c1 are strictly anti-correlated across every
configuration measured: every arm that closes the console cannot spawn `git`, and every arm
that can spawn `git` leaves the console open. There is no cell where both hold, so
**#415 c3 applies**: #389's labelling branch becomes the permanent answer rather than the
interim one.

## The control was alive in every run

A refutation is only worth what its control is worth. `[control-detached]` reproduces the
2026-09-01 baseline byte-for-byte, in the same run as every other arm, and a detector
self-test writes a nonce to the console and reads it back out of the screen buffer BEFORE
any arm runs:

```
DETECTOR_SELFTEST (write nonce to own console, read screen buffer back) = PASS
[control-detached] CONOUT_BEFORE=DENIED(6)  ATTACH_PARENT_PROCESS=SUCCEEDED
                   CONOUT_AFTER=OPEN  CANARY_WRITE=OK(36 bytes)
                   EXEC[git --version]=0  EXEC[git status]=0  EXEC[cmd /c echo ok]=0
                   WROTE_TO_USER_CONSOLE=true
```

`[lowil]` also reproduces its known row exactly (`ATTACH FAILED(5)`, every exec
`0xC0000142`). The signal is CONOUT$ writability plus a canary write read back from the
screen buffer -- never `GetConsoleProcessList`, which cannot discriminate here.

## The measurement

```
ARM                                  CONOUT_BEFORE ATTACH_PARENT ATTACH_BY_PID CONOUT_AFTER WROTE  gitver          gitstat         cmdecho
control-detached                     DENIED(6)     SUCCEEDED     SKIPPED       OPEN         true   0               0               0
lowil                                DENIED(6)     FAILED(5)     FAILED(5)     DENIED(6)    false  0xC0000142      0xC0000142      0xC0000142
restricted-everyone                  N/A           N/A           N/A           N/A          false  N/A (child_exit=0xC0000022)
restricted-everyone-users            DENIED(6)     FAILED(5)     FAILED(5)     DENIED(6)    false  SPAWN_FAILED(5) SPAWN_FAILED(5) SPAWN_FAILED(5)
restricted-everyone-users-logonsid   DENIED(6)     SUCCEEDED     SKIPPED       OPEN         true   SPAWN_FAILED(5) SPAWN_FAILED(5) SPAWN_FAILED(5)
restricted-user-only                 N/A           N/A           N/A           N/A          false  N/A (child_exit=0xC0000022)
restricted-everyone-users-usersid    DENIED(6)     FAILED(5)     FAILED(5)     DENIED(6)    false  SPAWN_FAILED(5) SPAWN_FAILED(5) SPAWN_FAILED(5)
restricted-nullsid                   N/A           N/A           N/A           N/A          false  N/A (child_exit=0xC0000022)
restricted-everyone-users+dacl       DENIED(6)     FAILED(5)     FAILED(5)     DENIED(6)    false  SPAWN_FAILED(5) SPAWN_FAILED(5) SPAWN_FAILED(5)
restricted-...-restrictedsid+dacl    DENIED(6)     FAILED(5)     FAILED(5)     DENIED(6)    false  SPAWN_FAILED(5) SPAWN_FAILED(5) SPAWN_FAILED(5)
restricted-dacl-first                N/A           N/A           N/A           N/A          false  N/A (CREATE_FAILED(1314))
denyonly-logonsid                    DENIED(6)     SUCCEEDED     SKIPPED       OPEN         true   0               0               0

REPRODUCTION_CONTROL (control-detached reaches the console) = ALIVE
```

## Why the premise itself fails, not just these nine arms

#415 proposed: grant the restricting SID on the quarantine tree and on everything `git`
loads, but not on the console. That grant is **not the binding constraint** -- it is already
satisfied and still insufficient:

```
[restricted-everyone-users]  (console closed)
  OPENFILE[cmd.exe]=OK  OPENFILE[git.exe]=OK  OPENFILE[ntdll.dll]=OK  OPENFILE[own probe.exe]=OK
  EXEC[git --version]=SPAWN_FAILED(5)
```

The restricted child can open `cmd.exe`, `git.exe`, `ntdll.dll` and its own image for
`GENERIC_READ|GENERIC_EXECUTE`, and `CreateProcess` still returns `ERROR_ACCESS_DENIED`.
Filesystem pass-2 is not the blocker, so no amount of ACLing the quarantine tree or git's
dependencies can rescue it. The denial is in the process-creation machinery.

Supporting: in every console-closed arm the child cannot `OpenProcessToken(TOKEN_QUERY)` on
itself. The one restricting set that fixed that -- adding the logon SID -- simultaneously
re-opened the console (its canary landed in the operator's screen buffer) and still could not
spawn git. The console's access check and the child's ability to function are gated on the
same grantee. The obvious inverse, `denyonly-logonsid`, leaves the console fully open.

## The gap, stated rather than dressed over

`SetTokenInformation(TokenDefaultDacl)` returned `1344` in all three forms attempted --
including on an UNRESTRICTED duplicate token, so that is a harness defect, not a restricted-token
limitation. "Restricted token plus fully working Chromium default-DACL plumbing" was therefore
not measured. The gap is materially narrowed by the `OPENFILE`/`SPAWN_FAILED(5)` result above
and by `restricted-everyone-users-logonsid`, where the child could query its own token, could
open every exe, and still could not spawn while leaking the console. It is the one cell left.

## Environment

OpenSSH logon session (logon SID `S-1-5-5-6-1545208099`), not an interactive desktop logon.
`AllocConsole` returned `FAILED(5)` because the sshd-spawned session already owns a real
console, so that one was used. Both controls reproduce their exact production rows, so the
console mechanics under test are the real ones.

## For the record on c2

Moot, since the candidate is refuted, but: `CreateProcessAsUserW` composes with job objects
(create suspended, `AssignProcessToJobObject`, resume), so core#393's process-tree containment
was never the obstacle. The token was.

Probe: standalone single-file Rust, no crates.io and no wayland-core workspace, built with
`rustc -O`. Nothing under `D:\gha\*` was touched.
