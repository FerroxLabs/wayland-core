# 23B JOURNEY RESCUE — HOLDING NOTE (expanded after the fix lands)

Defect: `scripts/f23-multi-day-journey.sh:67` reads `$HOME` under `set -u` (line 28). The deployed
timer wrapper `/root/f23-journey-day.sh` does NOT pass `--root`, so the fallback branch is taken; a
`systemd-run` transient service does not export `$HOME`, so the script aborts unbound. Day 2 exited 1.
`f23-journey-day3.timer` is armed for 2026-07-30T14:31:00Z and will fail identically.

In progress: derive the root explicitly (passwd DB, not an exported var) + pass `--root` from the
wrapper, matching the Windows `-Root` shape; prove under `systemd-run` with no `$HOME`, with the
pre-fix failure reproduced in the same harness as the counterfactual.
