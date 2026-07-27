# 23B-04 — how to finish the journey a successor did not start

The multi-day journey is RUNNING. This file is the operational half of
`23B-04-LIVE-EVIDENCE.md`: exact commands, exact paths, and the two traps that
will otherwise cost three more days.

## The constants

```
PINNED SHA        0ed05322462e64cb44e2b80aa15b7357263b8187
BRANCH            lane/23B-04                (remote `gh` on the Mac, `origin` on the hosts)
POLICY            real-time-full             (23B-04-CLOCK-DECISION.md)
REQUIRED SPAN     259200 seconds, all three platforms
LINUX DAY ONE     2026-07-27T14:21:19Z    nonce 4ab44763e42849fd
WINDOWS DAY ONE   2026-07-27T23:54:26Z    nonce acb1d0b24b3fdecf
EARLIEST CLOSE    linux 2026-07-30T14:21:19Z, windows 2026-07-30T23:54:26Z
                  the journey as a whole: 2026-07-30T23:54:26Z
```

## Linux — already scheduled; verify only

```
host      hetzner-dsm
worktree  /root/wayland-23B-04        (detached at the pinned SHA, tracked tree clean)
state     /root/.f23-journey-linux/   (runlog.txt, day-one.json, journey.journal)
binary    /root/wayland-23B-04/target/release/wayland-core
harness   /root/wayland-23B-04/target/debug/deps/multi_day_journey_test-cd7922f357e39e40
timers    f23-journey-day2.timer (2026-07-28 14:25 UTC), f23-journey-day3.timer (2026-07-30 14:31 UTC)
```

Check the scheduled days landed, then verify:

```bash
ssh hetzner-dsm 'cat /root/.f23-journey-linux/scheduled.log; \
  grep -c "F23_04_DAY=" /root/.f23-journey-linux/runlog.txt'

cd /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23B-04
NONCE=$(/usr/bin/openssl rand -hex 8)
SHA=0ed05322462e64cb44e2b80aa15b7357263b8187
L=.planning/phases/23B-continuous-agency/evidence/23B-04-linux-verify.log
ssh -o BatchMode=yes hetzner-dsm "cd /root/wayland-23B-04 && \
  bash scripts/f23-multi-day-journey.sh --verify \
    --binary target/release/wayland-core --sha $SHA --nonce $NONCE \
    --harness \$PWD/target/debug/deps/multi_day_journey_test-cd7922f357e39e40" > "$L" 2>&1
rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "F23_04_JOURNEY=PASS platform=linux nonce=$NONCE" "$L"
```

If a timer missed, run the day by hand — it is idempotent and loses nothing:

```bash
ssh hetzner-dsm '/root/f23-journey-day.sh 2'   # or 3
```

## TRAP 1 — do not rebuild, and do not prune `target/`

The journey is pinned to a binary whose `--build-info` must report
`(source 0ed05322…)`. `cargo clean`, a `target/` prune, or a checkout to another
SHA followed by a rebuild will produce a binary that fails provenance with exit
`68`, and the harness path above will vanish. If the harness IS lost, rebuild it
at the pinned SHA and nothing else:

```bash
ssh hetzner-dsm 'cd /root/wayland-23B-04 && git rev-parse HEAD && \
  export PATH=/root/.cargo/bin:$PATH && \
  cargo build --release -p wcore-cli --bin wayland-core && \
  cargo test -p wcore-agent --test multi_day_journey_test --no-run'
```

The journey state in `/root/.f23-journey-linux/` is deliberately OUTSIDE the
worktree, so removing the worktree does not destroy the evidence. Remove the
worktree at the end of the phase; remove the journey root only after the verify
log is captured and committed.

## TRAP 2 — a verify run before the span elapses is SUPPOSED to fail

Before `2026-07-30T14:21:19Z`, `--verify` exits `72` and prints
`F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false`. That is the span gate working.
Do NOT lower `linux_required_real_span_seconds` in the decision record, do not
pass a smaller `--span-seconds`, and do not hand-edit the run log's timestamps.
The plan's own human-check is explicit: if the recomputed span is short of the
authorized threshold, the journey did not run and must be re-run rather than
re-described.

## Windows

```
host      SeanD@seandesktop            (PowerShell is the default remote shell)
worktree  C:\ferrox-win-23B04          (detached; created by this lane)
markers   C:\ferrox-win-23B04-DONE.txt, -rel.log, -harness.json, -harness.log
```

**Windows day one IS recorded**, at `2026-07-27T23:54:26Z`, against a binary
whose `--build-info` reports the pinned SHA. Days 2 and 3 are registered with
the Task Scheduler as SYSTEM:

```
f23win23B04day2   2026-07-29 06:58 local  (2026-07-28 23:58Z)
f23win23B04day3   2026-07-31 07:05 local  (2026-07-31 00:05Z)
```

Both call `C:\ferrox-win-23B04-resume.cmd <n>`. Check they landed, then verify
after `2026-07-30T23:54:26Z`:

```bash
ssh SeanD@seandesktop "Get-Content C:\Users\seand\.f23-journey-windows\scheduled.log; exit 0"

SHA=0ed05322462e64cb44e2b80aa15b7357263b8187
NONCE=$(/usr/bin/openssl rand -hex 8)
H='C:\ferrox-win-23B04\target\debug\deps\multi_day_journey_test-4f5c0f06a2c8fef9.exe'
L=.planning/phases/23B-continuous-agency/evidence/23B-04-windows-verify.log
ssh -o BatchMode=yes SeanD@seandesktop "Set-Location C:\ferrox-win-23B04; \
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f23-multi-day-journey.ps1 \
    -Verify -Binary target\release\wayland-core.exe -Sha $SHA -Nonce $NONCE \
    -Harness $H -Root C:\Users\seand\.f23-journey-windows; exit \$LASTEXITCODE" > "$L" 2>&1
rc=$?; test "$rc" -eq 0 && /usr/bin/grep -qF "F23_04_JOURNEY=PASS platform=windows nonce=$NONCE" "$L"
```

Never pipe the ssh command into a filter — `ssh host 'cmd' | grep` reports
grep's status, not the command's. And `powershell -File <missing.ps1>` exits
**0**, so the nonce-bound marker is the second, independent check; the status
alone is not a gate.

### TRAP 3 — `Start-Process` ignores the PowerShell location, and dies with the session

Two separate defects, both of which cost this lane real time.

`Set-Location` changes the PowerShell *provider* location, not the process's
working directory, and `Start-Process` inherits the latter — so `cargo` ran in
the ssh session's home directory and built nothing for ninety minutes. The
symptom is deceptive on a shared box: `Get-Process rustc` reports a healthy
count because ANOTHER lane is compiling, while the worktree's `target/` never
appears and the redirect target stays at zero bytes.

Even with `-WorkingDirectory` set correctly, the child was killed when the ssh
session ended. **Anything that must outlive a session on this box has to be a
scheduled task, not a `Start-Process` child.**

Verify a build is genuinely running with these two, never with a process count:

```powershell
(Get-Item C:\ferrox-win-23B04-rel.log).Length     # non-zero within ~30s
Test-Path C:\ferrox-win-23B04\target             # true
```

### TRAP 4 — a SYSTEM scheduled task has a different USERPROFILE

`schtasks /RU SeanD /IT` is "Interactive only" and does nothing at all unless
that user is logged on at the console — it reports `Last Result: 0` while
running nothing. Use `/RU SYSTEM /RL HIGHEST`.

But SYSTEM's `USERPROFILE` is `C:\Windows\System32\config\systemprofile`, so a
scheduled resume that lets the driver default its root would silently create a
SECOND, empty journey root in which **every day looks like day one**. The
resume command therefore passes `-Root C:\Users\seand\.f23-journey-windows`
explicitly, and that argument must not be dropped. The build script sets
`CARGO_HOME`, `RUSTUP_HOME`, `USERPROFILE` and `HOME` explicitly for the same
reason.

Both were verified rather than assumed by running the scheduled path against
day 1: it printed `F23_04_DAY_ALREADY_RECORDED=1`, exited 0, left the day count
at exactly 1, and created no second root.

## macOS

NOT ACHIEVED — nothing was run, so nothing is claimed in either direction.
Blocked on the compiled test harness rather than on the product binary.
The full measurement, the two corrections to the plan's reasoning, and the exact
route to unblock are in `23B-04-LIVE-EVIDENCE.md` under "macOS — OPEN, and
precisely why". Do not improvise a second binary resolver; the plan forbids it
by name.

## What Task 3 still needs

Unstarted by design — it depends on 23B-03, which was being built by a
concurrent lane. It needs: one pinned SHA in
`evidence/23B-04-pinned-sha.txt`; the host's HEAD asserted equal to it with
`git diff --quiet && git diff --cached --quiet` before any build step; the
locked all-features build; the CI-profile aggregate with verbatim counts; a
verdict for Success Criteria 2–6 traced to predecessor evidence rows; a
disposition for F23-02..F23-06; and `23B-04-D2-OWED.md`.

Note for whoever pins Task 3's SHA: it does NOT have to equal the journey's
pinned SHA. The journey is pinned so its binary cannot change mid-span; the
aggregate proof is pinned so the test counts name one tree. Pinning the
aggregate at a later SHA that includes 23B-03 is correct, and the journey's
verify logs remain valid evidence for the SHA they name.
