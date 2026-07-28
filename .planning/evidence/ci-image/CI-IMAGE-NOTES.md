# CI-IMAGE-NOTES — running notes, lane/ci-image

Base `plan/f20-unified-audit-repair` @ `0b5182ef`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-image`.
Append after every measurement, per LANE-BRIEF §6b-i. Nothing here is a claim
until a named run is quoted next to it.

---

## T0 — inherited facts (read, not measured by me)

From `.planning/RED-68-TRIAGE.md` (lane `red-68`) and `.planning/CI-TRIAGE.md`
(lane `ci-triage`). I treat these as *prior* measurements to be confirmed by a
real CI run, not as established for my own report.

| class | n | inherited claim |
|---|---|---|
| C1 | 23 | `python3` absent from CI image |
| C3 | 20 | `bubblewrap` absent — and installing it is measured NOT to fix it |
| C4 | 13 | descendant reaping; container, NOT parallelism, NOT the missing `ps` |
| C2 | 6 | `ps` (`procps`) absent from CI image |
| C5 | 3 | container timing / provenance |
| K1/S1/R1 | 3 | already-known / stale test / the contract-digest guard (only true red) |

The image is built inline in `ci.yml` at line 304-312:

```
FROM rust:1.95-slim-bookworm
RUN apt-get install ... libdbus-1-dev libseccomp-dev libssl-dev libasound2-dev \
                       pkg-config mold ca-certificates git
```

No `python3`, no `procps`, no `bubblewrap`.

### The bubblewrap trap, already measured by lane/ci-triage (CI-TRIAGE §2)

Installing `bubblewrap` changes the failure mode and not the outcome. In an image
WITH bwrap installed, on a near-exact match of the runner:

| docker flags | result |
|---|---|
| none (what CI uses today) | `Creating new namespace failed: Operation not permitted` |
| `seccomp=unconfined` alone | same |
| `apparmor=unconfined` alone | same |
| both unconfined | same |
| `--cap-add SYS_ADMIN` | `Failed to make / slave: Permission denied` |
| `SYS_ADMIN` + `apparmor=unconfined` | `pivot_root: Operation not permitted` |
| **`SYS_ADMIN` + `seccomp=unconfined` + `apparmor=unconfined`** | **rc=0** |
| `--privileged` | rc=0 |

And even with the working grant, lane/ci-triage RAN a dedicated job on
`ubuntu-latest`: bwrap could create a namespace, but the **engine's own gate
execution against the bind-mounted `/work` still failed** (`expected
LandingReport::Landed, got None`; 3 candidates built vs 1 on the build host,
`tokens=0+0` = scripted provider exhausted = `cand-0`'s gate did not pass).
So the SYS_ADMIN recipe is necessary and NOT sufficient. That job was removed
rather than shipped red; recoverable from `git show 189599ca -- .github/workflows/ci.yml`.

### Forbidden / constrained by the lane prompt

- `WAYLAND_ALLOW_NO_SANDBOX=1` is forbidden — it converts a sandbox test into a
  test that proves nothing.
- A skip must be **loud and counted**, and the count must actually count. The
  prior lane's counted skip counted nothing: `record_loud_skip` wrote to a
  relative `"target"` path, a test's cwd is the crate root, the open failed and
  `if let Ok` swallowed it. Repaired there via `CARGO_TARGET_TMPDIR` + panic.
  I must not reintroduce that shape.
- `is_available()` is `which::which("bwrap").is_some()` — **presence, not
  capability**. A presence check reports READY in exactly the container where
  the sandbox cannot work.

---

## T1 — what I intend to establish

1. `python3` + `procps` into the inline Dockerfile → **verify by a real CI run
   id**, reading executed counts back, not by reasoning.
2. The bubblewrap 20: privileged-container vs qualify-or-skip-on-a-real-probe.
   Decide with the §4 cross-audit panel, record the dissent.
3. The 13 reaping failures: name the container mechanism, or state precisely
   that it is still unknown. A wrong mechanism here is worse than none because
   process containment is a security property.

## T1a — traps I am carrying into my own instruments

- A suite can exit 0 having run **zero** tests (four flavours measured). Run
  targets **by file**, never by filter; read `N passed` back.
- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline
  returns empty on this Mac (zsh).
- `rtk` silently filters `git log` and drops merge commits at rc=0, and
  `wc -c < file` reads 0 through the proxy. Use `/usr/bin/git`, `/usr/bin/wc`.
- `gh run view --job <id> --log` is intercepted by `rtk` (`rtk: Run ID required`,
  rc=1). Working path: `gh api /repos/<owner>/<repo>/actions/jobs/<id>/logs`.
- Push **once** and poll. Re-pushing supersedes my own queued run.

---

## LOG

- **T0** worktree created, `lane/ci-image` @ `0b5182ef`, toplevel verified as the
  lane path (NOT `/Users/seandonahoe/dev/waylandcore`). Brief + both prior triage
  reports read. Nothing measured by me yet. This file committed before any
  investigation, per §6b-i.
