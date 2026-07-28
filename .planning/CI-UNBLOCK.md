# CI-UNBLOCK — restoring the workspace test signal

Lane `lane/ci-unblock`. Base `plan/f20-unified-audit-repair` @ `ef1d97be`
(captured once, quoted throughout). Fix commit `1e1770d4`.

---

## 1. The fix

**Four lines, one lint, four days of lost CI test coverage.**

CI's clippy step is `vx just lint` (`ci.yml:160`) → `cargo clippy --workspace
--all-targets -- -D warnings` (`justfile:75-76`); the linux-containerized job
runs the identical flags directly at `ci.yml:323-324`. That step **precedes**
the test step, so `nextest --workspace` had executed **zero times** in CI on
this tree since 2026-07-25.

The lint is **`clippy::cloned_ref_to_slice_refs`, new in Rust 1.95.0** — a
toolchain-bump lint, not a code regression. That is why it appeared on all
three platforms at once and why no lane felt ownership of it.

Sites, all in `crates/wcore-eval-scenarios/src/journey.rs`, all `#[cfg(test)]`,
all the `canaries: &[String]` argument of `scan_canaries`: **683, 695, 707, 717**.

`&[canary.clone()]` allocates a one-element array by cloning the String.
`std::slice::from_ref(&canary)` yields the same `&[String]` by borrowing — same
type, same value, no allocation. It is the lint's own suggestion and already
the established idiom in this workspace, including the same crate
(`providers.rs:273,277,279,280`, `compact/estimate.rs:239,245,251`,
`acp/server.rs:1410`, `channel-email/smtp.rs:628`).

**Not suppressed.** No `#[allow(clippy::cloned_ref_to_slice_refs)]` was added,
and none exists anywhere in `crates/` (grepped: zero hits).

Site 707 was the one with real risk — `canary` is moved into the expected value
in the *same* `assert_eq!` statement, so swapping a clone for a borrow puts a
live borrow and a move in one statement. It compiles; NLL ends the borrow when
`scan_canaries` returns an owned `Result`. Flagged as a risk before compiling
and then confirmed, rather than assumed.

### The gate is proven able to fail

Per LANE-BRIEF §6b-ii, three assertions, not two. A script restores the pre-fix
`journey.rs` from base, runs clippy, restores the fixed file, runs clippy again:

| assertion | result |
|---|---|
| known-negative — pre-fix file | `WLNEG=101` FAILS |
| known-positive — fixed file | `WLPOS=0` PASSES |
| worktree restored, no residue | `WLDIFF=0` |

And it failed for the *right* reason: the negative log holds exactly four
`cloned_ref_to_slice_refs` errors at exactly `journey.rs:683:13, 695:13,
707:34, 717:13` — the same four lines, same columns, as the CI log from failed
run `30369041140`.

**Toolchain checked before trusting any of it.** The lint is 1.95-specific, so
an older rustc would have produced a green proving nothing (§3.2, "the gate was
already green at base"). Inside the worktree rustup honours the
`rust-toolchain.toml` pin: `rustc 1.95.0` / `clippy 0.1.95`, matching `vx.toml`.
Hetzner's *default* is 1.96.0 — checked from outside the worktree it would have
been the wrong compiler.

### Workspace clippy — CI's exact invocation

```
cargo clippy --workspace --all-targets -- -D warnings
WLRC=0    Finished `dev` profile in 1m 45s
wcore_crates_checked=49   wayland_plugins_checked=5   (= the 54-crate workspace)
error_lines=0
```

Coverage counted, not assumed — rc=0 on a run that checked nothing is the exact
failure class this program keeps hitting. The one `warning:` line is cargo's
future-incompat notice for third-party `imap-proto v0.10.2`; not a clippy
diagnostic, not gated by `-D warnings`.

---

## 2. What `nextest --workspace` reveals

**This had not run in CI on this tree for four days. It runs now.**

`cargo nextest run --workspace --profile ci --no-fail-fast`, preceded by the
same two pre-builds CI does (`tool_token_bench`, `wcore-cli --release`; both
rc=0), on `hetzner-dsm` @ `1e1770d4`:

```
Starting 12775 tests across 544 binaries (50 tests skipped)
Summary [74.005s] 12775 tests run: 12772 passed (2 flaky), 3 failed, 50 skipped
```

**The headline is that it is nearly clean.** Four days of unwitnessed merges
produced **three** failures, and all three are explicable. Nothing here suggests
broad rot.

### None of the three is mine — proven, not asserted

```
BASE=ef1d97beb61f1b084bdfba745e8f49830924d757
git diff "$BASE" --stat HEAD
  .planning/CI-UNBLOCK-NOTES.md              | 149 +++++
  crates/wcore-eval-scenarios/src/journey.rs |  12 +-
files touched in wcore-cli / wcore-agent / wcore-protocol: 0
untracked files: 0        (--name-only is blind to these, so counted separately)
```

My whole diff is 12 lines inside one `#[cfg(test)]` module of a crate none of
the three failing tests depend on. Causally impossible.

### FAIL 1 + 2 — `browser_suite` / `computer_use` capability assertions (HIGH, dispatch)

| | |
|---|---|
| `wcore-cli::plugin_discovery_e2e` | `ready_event_has_plugin_capability_flags` (TRY 3 FAIL) |
| `wcore-cli::release_binary_smoke` | `release_binary_ready_event_advertises_plugin_capabilities` (FAIL) |

```
assertion `left == right` failed: expected capabilities.browser_suite=true
(wayland-browser plugin not discovered)
  left: Null      right: true
```

**The engine is right and the tests are stale.** Commit **`85b60a2f` (2026-07-28)
— `fix(agent): advertise browser/CUA capabilities on liveness, not linkage`**,
ledger row 27-C2(b), deliberately stopped advertising these flags when no
backend can actually start:

```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH
  and no sidecar answered http://localhost:9377/health
```

(`crates/wcore-agent/src/output/protocol_sink.rs:199` and `:212`.)

That commit is a *good* commit — it fixes a real defect where a headless host
advertised `browser_suite: true` and the first operation died with `spawn
camoufox: No such file or directory`. It shipped its own passing test
(`capability_liveness_narrowing.rs`), and `wcore-agent`'s
`browser_suite_advertised_when_wayland_browser_loaded` still passes. What it did
**not** do is update the two `wcore-cli` e2e tests, which have asserted the flags
unconditionally since `da5a18b5` (2026-06-08).

**It merged on 2026-07-28 — inside the blind window — and clippy being red is
the only reason nobody saw the two reds it created.** This is precisely the
class of breakage this lane was opened to expose.

Note the release test's own error text speculates "wayland-browser stripped by
release LTO?". **That hypothesis is dead** — the debug build fails identically.
Whoever picks this up should not spend time on LTO.

**Not fixed here, deliberately.** The correct repair is a contract judgement —
make the tests tolerate a liveness-narrowed capability on a headless host, or
provision `camofox-browser` on runners, or force the capability under a test
env var. Each changes what these tests certify, and LANE-BRIEF §5 forbids
weakening a test to reach green. This belongs to the owner of 27-C2(b) with a
cross-audit, not to a drive-by from the CI-unblock lane. **These two will fail
on any runner without `camofox-browser` installed, so expect them red in CI.**

### FAIL 3 — `desktop_contract_corpus` (MEDIUM, known, orchestrator-level)

```
wcore-protocol::desktop_contract_corpus
  checked_corpus_matches_real_serializers_byte_for_byte  (TRY 3 FAIL)
Desktop contract corpus drift: missing=[], extra=[],
drifted=["adversarial/events/fixture-mismatch.jsonl",
         "adversarial/events/schema-mismatch.jsonl",
         "adversarial/events/version-mismatch.jsonl",
         "events/ready.json", "manifest.json"]
```

Already documented as **CLASS-CONTRACT-01** in `.planning/BACKLOG.md:889`
("the contract corpus reddens every lane by construction — MEDIUM, friction not
defect"). `SOURCE_INPUTS` digests 40 engine files including
`crates/wcore-cli/src/main.rs`, the shared fence every lane is told to edit.

I did not touch `main.rs`, and `wcore-eval-scenarios` is **not** in
`SOURCE_INPUTS` (grepped: zero hits), so this red is inherited from the
integration branch, not caused by this lane.

Correctly **not** actioned here: LANE-BRIEF forbids `wcore-contract generate`,
and BACKLOG is explicit that per-lane regeneration is actively harmful — one
regeneration over the *merged* tree clears all lanes at once, and Desktop must
re-pin in the same release train.

> **Seam request (orchestrator, serialize):** one `wcore-contract generate` over
> the merged tree once lanes are integrated, plus the matching Desktop re-pin.
> `schema_digest` is unchanged; only `fixture_digest` and `source_inputs_digest`
> move — the benign shape Sean authorized at `c743f398` / `5f74d559`.

### The 2 flaky (passed on retry — recorded, not actioned)

- `wcore-swarm worktree::tests::linux::status_output_cap_kills_git_descendant` — FLAKY 2/3
- `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream` — FLAKY 3/3

Both are wall-clock/process-timing shaped, matching the known contention class
in BACKLOG. Reported, not chased.

### What did *not* go wrong

No EMFILE cluster in `wcore-skills` (`fs.inotify.max_user_instances` was already
512, load 8.8/96 cores, 691G free), so no re-run-in-isolation was needed. The
`no-tests = "fail"` nextest setting means a zero-test invocation could not have
passed silently. 544 binaries and 12,775 executed tests are counted from the
run, not inferred from exit status.

---

## 3. CI run

Pushed **once**, at 19:28:37Z, commit `1e1770d4`. Run **`30392102087`**.

`ci.yml` sets `cancel-in-progress: ${{ github.event_name == 'pull_request' }}`,
so a branch push does **not** cancel — the queue was 26 deep with 3 in flight
behind ~20 parallel lane branches, which is capacity, not failure.

**Result: see §4 below.**
