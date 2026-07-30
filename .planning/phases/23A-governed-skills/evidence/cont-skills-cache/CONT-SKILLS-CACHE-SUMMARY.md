---
lane: cont-skills-cache
scope: "CONT-* — the two unbuilt sub-areas: governed skills (23A-C1) and cache economics (F23-04)"
base_sha: b2ddf113681647221dc9e5bbfc7de79b1da90b54
branch: lane/cont-skills-cache
build_host: hetzner-dsm (/root/wayland-cont-skills-cache)
status: complete
---

# `CONT-*` — governed skills and cache economics

Two questions were assigned. **The first was already answered by a lane the competitive ledger did
not know about; the second was misdiagnosed and is now built.**

---

## 1. `23A-C1` — which reading is true at HEAD

**The 2026-07-30 re-grade (MET for the shipped surface) is TRUE. The 2026-07-28 competitive-ledger
row is STALE and false on three specific claims.** Graded off the code and executed tests, never off
a SUMMARY.

| competitive-ledger claim (2026-07-28) | measured at `b2ddf113` |
|---|---|
| *"`promote`, `revoke` and `rollback` do not exist"* | **FALSE.** `crates/wcore-cli/src/skill_govern.rs` (14143 B) ships all four verbs: `run_list:96`, `run_revoke:212`, `run_rollback:238`, `run_promote:256` |
| *"`run_skills_promote` fails closed at `wcore-cli/src/main.rs:2408`"* | **FALSE.** `main.rs:2687` delegates to `skill_govern::run_promote` |
| *"satisfied **only by the absence of any promotion path**, which 23A itself records as a vacuous satisfaction"* | **FALSE — the vacuity is gone.** The passing world is constructible and constructed |

### The vacuity question, answered directly

The brief asked me to name it if I found a satisfaction that holds only because no path exists.
**I did not find that shape at HEAD.** Promotion is load-bearing, not a status field with no reader:
`wcore-skills/src/loader.rs:185 apply_governance()` is the single catalog choke point called from
`load_all_skills:156`, it reads `store.live_revocations()` + `store.promotions()` at `:191`, drops
revoked skills from the catalog entirely at `:228`, and lifts `disable_model_invocation` for a
promoted generated draft at `:244`. Two further readers in the shipped agent:
`wcore-agent/src/bootstrap.rs:4255` and `wcore-agent/src/slash/skill.rs:319`.

### Both directions, executed (not summary-graded)

| suite | result |
|---|---|
| `wcore-skills --test govern_catalog_enforcement` | **5 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_revoke_rollback` | **15 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_cli_drive` | **6 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-skills --test govern_staging_discovery` | **2 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-cli --test skills_promote_advertised_and_works` | **5 passed; 0 failed; 0 ignored; 0 filtered out** |

`govern_catalog_enforcement::promotion_lifts_quarantine_and_an_edit_puts_it_back` is the both-directions
control the brief demanded, and it runs on the production path: a generated draft is asserted
quarantined **before** promotion (the known-positive, without which "promotion lifted it" is free),
un-quarantined after, and re-quarantined when one variable — the bytes — changes.

### Live, through the shipped binary (`wayland-core 0.12.25`)

`LIVE-GOVERN-JOURNEY.txt`. Control skill `wl-control` held constant across all ten steps.

| step | direction | result |
|---|---|---|
| promote `wl-subject` | CAN-PASS | `installed` → `promoted`, digest/authority/promotion-id recorded |
| promote a name that does not exist | CAN-FAIL | **RC=1** |
| revoke | CAN-PASS | leaves `INSTALLED`, enters `REVOKED (1)`, grant auto-`WITHDRAWN` |
| rollback a bogus id | CAN-FAIL | **RC=1** |
| rollback the real id | CAN-PASS | returns `status=installed` — re-quarantined, **not** promoted |

**Nothing built for this item. The correct result was a refutation, and manufacturing work to match
a falsified brief is the thing LANE-BRIEF warns against.**

## 2. `F23A-01-H2` — CLOSED, and now executed rather than source-verified

`23A-STATUS-CORRECTION.md` §1 verified the fix from source and git and stated plainly *"Not re-run
by this lane."* **Re-run here at HEAD: `wcore-agent --lib d1_refusal_terminal_tests` →
5 passed; 0 failed; 0 ignored; 2232 filtered out.** The `2232 filtered out` is the anti-vacuity
read-back — the name filter matched five real tests, not the zero that flavour (c) produces. One of
the five is `approval_denial_control_leaves_turn_committable`, a control, so the other four are
falsifiable. **The competitive ledger's "open and committed red" is stale.**

---

## 3. Cache economics — the ledger is wrong on column 1 and right on column 2

Both F05 rows read `Unavailable: no production constructor` / `Runtime outcome proof: None`.

**Column 1 is FALSE at HEAD.** Production construction sites (unproxied grep; known-positive
`mid_flight_monitor`=10, known-negative `zzz_no_such_symbol`=0):

- `PricingRefresher` — `bootstrap.rs:4073`, `bootstrap.rs:4143`. **Two, not zero.**
- `CooldownTracker` — `resilient.rs:257` (per fallback), `:263` (primary), reached from
  `bootstrap.rs:1039`, which is unconditional on the session path. **Not zero.**

This confirms what `lane/22-remaining` flagged and deliberately left for this row.

**Column 2 is TRUE, and is what this lane built.** Before this change only five
`successful_occurrence` call sites existed, covering `ProcedureSkillDrafting`,
`LegacyAutoSkillDrafting`, `SmartHandoff`, `LearnedPolicy`, `MidFlightMonitor`. Neither
cache-economics capability emitted one.

### What landed

**`CooldownTracker` runtime outcome proof** — `wcore-agent/src/resilient_reporter.rs`. Emitted from
`CircuitReporter::report` **only** on `CircuitState::Open`: the tracker's accumulated failures
crossed the threshold and routing actually changed. `Closed` is the resting state every session
starts in and `HalfOpen` is a trial, not a change of destination; emitting on either would make the
proof unfalsifiable. Deliberately not tied to construction, which happens every boot and would fire
unconditionally.

**`PricingRefresher` runtime outcome proof** — `wcore-agent/src/bootstrap.rs`, emitted **only** when
a live fetch is successfully published to the cache. Construction, a bundled-catalog load, an
already-fresh cache read, a failed fetch and a failed save all emit nothing.

**Two unmeasured startup inputs replaced** (see §4).

### Both directions, live, one variable each

`LIVE-COOLDOWN-both-arms.txt` — one variable, the canned endpoint's response mode. The arm is read
back from the endpoint's own request log, not inferred from the environment (LANE-BRIEF §3b-ii).

| | fail arm (503) | ok arm (200) |
|---|---|---|
| `cooldown_tracker` `ready` | 1 | 1 |
| `reached` | **2** | **0** |
| `outcome_changed` | **2** | **0** |
| `observed` | **2** | **0** |
| `"state":"open"` circuit events | **2** | **0** |
| `CANNED_MODE_SEEN` | `mode=fail` | `mode=ok` |
| known-positive `capability_activation` | 32 | 26 |
| known-negative bogus capability | 0 | 0 |

`LIVE-PRICING-both-arms.txt` — one variable, `WAYLAND_PRICING_AUTO_REFRESH`. Served by a **real**
live fetch from `openrouter.ai` (`UPSTREAM_OPENROUTER_HTTP=200`).

| | on | off |
|---|---|---|
| `pricing_refresher` `ready` | 1 | 1 |
| `reached` / `outcome_changed` / `observed` | **1 / 1 / 1** | **0 / 0 / 0** |
| pricing cache file on disk afterwards | **1** | **0** |
| known-positive `capability_activation` | 29 | 26 |

The cache-file count is an independent filesystem corroboration that the side effect the occurrence
claims really happened.

**The first pricing run was a dead instrument and the known-positive caught it.** Adding
`WAYLAND_HOME` switched the binary to an isolated profile that ignored the test config, so it exited
with `init_failed` — and `KNOWN_POSITIVE_any_capability_activation=0` reddened while every
`PRICING_*` count read a perfectly innocent `0`. Without that assertion this lane would have
reported "no occurrence in either arm" as a finding. Harness fixed in-lane (LANE-BRIEF §6b-ii),
not merely written up.

### Every new gate proven falsifiable AND passable (§3b-iii)

`MUTATION-falsifiability.txt`. Baseline pass → mutate → confirm FAILED → `git checkout -- <path>` →
confirm pass again.

| gate | mutation | result |
|---|---|---|
| `cooldown_occurrence_fires_on_open_and_on_nothing_else` | drop the `Open` guard | **FAILED**, restored → ok |
| `the_open_occurrence_names_only_the_cooldown_tracker` | name `SmartHandoff` instead | **FAILED**, restored → ok |
| `pricing_refresher_construction_is_recorded_in_both_directions` | set the flag at fn entry | **FAILED**, restored → ok |

**Unrun cells: 0.** Every direction of every control above was executed; nothing was skipped.

---

## 4. Zero-measurement / zero-reader surfaces found

**Two fixed in this lane, inside the honesty reporter itself:**

1. **`bootstrap.rs:3040` `cooldown_tracker_constructed: true` — a hardcoded literal.** Every peer
   input is a real fact (`engine.skill_drafter().is_some()`,
   `engine.midflight_monitor_constructed()`, `engine.learned_policy_constructed()`). Because this
   one could not be false, `capability_activation.rs:92-97` — the `NoProductionConstructor` arm for
   `CooldownTracker` — **had no reachable state in production.** A permanently-green gate living
   inside the machinery whose entire job is honest reporting. Now read from
   `ResilientProvider::cooldown_tracker_count()`. **Stated honestly: the value is still structurally
   true on this path today**, because the wrap at `bootstrap.rs:1039` is unconditional. What changes
   is that the fact now follows the code if that ever stops being so. I am not claiming I made it
   falsifiable; I claim I made it a measurement.
2. **`bootstrap.rs:2884` `pricing_refresher_constructed = self.config.provider_chain.enabled`** — a
   *configuration* value assigned to a `*_constructed` field, against `StartupCapabilityInputs`'s own
   contract that keeping the two apart is what *"prevents configured from becoming ready by
   implication."* Now an out-parameter set on the line that constructs the refresher. **This one IS
   falsifiable and both directions are proven** by `pricing_refresher_construction_is_recorded_in_both_directions`.

**One found and NOT fixed — outside my scope, reported per the brief:**

3. **`capability_activation.rs:133-137` — `CapabilityId::DelegateIsolation` is emitted
   `Unavailable / IsolationNotEnforced` unconditionally.** It has no input field, so **no
   configuration and no amount of engineering can make that row report ready** — it is a
   permanently-red gate in the exact §3b-iii sense, and it is the same shape `learned_policy` had
   before Phase 22 wired it (the comment at `:99-106` says so in as many words: *"there was no input
   for it, so no configuration could ever make it ready"*). A row stuck red also hides real
   progress. Maps to `AUTH-*` (sandbox/isolation), not `CONT-*`. **Reported, not edited.**

---

## 5. Regression posture — honest

`wcore-providers --lib`: **862 passed; 0 failed; 0 ignored; 0 filtered out.**

`wcore-agent --lib` at lane HEAD: **2219 passed; 18 failed.** These are **not mine**, and I proved it
rather than asserting it. A BASE control worktree at `b2ddf113` (SHA asserted before the run, its own
`target/`, LANE-BRIEF §"never share a CARGO_TARGET_DIR") with **zero changes** gives
**2212 passed; 22 failed** — *more* failures than my branch, in the same families
(`session::`, `session_journal::`, `engine::audit_2026_05_22_tests::`, `channel_lease::`,
`goal::strategy::`). The failing *set* also differs run to run, which is a flake signature, not a
deterministic regression. None of the 18-22 touches `resilient_reporter`, `build_fallback_providers`
or `capability_activation`.

**I am flagging this cluster as a real finding, not dismissing it:** ~20 session-lease/journal tests
in `wcore-agent --lib` are flaky at BASE on this host. It is out of this lane's scope and I did not
chase it.

`cargo clippy -p wcore-agent -p wcore-providers --all-targets`: no errors. Five warnings, all in
`cache_ledger_engine_test` and `user_model_identity_wire` — test files this lane never touched.
`cargo fmt --all -- --check` → **rc=0**.

---

## 6. Instrument defects hit and repaired this lane

- **Unquoted `--include=*.rs` was eaten by zsh** on my first readers-grep; all four searches returned
  `no matches found`. **The known-positive caught it** — `GovernanceStore` came back `0` when it must
  be non-zero. Re-run quoted: 56 and known-negative 0. Had I trusted the first run I would have
  reported the loader as having no governance readers, the exact false absence LANE-BRIEF §3b-i is
  about.
- **The pricing harness's first run was dead** (see §3). Repaired in-lane.
- Every number in this document was captured by redirect-to-file and read with the Read tool, never
  through a Bash pipe.

---

## 7. What I did NOT do

- Did **not** modify `crates/wcore-cli/src/main.rs` or `lib.rs`. **Zero edits to the fenced shared
  files** — nothing to serialize.
- Did **not** implement `promote_new` materialisation, purge `evolved_prompts` rows on revoke, or
  add promotion authority beyond the invoking surface. `23A-C1-GOVERNED.md` §9-10 excludes these and
  I keep its "MET for the shipped surface, not MET in the absolute" distinction rather than
  overclaiming past it.
- Did **not** chase the ~20 flaky session-lease failures present at BASE.
- Did **not** fix the `DelegateIsolation` permanently-red row (`AUTH-*`, not mine).
- Did **not** run a full-workspace build, edit any ledger row, push to integration, or open a PR.
- No macOS-only behaviour was under test, so the §0 Darwin exception was **not** used. `cargo fmt`
  was the only cargo invocation on the Mac.
- No credential was supplied, printed or transmitted. The only key-shaped values in the harnesses
  are synthetic literals (`sk-synthetic-not-a-secret-cont-skills-cache`).
