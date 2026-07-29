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

---

# MEASURED RESULTS (hetzner `hz/small-defects`, commit a897d5029bbac810f469b0a816981fd9aa1b3d79)

## Defect 1 — FIXED

Fix: `bootstrap.rs:2024` `let user_id = "default"` →
`let user_id = crate::engine::resolve_user_model_user_id();`. The 23B-C3 correction store's
paired id (`bootstrap.rs`, `engine.set_user_correction_store(store, user_id)`) rides the same
binding, so read-bucket and write-bucket move together by construction.

Test: `crates/wcore-agent/tests/user_model_identity_wire.rs`.

POSITIVE (post-fix):
    running 3 tests
    test a_named_user_does_not_render_the_default_buckets_model ... ok
    test the_prefix_render_expression_reads_an_empty_bucket ... ok
    test the_resolved_user_ids_model_reaches_the_wire ... ok
    test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

KNOWN-NEGATIVE (pre-fix render site restored verbatim by sed, then restored from a cp backup):
    test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

    the_resolved_user_ids_model_reaches_the_wire:
      "the user-context block is not in the outbound body at all"
    a_named_user_does_not_render_the_default_buckets_model:
      "the \"default\" bucket's learned style reached a session running as
       WAYLAND_USER_ID=alice-7QW2M9NONCE. That is one user's inferred traits in another
       user's system prompt, on its way to the provider."

The leaked line, extracted verbatim from the captured outbound provider body:
    style: formality=0.00, energy=0.50, terseness=0.00, emoji_use=0.00
That is the `"default"` bucket's fingerprint `[0.0, 1.0, 0.0, 0.0]` after the EMA fold,
rendered into the system prompt of a session whose identity was `alice-7QW2M9NONCE`.

### Cross-user leak: CONFIRMED, and it is worse than MEDIUM

Pre-fix the render site read `"default"` unconditionally while writes keyed
`$WAYLAND_USER_ID`. Any process running with the variable UNSET writes its inferred brief
into `"default"` — the exact bucket every NAMED user's prompt then rendered. The stores are
shared whenever the memory base is (`memory_base_dir()` = `$WAYLAND_MEMORY_DIR` or
`app_config_dir()`), which is the normal case for a channel gateway serving several humans
under one OS account. Demonstrated on the wire above, not argued.

## Defect 2 — dead constant. DELETED.

`CEILING_IN_FLIGHT` removed from `clamp_to`. Evidence it could never bind: `effective_bound()`
is the only bound consumer and always `clamp_to(default_bound())`; all seven variant defaults
hardcode `max_in_flight` (1, or 2 for Event) with no input path, so
`default.max_in_flight.min(16)` was `min(1_or_2, 16)` forever.

Note the brief's wording needed one correction: `CEILING_IN_FLIGHT` WAS referenced once in
product code (`trigger.rs:99`). What appears zero times in `runner.rs` is `max_in_flight`,
the field the ceiling bounded — that is the separate, unenforced-bound half, already closed
on the operator surface at base (`cron.rs` annotation) and pinned by a source census.

POSITIVE: `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

KNOWN-NEGATIVE (`Event` default temporarily widened 2 → 64):
    test result: FAILED. 2 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
    the_deleted_ceiling_changed_no_answer_for_any_variant:
      "event: deleting CEILING_IN_FLIGHT changed the answer for a persisted 17 against a
       variant default of 64. The deletion is NOT behaviour-preserving and must be reverted."
That is the tripwire the deleted constant used to carry, now carried by a test that can fail.

`FLOOR_INTERVAL_SECS` is non-binding for the same structural reason. NOT touched — out of
scope, and materially different in kind: it bounds a value partly derived from persisted
input, so it is one variant-author slip from being load-bearing.

## Defect 3 — REFUTED. Already consolidated at base.

Absence proof with a live instrument in the same invocation:
    KNOWN-POSITIVE  /usr/bin/grep -rn "fn has_unc_prefix" --include='*.rs' crates/
                    -> crates/wcore-config/src/network_path.rs:77   (1 hit)
    KNOWN-POSITIVE  /usr/bin/grep -rn "fn is_unc_path"    --include='*.rs' crates/
                    -> crates/wcore-tools/src/media_intake.rs:389   (1 hit)
    CLAIM           /usr/bin/grep -rn "fn is_network_path" --include='*.rs' crates/
                    -> 0 hits, rc=1
The three `is_network_path` hits inside `crates/wcore-tools/` are all COMMENT text describing
the removed copies (`vision_tools.rs:826`, `media_intake.rs:925`, `media_intake.rs:958`).
`media_intake.rs:389-390` is a two-line delegation to
`wcore_config::network_path::has_unc_prefix`; `executable_readiness.rs:563` delegates too.
