# THE PROBLEM — wayland-core RC — 2026-08-05

Branch `plan/f20-unified-audit-repair` @ `d06935d5`. Workspace 0.12.26. Nothing tagged.

Read this file first. `HANDOFF-2026-08-05-RC-CURRENT.md` has the full history;
this one exists to say, without hedging, what is broken and what fixes it.

---

## THERE IS ONE BLOCKER. IT IS THIS.

**A Windows worktree cleanup fails with `ERROR_SHARING_VIOLATION` and the code
never retries, so the whole CI leg goes red.**

```
crates\wcore-swarm\tests\worker_runtime_limits.rs:55
transaction cleanup: worktree io: io:
The process cannot access the file because it is being used by another process. (os error 32)
```

Everything else on Windows passes: **13551 of 13552**. This one test is the leg.

### Why it happens

`os error 32` is Windows' standard transient. Another process — an AV scan, a
just-exited child still closing handles, the search indexer — briefly holds a
file inside the directory being removed. It clears in milliseconds.

### Why it is not already fixed — and this is the infuriating part

**Every piece of the retry machinery already exists. Nobody calls it twice.**

| file:line | what it does |
|---|---|
| `crates/wcore-sandbox/src/directory_authority.rs:791` | `remove_open_dir_all` returns `Err(Box<(SandboxError, Self)>)` — it hands the authority **back** inside the error. You only design that signature so a caller can try again. |
| `crates/wcore-swarm/src/worktree.rs:236` | its own refusal message literally says *"transaction cleanup refused and its reservation held **for retry**"* |
| `crates/wcore-swarm/src/dispatch.rs:623` | calls `manager.release_transaction(&workspace)` **exactly once**, and turns any error straight into `WorkerHandle::failed("transaction cleanup: {error}")` |

The design anticipates the retry. The call site never performs it.

### Why it looked harmless until now

It is intermittent — roughly **1 in 16** under load. Windows was green four
consecutive runs *with this defect present*, because nextest's retries absorbed
it. On run 30971991803 it lost all three tries and took the leg down.

I had it filed as "not blocking" on the strength of those four green runs. That
was wrong, and it is the third time this cycle an intermittent failure in a
retry-covered path got classified as benign right up until it wasn't. **A flake
that can fail a release gate is a blocker; it just fails on a dice roll.**

---

## THE FIX

Bounded retry around `dispatch.rs:623`. Small change, one call site.

**Two decisions it needs, both real:**

1. **Which errors are retryable.** `SwarmError::WorktreeIo(String)` flattens the
   OS error code into text today, so you cannot match on errno without widening
   the error type. Retrying the security refusals (`"refused cleanup outside
   owned transaction root"`) would be wrong. Either widen the type to carry the
   `io::Error`, or gate the retry to the specific transient.
2. **Backoff and bound.** The violation clears in milliseconds — something like
   3 attempts at 50/150ms. **After the attempts are spent it must fail exactly
   as it does today.** Do not make it swallow a persistent failure.

**How to verify it — this part matters:**

```
# on SeanDesktop, D:\wincheck
# 32 CPU burners, then N iterations of the failing test
cargo nextest run -p wcore-swarm \
  -E 'test(multi_worker_output_exhaustion_fails_without_retaining_buffers)'
```

Base rate is ~1/16 under load. **A clean 0/30 after the fix proves nothing.**
Run several hundred iterations, or raise the contention to lift the base rate
first and measure before/after against the same harness.

---

## WHAT IS ALREADY DONE — do not redo any of it

Six substantive fixes landed this cycle, each proven rather than asserted:

| what | proof |
|---|---|
| Windows AppX profile RPCs out of the machine-wide mutex | measured 140→68 ms/op at 24-way; probe 3381→1188 ms |
| Linux F01 gate encoded pre-#170 semantics | **ablation** — revert the enforcement, corrected gate goes red |
| macOS voice ×3 honestly gated + given a real runner | hosted VM proven incapable (rms=0.0 both arms) |
| Windows packaged gate could NEVER pass on Windows | **sha256 of the hashed receipt matched** — cryptographic ID |
| credentials: lock could claim a heartbeat it lacked | real silent-degradation defect; also the discriminator for macOS |
| macOS lock test rescaled 10x | **ablation** — heartbeat off ⇒ red try 1; on ⇒ 10/10 green |

**The product itself was driven live** (release binary, hetzner): init,
project-context, models list all work; missing API key names three remedies and
**exits 1**; no keyring warns that saving will be REFUSED not silently
cleartexted; `index status` reports `semantic status=unavailable` rather than
faking it. It works.

---

## CURRENT CI — run 30971991803, head d06935d5

```
gh run view 30971991803 -R FerroxLabs/wayland-core
```

- **Windows: FAILURE** — 13551/13552, the sharing violation above
- **Linux: was in flight at handoff** (green on the two prior runs)
- **macOS: was in flight at handoff** (carries the heartbeat fix + rescale; its
  previous red was the lock test, now addressed)

**Do not push to this branch while a run is in flight** — it cancels it. I did
exactly that once this session and lost two leg verdicts.

---

## THE PATH TO A TAG

1. Fix + verify the sharing-violation retry (above). **This is the only known blocker.**
2. Confirm Linux and macOS verdicts from run 30971991803, or a fresh run.
3. All three green ⇒ tag `v0.12.26-rc.1`.

Tagging is reserved to Sean and authorised only over a genuinely green CI.

---

## FIVE THEORIES ALREADY REFUTED — do not re-walk them

1. Windows stall is an O(N²) recovery sweep — **false**, sweep is 8% of the section
2. CPU starvation causes it — **false**, 32 burners move the RPC 14.3→15.6 ms
3. `tool_formatter` failures are unrelated to the sandbox — **false**, they fail at 15.067s, the probe guard to three decimals
4. The sharing violation cannot be reproduced — **false**, ~1/16 under load
5. The voice gate changed the test schedule and caused the macOS red — **false**, that gate was in the commit that PASSED

Three of those five were my own confident claims, killed by measuring the
specific thing. Measure first.

---

## ENVIRONMENT TRAPS THAT COST TIME

- `COPYFILE_DISABLE=1 tar` from the Mac — a plain tar put **4381 `._*` files**
  into `D:\wincheck` and broke `wcore-plugin-wasm`'s WIT parsing, looking
  exactly like a real build regression
- Windows extract: `& "$env:SystemRoot\System32\tar.exe" -xzf "D:/x.tgz"` (forward slashes)
- Remote PowerShell via a scp'd `.ps1`, never inline nested quoting
- `rc=$?` after a pipe captures the last command — use `${PIPESTATUS[0]}`
- `rtk` mangles output; `rtk proxy` for anything quoted as evidence
- NEVER cargo on the Mac. Linux → `ssh hetzner-dsm` `/root/wincheck`. Windows →
  `ssh SeanD@seandesktop` `D:\wincheck`, never `C:\actions-runner-*`
- The `/private/tmp` worktree does not survive a reboot. Everything is pushed;
  rebuild with `git worktree add --detach <path> gh/plan/f20-unified-audit-repair`
