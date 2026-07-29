# lane/small-defects — running NOTES

Base: `0cba65d0aa909ea4be7365e802122a79941d8e48` (= `gh/plan/f20-unified-audit-repair`,
asserted against `git ls-remote gh` before any work).

Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-small-defects`

---

## Defect 1 — user model rendered under `"default"` while written under `WAYLAND_USER_ID`

### Premise: **HOLDS.**

- `crates/wcore-agent/src/bootstrap.rs:2024` — `let user_id = "default";` (verified by
  `/usr/bin/grep -n 'user_id = "default"' crates/wcore-agent/src/bootstrap.rs` → single hit at
  2024).
- Consumed at `:2140` (`s.corrections(user_id)`), `:2146` (`b.brief(user_id)`), `:2147`
  (`b.preferences(user_id)`).
- `crates/wcore-agent/src/engine.rs:3049` `resolve_user_model_user_id()` reads
  `WAYLAND_USER_ID`, falling back to `"default"`.
- Write path: `engine.rs:3205`, `:3444` set `user_model_user_id: resolve_user_model_user_id()`.
- `engine.rs:2764-2766` says outright: *"those two are not the same value"*.

So with `WAYLAND_USER_ID=alice`: writes go to bucket `alice`, the prompt renders bucket
`default`. The learned model never reaches the wire.

### Open questions
- Does any path WRITE `"default"` that another user then READS? (cross-user leak question)
- 23B-C3 bound the *correction* store id to the render site (`user_correction_store:
  (store, String)` pairing) — so corrections are consistent. The inferred brief/prefs are not.

---

## Defect 2 — `CEILING_IN_FLIGHT` unreachable

### Premise: **HOLDS, but it is already MEASURED AND DOCUMENTED at base, not unknown.**

`crates/wcore-cron/tests/in_flight_bound.rs` (already on the integration branch) asserts
exactly the brief's claim, with a known-positive control:
- `the_ceiling_constant_is_unreachable_by_any_input` — widest variant default is 2, ceiling 16.
- `the_runner_enforces_the_other_two_bound_fields_and_not_this_one` — source census over
  `runner.rs`: `max_in_flight` hits == 0, with `min_interval_secs` / `is_spent` as
  known-positive controls.

Correction to the brief's wording: `CEILING_IN_FLIGHT` **is** referenced once in product code
— `crates/wcore-cron/src/trigger.rs:99`, inside `TriggerBound::clamp_to`. What is referenced
zero times in `runner.rs` is `max_in_flight`, the field the ceiling bounds.

So the question the brief poses (dead constant vs unenforced bound) resolves onto the FIELD,
not the constant. TBD: read `runner.rs`.

---

## Defect 3 — two more `is_network_path` copies in `wcore-tools`

### Premise: **REFUTED (pending final grep proof).** Already consolidated at base.

`crates/wcore-config/src/network_path.rs` is the single implementation
(`has_unc_prefix` / `has_device_or_verbatim_prefix`), with the three legacy copies transcribed
into its test module as executable divergence evidence.

The two `wcore-tools` hits are **comments describing the removed copies**, not live code:
- `vision_tools.rs:826` — a comment in the test module saying the UNC tests moved.
- `media_intake.rs:925` — a doc-comment on a test explaining what the old local copy did.

TBD: prove there is no live `fn is_network_path` anywhere, with a live-instrument
known-positive in the same invocation.
