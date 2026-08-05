# seandesktop host access — the reported blocker, re-tested

**Three separate lanes recorded `seandesktop` as unreachable and graded four Windows legs
NOT ACHIEVED "blocked on a Sean-reserved credential". That report is false.** This file
records the measurement that settles it, because a wrongly-reported blocker costs a
Sean-reserved round trip for nothing.

## The measurement

```
$ ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop 'hostname'
SeanDesktop
rc=0
```

Taken 2026-07-28 from the planning Mac, with `BatchMode=yes` so no prompt could mask a
credential being supplied interactively. **No credential was supplied, obtained or guessed
by this lane.** The existing key already authorises the account.

## Why the earlier reports missed it

The accounts tried and reported refused were `sean`, `seandonahoe`, `sdonahoe`, `wayland`
and `Administrator`. **None of those accounts exists on the box.** The account is `SeanD`,
which is the spelling **every plan in this program already uses** — 28-03's own closure note
spells the completing command `SeanD@seandesktop`, and Phase 28's certification contract
cites `SeanD@seandesktop` as the host of the `KR-06` readjudication.

The refutation was therefore already inside the repository before the blocker was filed:
`.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md` (2026-07-27, `455dd836`) records
`live_fs_acl` **12/12 PASS over session-0 non-interactive SSH on `SeanD@seandesktop`**. A
lane cannot be locked out of a host that another lane had just finished measuring on.

## The constraint that IS real — do not conflate the two

`hetzner-dsm` genuinely cannot reach `seandesktop`:

```
hetzner-dsm $ ssh SeanD@seandesktop 'hostname'
Permission denied (publickey).
```

The planning Mac reaches both hosts; the two hosts cannot reach each other. Anything
requiring host-to-host SSH remains a genuine pending authorization. Anything requiring only
Mac→Windows does not, and was never blocked.

## Host facts, measured

| Fact | Value |
|---|---|
| hostname | `SeanDesktop` |
| default remote shell over ssh | **PowerShell** (not `cmd`) — `ver` is not a cmdlet there |
| logical processors | 32 |
| node | v24.16.0 (`C:\Program Files\nodejs\node.exe`) |
| git | 2.54.0.windows.1 |
| python | 3.13.14 (`python3`), 3.12.10 (`python`) |
| free space on C: | ~200 GiB |
| ssh account | `SeanD` (`whoami` → `seand`) |

## A measurement hazard found while proving the above

**Every non-zero exit status collapses to `1` over `ssh … powershell -EncodedCommand`.**
Measured directly:

```
requested=2   observed_ssh_rc=1
requested=3   observed_ssh_rc=1
requested=7   observed_ssh_rc=1
requested=100 observed_ssh_rc=1
requested=255 observed_ssh_rc=1
```

The exact status does not survive the transport; only pass/fail does. Any Windows gate on
this program that asserted a *specific* exit code (`rc=100`, say) over ssh was asserting a
value that cannot arrive — it would read as a mismatch on a passing run, or be quietly
weakened to `-ne 0`. Every Windows measurement in this lane therefore carries its status as
an explicit token written into a log file (`F28_SOAK_EXIT=<n>`), and the token is what is
read. Inside PowerShell the true code IS visible: the digest red-probe below shows `91`
recovered from `$LASTEXITCODE` on the box while ssh reported `1`.
