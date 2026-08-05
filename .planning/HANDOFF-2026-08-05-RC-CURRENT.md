# HANDOFF — wayland-core RC — 2026-08-05 (CURRENT; supersedes the overnight file)

Branch `plan/f20-unified-audit-repair` @ **`1a102ad9`**. PR #257 → main.
Workspace **0.12.26**. Nothing tagged.

This file supersedes `HANDOFF-2026-08-04-RC-OVERNIGHT.md`, which is still worth
reading for §"refuted theories" but has four rounds of correction layered on it
and is confusing as a status document. Trust THIS file for state.

---

## 1. WHERE IT IS — one command

```
gh run view 30969146948 -R FerroxLabs/wayland-core
```

At the time of writing: **Windows green (4th consecutive run)**, Linux and
macOS still executing.

Prior verified state, run 30934350294 and 30961266169:

| leg | result |
|---|---|
| Windows | **13552 / 13552 pass** — main suite AND the packaged driver gate |
| Linux | **green** |
| macOS | 13869 / 13870 — one lock test, fixed in this head, verdict pending |

**If all three are green: tag `v0.12.26-rc.1`.** That is the only remaining
step, and it was deliberately left for Sean rather than done unattended,
because it is the one irreversible action.

**If macOS is red again:** read the failing test name FIRST. If it is
`exclusive_lock_steals_a_stale_holder_but_never_a_heartbeating_one` again then
the 10x rescale was insufficient and the answer is to route the macOS leg to
`sean-mac-arm64` (online, already used by `macos-native-suites.yml`) rather
than widening further — widening again would start eroding the invariant.

---

## 2. WHAT LANDED, AND WHY EACH ONE IS TRUSTWORTHY

Five substantive commits. Each has its evidence in its own commit message.

| commit | what | how it was proven |
|---|---|---|
| `4d46ee0f` | Windows: AppX profile RPCs out of the machine-wide mutex | measured 140→68 ms/op at 24-way; probe 3381→1188 ms |
| `94e00653` | Linux F01: gate encoded pre-#170 semantics | **ablation** — reverting the #170 enforcement turns the corrected gate red |
| `9044c1ae` | macOS voice ×3: honest capability gate + a real runner | hosted VM proven incapable (rms=0.0 both arms); gate asserts executed count |
| `4864b5eb` | Windows packaged gate could NEVER pass | **sha256 of the hashed receipt detail matched** — cryptographic ID, not inference |
| `1c3bad12` | credentials: a lock must never claim a heartbeat it lacks | real silent-degradation defect; also the discriminator that settled macOS |
| `1a102ad9` | macOS lock test rescaled 10x | **ablation** — heartbeat disabled ⇒ red on try 1; intact ⇒ 10/10 green |

Plus `3a32f90a` (cross-process probe cache — real load reduction, did NOT close
the Windows stall on its own) and `ae58f8cd` (anti-vacuity annotation the
repo's own linter correctly demanded).

---

## 3. LIVE PRODUCT EXERCISE — done, it works

Not the test suite. The actual release binary, built and driven on hetzner:

| check | result |
|---|---|
| `--version` | `wayland-core 0.12.26` |
| `init` | writes `.wayland/config.toml` + `WAYLAND.md` |
| `project-context` | walks up, finds and prints it |
| `models list` | real current catalog |
| no API key | names three remedies, **exit 1** |
| bogus provider | lists every valid provider, **exit 1** |
| no keyring | warns that saving will be REFUSED, not silently cleartexted |
| `index status` | reports `semantic status=unavailable` with the reason |

That last row is the point: it declines to claim a capability it does not have.

Build needs `WAYLAND_BUILD_SOURCE_SHA` set to 40 lowercase hex (the release
guard refuses an unattributable build — correct behaviour, not a defect).

---

## 4. OPEN — none of these block the RC

1. **`ERROR_SHARING_VIOLATION` in worktree cleanup (Windows).** Real,
   reproducible ~1 in 16 under 32 CPU burners on SEANDESKTOP. Retries absorb
   it; Windows is green with it present. Fix shape is known and the machinery
   already exists: `remove_open_dir_all` hands the authority BACK inside its
   error and `worktree.rs:236` says "held for retry", but `dispatch.rs:623`
   calls `release_transaction` exactly once. Two decisions needed — which
   errors are retryable (`WorktreeIo(String)` flattens the errno today) and the
   backoff bound. Verify with a few hundred iterations, not 30: at a 1/16 base
   rate a clean 0/30 proves little.
2. Startup log spew (task #147), log rotation (#155), and the other pending
   items in the task list. All pre-existing, none release-gating.

---

## 5. REFUTED — do not re-walk these

Five theories died against measurement in this cycle. Each cost real time.

1. **Windows stall is an O(N²) recovery sweep.** False. The sweep is
   ~0.47 ms/lease and 8% of the critical section. It came from arithmetic that
   happened to match.
2. **CPU starvation causes the Windows probe stall.** False. Under 32 burners
   the raw profile RPC moves 14.3 → 15.6 ms/op.
3. **The `tool_formatter` failures are unrelated to the sandbox.** False —
   they fail at 15.067s, the probe's guard to three decimals. I concluded this
   from a local PASS; absence of a local repro is not evidence of another cause.
4. **The sharing violation cannot be reproduced.** False. ~1/16 under load. I
   claimed it without trying.
5. **My voice gate changed the test schedule and caused the macOS red.** False.
   That gate was already in the commit that PASSED.

**Method note worth keeping:** every one of those was resolved by measuring the
specific thing, and three of them were my own confident claims. Measure before
concluding, and re-measure before building on a conclusion.

---

## 6. ENVIRONMENT GOTCHAS THAT COST TIME

- **`COPYFILE_DISABLE=1 tar`** when shipping from the Mac. A plain macOS tar put
  **4381 AppleDouble `._*` files** into `D:\wincheck`; they break
  `wcore-plugin-wasm`'s WIT parsing and look exactly like a real build
  regression. Clean with
  `Get-ChildItem -Recurse -Force -Filter "._*" | Remove-Item`.
- Windows: extract with `& "$env:SystemRoot\System32\tar.exe" -xzf "D:/x.tgz"`
  (forward slashes; MSYS tar mangles backslashes).
- Run remote PowerShell from a `.ps1` you scp over — inline nested quoting
  through ssh breaks.
- `rc=$?` after a pipe captures the LAST command, not the binary. Cost me a
  wrong "exit 0 on error" reading. Use `${PIPESTATUS[0]}`.
- `rtk` mangles output; use `rtk proxy` for anything quoted as evidence.
- NEVER cargo on the Mac. Linux → `ssh hetzner-dsm` (`/root/wincheck`,
  `export PATH=$HOME/.cargo/bin:$PATH`). Windows → `ssh SeanD@seandesktop`,
  `D:\wincheck`, never `C:\actions-runner-*`.

---

## 7. RESUME

```
cd /Users/seandonahoe/dev/waylandcore
claude --resume 11929102-d58a-47e9-9644-0e9d530b58c4
```

The worktree used this cycle lives under `/private/tmp/...` and may not survive
a reboot. That costs nothing — everything is pushed. The non-tmp checkout at
`waylandcore-gsd-planning/wt-f20-unified-audit-repair` is at `4018e5c3` and is
**stale**; do not mistake it for current state. GitHub is authoritative.

Reserved to Sean: merging to main, opening PRs, closing issues. Tagging was
authorised for a genuinely green CI only.
