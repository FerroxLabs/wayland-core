# The probe/supervisor drift differential — four arms, one headless host

Host `hetzner-dsm`, worktree `/root/wayland-27c2b`, `DISPLAY` and `WAYLAND_DISPLAY`
both UNSET, `camofox-browser` **not** on PATH. Lane head `e8439ae8`; pre-fix tree read
straight out of `d622cb09` (integration base) with `git show`.

**The drift being simulated** is one word: `supervisor.rs:72`'s default sidecar name
renamed `camofox-browser` → `camoufox-browser` (a plausible typo-correction — note the
shipped name really is missing the `u`). A fake executable was planted at
`/tmp/27c2b-bin/camoufox-browser` so the renamed program **resolves**, i.e. the machine
genuinely has a working browser under the new name.

All four arms ran the same scratch harness (`examples/drift_probe.rs`, **not committed**,
deleted afterwards) which prints the supervisor's config and the probe's verdict side by
side.

| Arm | tree | supervisor would spawn | resolves? | probe verdict | withdraws capability? | correct? |
|---|---|---|---|---|---|---|
| 1 | mine | `camofox-browser` | **false** | `Unavailable` | **true** | ✅ headless truth |
| 2 | mine + drift | `camoufox-browser` | **true** | `Ready{via:"camoufox-binary"}` | **false** | ✅ follows the config |
| 3 | **pre-fix** + drift | `camoufox-browser` | **true** | `Unavailable` | **true** | ❌ **DEFECT** |
| 4 | mine, duplication re-introduced | — | — | — | — | known-negative |

## Arm 3 is the defect, live

```
SUPERVISOR_WOULD_SPAWN=Some("camoufox-browser")
SUPERVISOR_PROGRAM_RESOLVES=true
PROBE_VERDICT=Unavailable(Unavailable { reason: "no browser backend can start: `camofox-browser` does not resolve on PATH and no sidecar answered http://127.0.0.1:1/health", ... })
PROBE_WITHDRAWS_CAPABILITY=true
```

The supervisor would have spawned a browser that **is installed and would have
started**. The probe hunted its own stale literal, found nothing, and withdrew
`capabilities.browser_suite` from the Desktop app. That is the *under-advertising*
direction of `27-C2(b)` — the false-negative class the prior lane's cross-audit panel
unanimously flagged and which `Indeterminate` was introduced to prevent. It survived in
this one path because the two program resolutions were separate literals.

Note the reason string is doubly wrong here: it names `camofox-browser`, a binary the
engine would never launch, so an operator reading it installs the wrong package.

**And the pre-fix suite did not notice:**

```
running 5 tests
.....
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 84 filtered out
```

That is the third assertion §6b-ii demands — *the old shape would have missed it* —
demonstrated behaviourally rather than asserted. The pre-fix docstring at
`liveness.rs:85-86` claimed the probe performed "the same resolution
`SupervisorConfig::local_camoufox` performs". Arm 3 is that claim being false with the
suite green.

## Arm 2 — the same drift, no defect

```
SUPERVISOR_WOULD_SPAWN=Some("camoufox-browser")
SUPERVISOR_PROGRAM_RESOLVES=true
PROBE_VERDICT=Ready { via: "camoufox-binary" }
PROBE_WITHDRAWS_CAPABILITY=false
```

Suite under the drift: `7 passed; 0 failed; 0 ignored; 0 measured; 84 filtered out`.

The fix does not *detect* this drift, it makes it **unrepresentable** — there is now one
literal, so renaming it moves the probe with it. Nothing to catch.

## Arm 4 — known-negative for my guard, verbatim

Mutation: `camoufox_program` reverted to ignoring the config and returning its own
literal (`let _ = cfg; Some("camofox-browser")`) — i.e. exactly the pre-fix shape.

```
thread 'liveness::tests::the_probe_reads_the_program_out_of_the_supervisors_own_config' panicked at crates/wcore-browser/src/liveness.rs:320:9:
assertion `left != right` failed: the two arms produced the same sidecar program, so this test compares a state with itself
  left: Some("camofox-browser")
 right: Some("camofox-browser")

thread 'liveness::tests::camoufox_program_honours_the_operator_override' panicked at crates/wcore-browser/src/liveness.rs:264:9:
assertion `left == right` failed
  left: Some("camofox-browser")
 right: Some("/opt/custom/camoufox")

failures:
    liveness::tests::camoufox_program_honours_the_operator_override
    liveness::tests::observe_only_mode_has_no_binary_to_look_for
    liveness::tests::the_probe_reads_the_program_out_of_the_supervisors_own_config

test result: FAILED. 4 passed; 3 failed; 0 ignored; 0 measured; 84 filtered out
```

Restored, re-ran: `7 passed; 0 failed; 0 ignored; 0 measured; 84 filtered out`.

Worth noting *which* assertion fired first in the new guard: the `assert_ne!` that the
two arms are different experiments. Had I written the guard as a same-string comparison
it would have passed on the mutated tree, because under the mutation both arms return
the identical stale literal and a `==` check between two copies of one wrong value is
satisfied. That guard exists because of §6a-i.

## Both directions, per §3b-iii

- **Can it fail?** Arm 4 — three named failures with verbatim output.
- **Can it pass?** Arm 2 — the state it claims to detect was constructed on a real host
  and the gate went green. Arm 1 also passes on the un-mutated headless truth, so
  neither polarity is stuck.

## Counts, read back from an unproxied cargo

Every figure above came from `/root/.cargo/bin/cargo` invoked by absolute path, and each
prints `0 ignored` and `84 filtered out` — the two fields §3b says the `rtk` proxy
strips. Their presence is the evidence the proxy was not in the path.
