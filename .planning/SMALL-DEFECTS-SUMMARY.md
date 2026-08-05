# lane/small-defects — SUMMARY

Branch `lane/small-defects`, forked from `plan/f20-unified-audit-repair` at
`0cba65d0aa909ea4be7365e802122a79941d8e48` (SHA asserted against `git ls-remote gh` before
any work). Shared-file fence (`wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`): **untouched**,
verified by `git diff --stat $(git merge-base HEAD plan/f20-unified-audit-repair)` over those
two paths — empty.

Verdict: **2 defects real and fixed, 1 refuted.** One finding upgraded above its briefed
severity, with wire evidence.

---

## Defect 1 — user model rendered under the wrong id — PREMISE HELD, FIXED

Every element of the premise verified in source before acting:

| claim | verified |
|---|---|
| `bootstrap.rs:2024` hardcodes `"default"` | yes — sole hit for `user_id = "default"` in the file |
| used at the brief/prefs fetch | yes — `:2139` corrections, `:2146` brief, `:2147` preferences |
| writes use `resolve_user_model_user_id()` | yes — `engine.rs:3205`, `:3444`; the write itself at `engine.rs:5440` |
| that function reads `WAYLAND_USER_ID` | yes — `engine.rs:3049` |
| `engine.rs` documents the divergence | yes — `:2764-2766`, *"those two are not the same value"* |

**Fix.** `bootstrap.rs` now binds
`let user_id = crate::engine::resolve_user_model_user_id();`. This is the whole repair, and
it is deliberately one binding: the 23B-C3 correction store's paired id
(`engine.set_user_correction_store(store, user_id)`) reads the *same* local, so read-bucket
and write-bucket move together by construction rather than by two call sites agreeing.
Stale doc comments in `engine.rs` that asserted the divergence were corrected — leaving them
would have told the next reader a defect still existed.

**Completeness check.** Audited every production `brief`/`preferences`/`observe`/`corrections`
call site in the workspace. Exactly three exist outside tests: the render site (fixed),
`engine.rs:5440` (already resolved), and `slash/usermodel.rs` (uses the paired id, so it
rides the fix). No other hardcoded `"default"` identity remains.

### Proof — at the outbound provider body, not at a store

`crates/wcore-agent/tests/user_model_identity_wire.rs`, modelled on the 23B-C3 lane's
`user_model_correction_wire.rs`: a real `AnthropicProvider` POSTing to a `wiremock` server,
asserted against `received_requests()`. Nothing between the assertion and the socket is
mocked. A store-level assertion passes throughout this defect — the value was always *in* a
store, just the wrong one.

**KNOWN-POSITIVE** (unproxied `cargo`, hetzner):

    test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

**KNOWN-NEGATIVE** — pre-fix render site restored verbatim, same test binary:

    test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out

    ---- the_resolved_user_ids_model_reaches_the_wire stdout ----
    panicked at crates/wcore-agent/tests/user_model_identity_wire.rs:348:5:
    the user-context block is not in the outbound body at all.

    ---- a_named_user_does_not_render_the_default_buckets_model stdout ----
    panicked at crates/wcore-agent/tests/user_model_identity_wire.rs:422:5:
    the "default" bucket's learned style reached a session running as
    WAYLAND_USER_ID=alice-7QW2M9NONCE. That is one user's inferred traits in
    another user's system prompt, on its way to the provider.

**THE OLD SHAPE WOULD HAVE MISSED IT** — `the_prefix_render_expression_reads_an_empty_bucket`
evaluates the pre-fix expression (`brief("default")`) and the fixed one against the *same*
store and requires them to disagree. It is the only test here that passes in both worlds by
design, because it measures the two expressions rather than the product's choice between
them; without it the other two would pass against broken code as easily as against the fix.

### `WAYLAND_USER_ID` is process-global — the flake family was not fed

The variable is set **once**, via `Once`, to a single value, and never unset or changed.
Discrimination comes from *which bucket is seeded*, not from toggling the environment — so
there is no toggle to race on. The file is its own integration-test binary (separate
process), and every test is `#[serial]`. `user_model_correction_wire.rs` deliberately does
not set the variable, so its `BOOTSTRAP_USER_ID = "default"` stays correct; re-run to confirm
no regression: `3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

### The second question: **YES — and it is a cross-user leak, not just a dead surface**

Pre-fix the render read `"default"` *unconditionally* while writes keyed `$WAYLAND_USER_ID`.
A process running with the variable **unset** writes its inferred brief into `"default"` —
precisely the bucket every *named* user's prompt then rendered from. The store is shared
whenever the memory base is (`memory_base_dir()` = `$WAYLAND_MEMORY_DIR` or
`app_config_dir()`; the brief file is one JSON map bucketed by user id), which is the normal
shape for a channel gateway serving several humans under one OS account — the deployment
`WAYLAND_USER_ID` exists for.

Extracted verbatim from the captured outbound body of a session whose identity was
`alice-7QW2M9NONCE`, in the pre-fix run:

    style: formality=0.00, energy=0.50, terseness=0.00, emoji_use=0.00

That is the `"default"` bucket's seeded fingerprint `[0.0, 1.0, 0.0, 0.0]` after the EMA
fold. **It is demonstrated on the wire, not argued.**

**Severity: this half is HIGH, not MEDIUM.** One user's inferred personal traits are placed
in another user's system prompt and shipped to a third-party provider. It is unauthenticated
(no capability is required — merely being a *named* user is enough), silent (no surface says
which bucket was rendered), and the disclosed material is exactly what the layer exists to
infer. Precondition, stated honestly: it needs two processes sharing a memory base with
different values for the variable. That is a configuration, not an exploit — but it is the
configuration the variable was built for. The fix closes it by construction.

---

## Defect 2 — `CEILING_IN_FLIGHT` — PREMISE HELD, VERDICT: **DEAD CONSTANT**, DELETED

**One correction to the brief's wording.** `CEILING_IN_FLIGHT` *was* referenced in product
code — once, at `trigger.rs:99`, inside `TriggerBound::clamp_to`. What appears **zero** times
in `runner.rs` is `max_in_flight`, the field the ceiling bounded. The two halves are
different findings and call for different answers, so this matters.

**Evidence it is dead, not unenforced.** `effective_bound()` is the only bound consumer and
always applies `clamp_to(&trigger.default_bound())`. All seven variant defaults hardcode
`max_in_flight` as a literal — 1, or 2 for `Event` — with **no input path reaching them**.
So `default.max_in_flight.min(16)` was `min(1_or_2, 16)` forever. The ceiling could never be
the binding operand, and no input could make it one. There is nothing to enforce: `clamp_to`
already enforces a strictly tighter bound than the ceiling ever would.

Deleted, and the clamp is now `self.max_in_flight.min(default.max_in_flight).max(1)`.

**Decision procedure** (LANE-BRIEF §3.3 / §4). Cross-audit panel, three models, **unanimous**
for delete: codex `gpt-5.6-sol`, gemini `3.1-pro-preview`, kimi K3 all returned
`PANEL_POSITION=A`, each independently citing the project rule against code kept for
hypothetical future authors. Internal adversarial pass argued for keeping it as a
defence-in-depth backstop; that argument loses because the constant guards values *defined in
code*, not values supplied by input — it is a lint on future authors, which AGENTS.md §2
forbids as speculative.

**I dissent from the panel on one point, and say so.** All three also recommended deleting
`FLOOR_INTERVAL_SECS` "for coherence". I did not. Two reasons: (a) it is out of this lane's
scope and AGENTS.md §3 forbids drive-by changes — coherence is not a licence for scope creep;
(b) it is materially different in kind. `Interval`, `Poll` and `Commitment` fold their own
**persisted parameters** into `default.min_interval_secs`, so the floor bounds a partly
input-derived value and is one variant-author slip (a missing inline `.max(60)`) from being
genuinely load-bearing. The in-flight ceiling has no such path. Named below as a finding, not
fixed.

**The tripwire moved rather than being deleted.** The old test warned that if a variant
default ever reached 16 the constant would become live. `in_flight_bound.rs` now carries
`the_deleted_ceiling_changed_no_answer_for_any_variant`, a differential running the OLD and
NEW clamp expressions over every variant × a grid of persisted values `[0,1,2,3,15,16,17,
u32::MAX]`, plus a count assertion so an empty loop cannot pass, plus its own known-negative
against a synthetic default of 100 where the two provably differ.

**KNOWN-POSITIVE:**

    test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

**KNOWN-NEGATIVE** (`Event` default temporarily widened 2 → 64, then reverted):

    test result: FAILED. 2 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out

    ---- the_deleted_ceiling_changed_no_answer_for_any_variant stdout ----
    panicked at crates/wcore-cron/tests/in_flight_bound.rs:225:13:
    event: deleting CEILING_IN_FLIGHT changed the answer for a persisted 17
    against a variant default of 64. The deletion is NOT behaviour-preserving
    and must be reverted.

    ---- the_effective_bound_can_never_exceed_two stdout ----
    panicked at crates/wcore-cron/tests/in_flight_bound.rs:150:9:
    event: a persisted u32::MAX produced an EFFECTIVE bound of 64; no variant
    default permits more than 2

That first failure is the warning the constant used to give, now given by something that can
fail. `max_in_flight` the field is untouched — it is serialized into the persisted job schema
and part of the Desktop wire contract.

---

## Defect 3 — two more `is_network_path` copies in `wcore-tools` — **PREMISE REFUTED**

Both copies are already gone at base. `crates/wcore-config/src/network_path.rs` is the single
implementation (`has_unc_prefix` / `has_device_or_verbatim_prefix`), and it keeps the three
superseded copies transcribed into its test module as executable divergence evidence.

Absence proof, with the instrument proved alive in the same invocation (§3b-i), unproxied
`/usr/bin/grep`, globs quoted:

    KNOWN-POSITIVE  grep -rn "fn has_unc_prefix" --include='*.rs' crates/
                    -> crates/wcore-config/src/network_path.rs:77          (1 hit)
    KNOWN-POSITIVE  grep -rn "fn is_unc_path"    --include='*.rs' crates/
                    -> crates/wcore-tools/src/media_intake.rs:389          (1 hit)
    CLAIM           grep -rn "fn is_network_path" --include='*.rs' crates/
                    -> 0 hits, rc=1

The three surviving `is_network_path` strings under `crates/wcore-tools/` are **comment text
describing the removed copies** — `vision_tools.rs:826`, `media_intake.rs:925`,
`media_intake.rs:958`. The live call sites delegate: `media_intake.rs:389-390` is a two-line
forward to `wcore_config::network_path::has_unc_prefix`, and
`executable_readiness.rs:563` (`is_unc_or_verbatim_path`) forwards to the same module.

**Nothing to consolidate. No change made.**

### The Windows-only branches — how they were actually verified

The brief's "both copies are dead off Windows" no longer applies: removing that evaporation
was the point of the consolidation. `has_unc_prefix` answers **the same on every platform** —
its `#[cfg(windows)]` `Component::Prefix` arm is an additive fast path over a string matcher
that runs everywhere.

The genuinely Windows-only code is those two `#[cfg(windows)]` arms
(`network_path.rs:80-88` and `:109-120`). **They were executed on real Windows.** Cloned this
lane's HEAD to `D:\wl-small-defects` on `SeanD@seandesktop` (Windows, cargo 1.95.0, on `D:`
per the brief's disk rule, not `C:`), and ran:

    cargo test -p wcore-config --lib network_path

    running 6 tests
    test network_path::tests::consolidation_changes_answers_the_old_copies_got_wrong ... ok
    test network_path::tests::device_and_verbatim_are_separated_from_unc ... ok
    test network_path::tests::local_and_device_forms_are_not_unc ... ok
    test network_path::tests::unc_forms_are_recognised ... ok
    test shell::executable_readiness::tests::invalid_cwd_drive_relative_and_windows_network_paths_fail_closed ... ok
    test network_path::tests::syntax_check_and_filesystem_check_answer_different_questions ... ok

    test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 552 filtered out

    WLRC=0
    WLDONE

Read back from a status file by a **separate** ssh call, per §6b-ii — an exit status crossing
ssh+PowerShell is one bit and cannot be trusted. `552 filtered out` with `6 passed` confirms
the name filter matched real tests rather than zero (flavour (c) of the vacuous-green family).
On Windows those `Component::Prefix` arms execute and agree with the platform-independent
assertions, including the `\\?\C:\` verbatim-local case whose classification the
consolidation changed.

**What this does NOT cover, stated plainly.** The
`consolidation_changes_answers_the_old_copies_got_wrong` under-blocking demonstration — that
the legacy `vision_tools` copy returned `false` for a UNC string — sits inside a
`#[cfg(not(windows))]` block and therefore did **not** run on Windows. It is a claim about
Unix behaviour and is proved on Unix; on Windows that test only exercises the over-blocking
half. I did not verify the legacy copies' Windows behaviour, and no test in the tree does.

---

## Gates

* `cargo fmt --all` — clean (run on the Mac, the one permitted local cargo verb).
* `cargo check --workspace --all-targets` on hetzner — **clean**, `Finished dev profile`.
  Workspace-wide, never `-p`. One pre-existing warning, not mine and not touched:
  `non_snake_case` on `wcore-memory/src/activation.rs:198`.
* Every count above read from an **unproxied** `/root/.cargo/bin/cargo`, with
  `0 ignored` / `0 filtered out` intact — `rtk` strips exactly those two fields.

## Real but out of scope — named, not fixed

1. **`FLOOR_INTERVAL_SECS`** (`wcore-cron/src/trigger.rs`) is non-binding today for the same
   structural reason as the deleted ceiling. It is NOT the same finding: it bounds a partly
   input-derived value, so it is one variant-author slip from being load-bearing. Deleting it
   is a judgement call for whoever owns that crate.
2. **`max_in_flight` remains an advertised-but-unimplemented field.** `cron status` renders it
   and `runner.rs` never reads it. The operator surface is annotated at base and the census
   test pins it; enforcing real dispatch concurrency would be a behavioural feature, not a
   repair, and I did not invent one.
3. **`non_snake_case` warning** at `wcore-memory/src/activation.rs:198`.

## Not done

* No PR, no tag, no merge to integration, no issue closed, no `wcore-contract generate`.
* No credential used anywhere; no secret printed, written or transmitted.
* Nothing pushed to `plan/f20-unified-audit-repair` — only `gh lane/small-defects`.
