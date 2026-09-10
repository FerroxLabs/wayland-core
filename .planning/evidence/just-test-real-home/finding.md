# `just test` ran the suite against the real home, and the suite wrote into it

Found because `just push` failed twice at its test stage on the release tree
(receipts `/root/wl-01314-push.status`, EXIT=100, at `28644d17d` and at
`38f82b87f`) on a test that passed in every `tools/remote-proof.py` run the
same day, including a full-workspace run of 18,175 passing tests.

## The failure

    wcore-agent::stabilization_skill_memory cached_skill_edit_add_remove_applies_on_next_turn
    panicked at crates/wcore-agent/tests/stabilization_skill_memory.rs:198:5:
    assertion failed: current.contains("w11-beta")

## The A/B that decided it

Same commit (`38f82b87f`), same checkout (`hetzner-dsm:/root/w-f13/integ`),
same build, arms interleaved, `--retries 0`. The ONLY variable is whether
`WAYLAND_HOME` points at a fresh directory:

| arm | round 1 | round 2 | round 3 |
|---|---|---|---|
| real home (`env -u WAYLAND_HOME -u XDG_DATA_HOME`) | FAIL at :198 | FAIL | FAIL |
| isolated (`WAYLAND_HOME=<mktemp>/wayland-core`) | 4/4 pass | 4/4 pass | 4/4 pass |

## The mechanism, read off the host rather than inferred

The push host held `/root/.config/wayland-core/skills/auto-telemetry-zorbulate/`,
written by the auto-skill drafter during the push (`SKILL.md` mtime
`2026-09-10 13:37:59`). Its own body names its origin:

    > NOTE: This skill was auto-drafted from a streak of successful turns ...
    Signature: `telemetry-zorbulate`
    Evidence: 3 successful turns
    1. `zorbulate telemetry` -> 1 turn(s)
    2. `zorbulate telemetry` -> 1 turn(s)
    3. `zorbulate telemetry` -> 1 turn(s)

`cached_skill_edit_add_remove_applies_on_next_turn` runs the prompt
`"zorbulate telemetry"` exactly three times. The real-home arms of the A/B
REWROTE that file again (mtime `1789047479.84` -> `1789047751.72`), so the
suite writes into the operator's home on every run, not once.

Why it then fails: `crates/wcore-skills/src/loader.rs:33` documents the
priority order `bundled -> MCP -> user -> project -> additional -> legacy`, and
`clamp_to_budget` keeps entries in that order until the listing budget runs
out (1,310 chars for this test's unlisted 32k model). The drafted skill sits in
the USER tier alongside the host's other 83 ambient skills; the test's `w11-beta`
sits in the PROJECT tier, below them, and is clamped out of the
`Current skill inventory` block the assertion reads.

## Why every other harness was already immune

- CI runs the suite inside a clean container (`just test-ci`).
- `tools/remote-proof.py:89` exports `WAYLAND_HOME="$root/test-homes/$nonce/wayland-core"`.
- `just test` — the stage `just push` runs — exported nothing.

`WAYLAND_HOME` alone is sufficient: `wcore_skills::paths::user_skills_dir()`
resolves through `app_config_dir()` -> `wayland_config_dir()`
(`crates/wcore-config/src/config.rs:4313`), which reads `WAYLAND_HOME` first,
and `wayland_home_skills_dirs()` roots the second tier at `$WAYLAND_HOME` too.

## What this does NOT fix, stated so it is not over-read

1. **The static gate is blind to this class.** `scripts/check-test-state-dir-guards.py`
   reported `OK: 2140 files scanned, no unguarded state-directory reach` on this
   exact tree. It looks for DIRECT calls to the state-directory resolvers in test
   code; here the reach happens inside production code the test drives through
   `AgentBootstrap`. A test run with plain `cargo test` or `cargo nextest run`
   outside the recipe will still write into the caller's real home.
2. **The Windows `test` recipe is unchanged.** The same exposure exists on a
   Windows developer machine; it was not changed here because it could not be
   verified on Windows at the time without contaminating a timing measurement
   running on the only Windows host.
3. **The drafted skill on the push host was left in place deliberately**, so the
   fix is verified WITH the pollution present rather than after removing it.
