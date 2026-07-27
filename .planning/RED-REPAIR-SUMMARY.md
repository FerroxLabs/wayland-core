# RED-REPAIR-SUMMARY — lane/red-repair

**Base:** `0f3330e5` (integration branch `plan/f20-unified-audit-repair` HEAD).
**Branch:** `lane/red-repair`. **Box:** `hetzner-dsm`, worktree `/root/wayland-red-repair`.

## The measurement, taken myself, not inherited

I re-ran the full workspace at my own base before touching anything, and the numbers differ
from the ones I was handed — so the handed numbers are not what this lane is graded against.

| | build | tests run | passed | failed | timed out | skipped |
|---|---|---|---|---|---|---|
| **Baseline @ 0f3330e5 (mine)** | exit 0, 0 errors | 12172 | 12165 | **6** | 1 | 49 |
| After @ `d29413c1` | exit 0, 0 errors | 12172 | 12169 | 2 | 1 | 49 |
| **Final @ `883d8f71`** | exit 0, 0 errors | 12172 | **12170** | **1** | 1 | 49 |

The two runs after the fixes differ only in `packaged_core_cancels_an_active_stream`, which is
the ~2.5% flake of Item 2 — it failed in the first and passed in the second, at the same commit
for that test's purposes. **Five of the six baseline failures are closed.** The two remaining
reds are exactly the two I was told not to fix and could not honestly fix:
`desktop_contract_corpus` (Item 5, fenced) and `fix1_dispatch_budget_aborts_with_partial_result`
(Item 3, disproved as a hang, red left standing).

Two differences from the brief's numbers worth stating plainly:

- The brief cited 12154 tests / 7 failed at `32e2f57d`. My base is `0f3330e5`, 18 tests later
  (the `lane/22-wire` merge), and carries 6 failures, not 7.
- **`packaged_core_cancels_an_active_stream` PASSED on my baseline** — `PASS [3.652s]`,
  6371/12172, first try. It is not a deterministic red. See Item 2.

Logs: `/root/rr-build-base.log`, `/root/rr-test-base.log`, `/root/rr-build-after.log`,
`/root/rr-test-after.log`, `/root/rr-test-final.log`.

---

## Item 1 — gateway escapes the hermetic root — **FIXED** (`f8ac8b46`)

**Root cause.** `SystemdManager::unit_path` at `crates/wcore-gateway/src/service.rs:338` called
`dirs::config_dir()` directly, the fourth raw call site outside the canonical helper. Introduced
by **`b22e3ecc`** ("fix(24-01): make the Windows daemon actually survive its session") — not
`fa6c33c0` or `8b582851`; `git log -S` over the file returns exactly one commit.

**Blast radius: none for hermeticity, and the ALLOWLIST would have been the wrong answer for a
different reason than expected.** The path is where `gateway install` writes the *systemd user
unit*. Two facts decide it:

1. Routing it through `wayland_config_dir()` would **break the feature outright**. systemd's user
   manager scans only `$XDG_CONFIG_HOME/systemd/user` (else `~/.config/systemd/user`). A unit
   under `$WAYLAND_HOME/systemd/user/` is never read: `install` would print `wrote unit: …` and
   register nothing, and `systemctl --user start` would fail "Unit not found".
2. No Wayland state leaves the hermetic root. The generated unit carries
   `Environment=WAYLAND_HOME=<home>`, so it is a *pointer into* the hermetic home, not state
   inside it. The macOS sibling writes its plist to `~/Library/LaunchAgents` for the identical
   reason and escapes the audit only because `dirs::home_dir()` is not gated.

So the caller **is** legitimately outside the hermetic root — but the ALLOWLIST is still wrong,
because its three existing entries are read-only probes that write nothing, and this one writes.
`config.rs` already exports `os_native_config_root()` for exactly this: the sanctioned bypass,
kept in the one allow-listed file so the audit's single-call-site invariant survives. It was
`pub(crate)` only because it had one consumer.

**Fix.** Make `os_native_config_root()` `pub`, widen its doc to name both consumers, call it from
`service.rs`. `wcore-gateway` gains a `wcore-config` dep, which its own crate header already
declares as permitted; `wcore-config` depends only on `wcore-types`/`wcore-compact`/`wcore-budget`,
so no cycle. Behaviour is byte-identical — this moves *where* the native root is resolved, not
what it resolves to.

**Panel.** codex 5.6-sol / gemini 3.1-pro / kimi K3 + internal adversarial: **3-0-0** for this
option, unanimous that `wayland_config_dir()` would break the feature and that allow-listing a
*state-writing* bypass alongside read-only probes weakens the audit.

- **RED BEFORE:** `FAIL … Found 1 bypass(es) … crates/wcore-gateway/src/service.rs:338`
- **GREEN AFTER:** `635 tests run across wcore-config + wcore-gateway: 635 passed, 0 skipped`;
  `cargo clippy -p wcore-gateway -p wcore-config --all-targets` clean.
- **GATE PROVEN LIVE, unplanned:** the audit went red *on my own first draft* — the rationale I
  put in `crates/wcore-gateway/Cargo.toml` contained the literal call form, and the filter strips
  `//` and `*` comment prefixes but not `#`. I reworded my comment; I did not touch the test.
  That is a live demonstration that this gate can fail.

---

## Item 2 — the stream-cancel accounting assertion — **NOT A RED; it is a ~2.5% flake** (`883d8f71`)

**It passed on my baseline.** Then it failed both tries in a later full-workspace run at the same
commit. Characterised in isolation on an idle 96-core box:

- **39 PASS / 1 FAIL in 40 sequential reps** (failed on rep 7).
- **0 FAIL in a further 80 reps at `-j 8`.**
- Both observed reds happened under **full-suite contention**, not in isolation.

So neither hypothesis in the brief holds: there is no new cost-accounting regression on the
cancel path, and the expectation is not stale — the assertion is right ~97.5% of the time on the
same binary and the same commit.

**What I could not learn, and why.** The assertion was
`assert!(matches!(result.failures.as_slice(), [Failure::CostMissing]))` with **no message**. Two
full-workspace reds produced nothing but `assertion failed: matches!(…)` and a line number. For a
2.5% flake that is the worst possible shape — the one run in forty that goes red is the only
chance to see the actual value, and it discards it.

**What I did.** Added the failure payload (the actual `failures` vector, wall time, cancellation
flag, final text). **The predicate is byte-for-byte unchanged** — same `matches!`, same expected
shape, same strength. Nothing was weakened, re-gated or ignored; a mute red became a diagnostic
one. I then re-ran the full workspace to try to capture the value under contention.

**Honest verdict: OPEN.** The flake is not closed. Filed in `.planning/BACKLOG.md` under
`wall-clock-budgeted binary tests are flaky under full-suite load` (MEDIUM), which it shares with
the eval-scenarios orphan-listener flake.

---

## Item 3 — the dispatch-budget abort — **DISPROVED: it does not hang** (no code change)

**The brief's framing was that this is the most serious item — "the exact shape of an abort that
does not abort". It is not. The abort works.**

Run standalone under `cargo test` with no harness timeout:

```
test fix1_dispatch_budget_aborts_with_partial_result ... ok
test result: ok. 1 passed; 0 failed; … finished in 65.97s
```

It **passes** in 65.97s. It produces its `DispatchBudgetExceeded` with its partial result. It is
killed by nextest, whose profile is `slow-timeout = { period = "30s", terminate-after = 2 }` —
a 60s budget. 66 > 60. That is the whole of it.

**Where the 66s goes**, measured on the real path, not inferred:

| phase | cost |
|---|---|
| `prepare_durable_launch` | 0.10 ms |
| `admit_resolved` | 2.38 ms |
| `execute_resolved_launch` | **46.38 ms** — of which `engine.run()` is **44.82 ms** |

~47ms of **inline synchronous CPU** per sub-agent dispatch in a debug build (6.9ms in release),
against a zero-latency in-memory provider. Not I/O: moving the journal to tmpfs changes nothing
(47.2 → 46.2 ms). 1000 dispatches × 47ms ≈ 47s, plus the rest ≈ 66s.

**A finding I then retracted, and the retraction is the useful part.** `run_pipeline` drives its
per-item futures with `buffer_unordered(20)`, which multiplexes every future onto ONE task. Over
the spawn path that measured **zero** speedup — width 1/4/20 → 47.2 / 51.0 / 53.9 ms per spawn,
mildly *negative*; same shape in release (7.85 / 9.76 / 11.38). Real `tokio::spawn` tasks gave
3.08 ms/spawn on 16 workers, a 9x speedup. That looked like a HIGH finding: the module doc claims
"a fast item races ahead through all stages while a slow item is still on stage 1", and bounded
concurrency of 20 appeared to deliver 1x.

Then I varied **only** the provider's latency (60 dispatches each):

| provider latency | width=1 | width=20 | speedup |
|---|---|---|---|
| 0 ms | 48.7 ms/spawn | 50.9 ms/spawn | 0.96x |
| 50 ms | 100.4 ms/spawn | 50.3 ms/spawn | 2.0x |
| **500 ms** | **551.1 ms/spawn** | **69.1 ms/spawn** | **8.0x** |

**The 1x was an artifact of a fake provider with no I/O to overlap.** Real providers are network
calls in the hundreds of milliseconds; that I/O *does* overlap, and the pipeline streams at 8x
and rises toward the configured width as latency grows. The doc's claim is **true** for the
production workload. What survives is a throughput ceiling of roughly `1 / 47ms` per pipeline
that binds only when stage latency falls *below* the per-dispatch CPU cost — i.e. only against
fixtures. Filed **LOW**, `pipeline-cpu-floor`, in `.planning/BACKLOG.md`.

**Panel, and the vote that moved.** On the pre-correction facts: gemini MEDIUM/REPORT, kimi
MEDIUM/REPORT, **codex HIGH/FIX**. Because codex was the dissenter and its vote would have sent
me into a public-API refactor, I re-put the corrected facts to all three rather than banking a
stale vote. On the corrected facts: gemini **LOW/REPORT/HONEST=YES**, kimi
**LOW/REPORT/HONEST=YES**, codex produced **no output on two attempts** — a dropped vote, which
I am recording rather than counting as agreement.

**What I did NOT do, deliberately.** I did not raise the timeout, add the test to
`.config/nextest.toml`'s documented 90s per-test override cohort, shrink the 400-item workload,
or mark it ignored. Every one of those is forbidden to this lane, and the last three would have
converted a true signal into a green. **The test stays red.** The correct repair is a
test-infrastructure decision — the override cohort already exists for exactly this class, with
the rationale "the tests themselves were never hung — just slow under load" — and it belongs to
whoever owns that file, not to a repair lane.

---

## Item 4 — the four corpus reds — **FIXED, and they were a real finding** (`d29413c1`)

The brief is right that these four are not test breakage. The canary was unblinded, it tripped,
and the trip was true: F21-02 built the sub-allocation channel (`sub_budget_narrowed`, reachable
from the Delegate tool's `budget` object through `ForkOverrides`), so the *absence* that the
census recorded as part of the protection is gone.

But its `Tripped` state had become permanent and correct-forever, and **a canary whose red is the
right steady state is a red light wired to the ignition.** The repair re-points it from absence
to enforcement — the thing the property now actually rests on.

**`budget_narrowing_channel_canary(dimension)` drives the real channel.** It hands
`sub_budget_narrowed` a request orders of magnitude wider than the parent's on every dimension
and observes the refusal. It trips in the two directions that are genuinely news:

- **the channel VANISHED** — no `crates/*/src` caller forwards an override any more, i.e. F21-02
  reverting to the vacuity Phase 21 graded NOT MET three times. Absence is now the *alarm*, not
  the pass. This is the inversion the brief asked for.
- **the widening SUCCEEDED** — judged as an **equality against the parent's cap**, not an
  inequality against the ask, so a seam that widened to some third value has nowhere to land.

It checks **both** the envelope that BINDS the child (`effective_budget()`, the ancestor fold)
and the one the child NAMES (`limit_for`, the leaf). The second is precisely what
`sub_budget_narrowed` was added to fix, and the first stays correct without it — so checking only
the first would be blind to the regression.

**A self-passing hazard found and closed while writing it.** `limit_for` resolves through
`with_reason_state`, which renders the cap of whichever state is *currently exceeded* and only
falls back to the leaf when none is. Probing against `tight_parent_budget()`'s deliberately tiny
40 ms wall cap meant any scheduling hiccup would make the ancestor exceeded, so `limit_for` would
render the **parent's** cap and the Time leg would pass regardless of what the child named. The
canary gets its own `narrowing_probe_parent()` with a 3600 s wall cap — unreachable in a probe
that runs in microseconds, still 24x narrower than the 86 400 s the hostile request asks for.

The four entries keep `no_channel_canary: true` (they still carry a canary; it measures something
else now), so `every_vacuous_dimension_carries_a_no_channel_canary` is untouched. The
`assert_no_channel_canaries_stayed_intact` panic text was updated because it asserted a cause it
can no longer know. **Nothing was re-blinded.**

- **RED BEFORE:** `corpus_time`, `corpus_token`, `corpus_cost`, `corpus_depth` — FAILED,
  `NO-CHANNEL CANARY TRIPPED`.
- **GREEN AFTER:** `27 tests run: 27 passed, 0 skipped`. Siblings still green:
  `f21_02_no_channel_canary` 3/3, `f21_02_child_budget_live` 10/10. clippy `-p wcore-cli
  --all-targets` clean.
- **REVERT-PROOF 1 — the widening leg.** `sub_budget_narrowed` reverted to a passthrough
  (intersection dropped) on hetzner: all four go RED, and the message names which half broke —
  *"the child asked for 10000000, the child NAMES 10000000 and is BOUND BY 100 … the ancestor
  rollup still binds it, so every leaf-rendering accessor now reports a cap the child cannot
  actually spend"*. That is the `(binding_ok=true, named_ok=false)` arm: the canary caught the
  exact defect the ancestor fold alone cannot see.
- **REVERT-PROOF 2 — the vanish leg.** The production caller in `spawner.rs` reverted to
  `sub_budget(None)`: *"the sub-allocation REQUEST CHANNEL HAS VANISHED … F21-02 has reverted to
  being satisfied by ABSENCE"*.
- Both reverts restored; `git status` on hetzner confirmed clean of them.

---

## Item 5 — the contract digest — **LEFT ALONE, as instructed**

`wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte`
is still red and I did not touch it. `git diff --name-only 0f3330e5..HEAD` matches
`wcore-protocol|contract|\.json$` **zero** times. `wcore-contract generate` was never run. The
fence at `.planning/SEAM-REQUESTS/CONTRACT-DIGEST-RESTAMP.md` is intact.

## The eval-scenarios flake — investigated cheaply, filed

`wcore-eval-scenarios::runner_contracts outer_deadline_reaps_owned_descendant_listener`, FLAKY
(failed try 1, passed try 2). It fails at `runner_contracts.rs:230`, *"owned descendant must
publish pid, port, and heartbeat"*. `wait_for_orphan_state` polls **one second** for a freshly
spawned descendant to write pid/port/heartbeat, inside a scenario whose `max_total_time` is also
**one second**. Under contention the process spawn does not beat the poll window. The product's
reaping is not implicated — the assertion is about the fixture publishing its own markers.
Real race, in the harness. Filed MEDIUM.

---

## What this lane did NOT do

- Did not weaken, re-gate, `#[ignore]`, delete, or raise the timeout on any test. The one test
  edit (Item 2) added a failure message and changed no predicate.
- Did not run `wcore-contract generate` or touch any contract fixture.
- Did not attempt the `Arc`/`'static` refactor of `WorkflowRunner` / `PipelineStageDispatch`
  (Item 3), on a corrected LOW severity and a 2-0 panel with the third vote dropped.
- Did not close the Item 2 flake; only made its next red readable.
- Never used `git add -A`, `checkout`, `reset`, `stash`, or `rebase`. Staged only the seven files
  listed by `git diff --name-only 0f3330e5..HEAD`.
