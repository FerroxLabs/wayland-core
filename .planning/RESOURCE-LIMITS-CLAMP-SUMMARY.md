# SUMMARY — lane/resource-limits-clamp

Branch `lane/resource-limits-clamp`, base integration `b2ddf113`.
Verdict: **goal achieved.** `BL-UNTRUSTED-RESOURCE-LIMITS` closed, the family swept in full, two
further instances found and measured, `egress_allow` adjudicated.

## 1. Brief premise — all six claims HELD

Every file:line and value in the brief was exact at `b2ddf113`: the false comment at `:4554`, the
unclamped `max_tokens` at `:4088` and `max_turns` at `:4093`, the 100→999999 / 5→100000 proof test,
`[budget]`/`[session_cap]` not forwarded, and the sibling fix at `f09d5898`. Nothing refuted.

## 2. The clamp

In `restrict_untrusted_project_config` (which gains a `&global` parameter) — i.e. exactly where the
false comment lived — and **trust-gated**:

```rust
restricted.default.max_tokens = if project.default.max_tokens != default_max_tokens() {
    project.default.max_tokens.min(global.default.max_tokens)
} else {
    project.default.max_tokens
};
restricted.default.max_turns = match (project.default.max_turns, global.default.max_turns) {
    (Some(p), Some(g)) => Some(p.min(g)),
    (Some(p), None)    => Some(p.min(SMART_MAX_TURNS)),
    (None, _)          => None,
};
```

Both clamps `tracing::warn!` when they bite, so a suppressed legitimate request is discoverable
rather than silent.

### Absent-value behaviour — the thing the brief told me to check

- **`max_tokens` absent** (or **no project file at all** — a missing one loads as
  `ConfigFile::default()` and the merge runs unconditionally) deserializes to the non-zero default
  **64000**, which is **not** the identity element for `min`. The presence gate is retained for that
  reason. Honest qualification: enumerated over 30 `(project, global)` pairs, the gate is
  **equivalent** to an unclamped `min` at this site (0 differing cases) because the merge's own
  presence gate downstream already rescues the absent case. What it really guards is the natural
  next edit — moving the comparison to the merge site as `project.min(global)` — which **is** a
  regression: measured, it drags an operator's global 200000 down to 64000. I put an explicit
  "do not add `.min()` here" comment at the merge site and a test that reddens for that variant.
- **`max_turns` absent** is `None`, which `Option` models exactly — no landmine.

### The residual hole my own first draft left open

A global `None` is **not** unlimited: `Config::resolve` finishes the field as
`cli.max_turns.or(merged.default.max_turns).unwrap_or(SMART_MAX_TURNS)`, so an operator with no cap
has an **effective** ceiling of 512. My first draft passed `(Some(p), None)` straight through, which
still let an untrusted project raise the effective ceiling from 512 to 100000 — **the defect
surviving inside its own fix.** The `(Some, None)` arm now clamps against the backstop, and
`untrusted_project_cannot_raise_past_the_backstop_when_the_operator_has_no_cap` locks it.

### Why trust-gated

Panel **3/3** (codex `gpt-5.6-sol`, gemini `3.1-pro-preview`, kimi K3), same strongest reason from
all three: a trusted workspace can already register `[mcp.servers]`/`[providers]` (arbitrary tool
execution), and `[budget]`/`[session_cap]` — strictly more powerful, dollar-denominated ceilings —
already merge project-wins unclamped on the trusted path while being dropped on the untrusted one.

**All three raised the same counter, and it is refuted by measurement, not argument:** *"trust is
sticky while repo content is not."* It is not sticky. `fingerprint_workspace`
(`workspace_trust.rs:162, :206-211`) hashes the **content of `.wayland-core.toml`** into the trust
digest, and `WorkspaceTrustStore::resolve` (`:99-104`) re-derives and compares it every resolve. The
hostile edit that would exploit the trusted path is the same edit that revokes the grant. Locked by
`raising_the_ceiling_in_a_trusted_repo_revokes_its_own_trust`, which asserts **both** halves — that
the grant worked on the reviewed content, and that it lapsed after the edit.

Minority position retained: the internal-consistency argument (an unclamped knob among clamped
neighbours) is real, but it is a style cost, and closing it properly needs a `max_tokens:
Option<u32>` migration to distinguish "unset" from "64000".

## 3. Full field-by-field sweep

**I did not inherit the prior lane's "everything else is clamped or dropped" claim — I re-derived
it, and it is WRONG.** Two further loosenings/overrides exist. All 26 `ConfigFile` fields:

Legend — **U** = untrusted path, **T** = trusted path.

| # | Field | Declared intent (its comment) | Actual behaviour | Verdict |
|---|---|---|---|---|
| 1 | `default.provider` | project overrides | non-default project wins; U: dropped→global | correct |
| 2 | `default.model` | project overrides | `project.or(global)`; U: dropped→global | correct |
| 3 | `default.max_tokens` | *"can only reduce power"* | **never compared** | **UNCLAMPED — FIXED** |
| 4 | `default.max_turns` | *"can only reduce power"* | **never compared**, and `None`≠unlimited | **UNCLAMPED — FIXED** |
| 5 | `default.approval_mode` | stricter-only | honoured iff `is_at_least_as_strict_as` | correct |
| 6 | `default.system_prompt` | defanged | `neutralize_trust_delimiters` | correct |
| 7 | `default.user` | cosmetic | project wins | correct (engine never gates on it) |
| 8 | `default.read_only` | either layer may ask | `global \|\| project`, default **false** = identity | correct |
| 9 | `security.enabled` | operator-owned | `global` only | correct (fixed `f09d5898`) |
| 10 | `security.egress_allow` | widens, trust-gated | concat; U: dropped | **by design — see §4** |
| 11 | `execution` | operator-owned | `global`, warns | correct |
| 12 | `provider_policy` | operator-owned | `global`, warns | correct |
| 13 | `providers` | trust-gated | merged; U: dropped+warn | correct |
| 14 | `profiles` | trust-gated | extend; U: dropped | correct |
| 15 | `tools.auto_approve` | never grantable | `global` always | correct |
| 16 | `tools.allow_list` | narrow-only | intersected with global | correct |
| 17 | `tools.allow_no_sandbox` | tighten-only | `Some(true)` honoured iff global allows | correct |
| 18 | `tools.skills.deny` | narrowing | concat | correct |
| 19 | `tools.skills.allow` | trust-gated grant | concat; U: dropped+warn | correct |
| 20 | `tools.env_passthrough` | trust-gated | concat; U: dropped+warn | correct |
| 21 | `tools.sandbox` | trust-gated | `project.or(global)`; U: `None`→global | correct (and defanged by #17) |
| 22 | `tools.windows_shell` | project overrides | `project.or(global)`; U: `None`→global | correct |
| 23 | `tools.media_pricing` | key-by-key | extend; U: empty→global | correct |
| 24 | `tools.verify_edits` | *"Off by default"* | **default is `true`**; `\|\|` makes `true` absorbing | **MIS-POLARISED — LOW, measured** |
| 25 | `session.*` | project overrides | U: default `enabled=true` is `&&`-identity → global | correct |
| 26 | `hooks.pre/post/stop` | operator opt-in | dropped unless global `trust_project_hooks`, warns | correct |
| 27 | `hooks.dispatch_enabled` | opt-out either side | `&&`, default `true` = identity | correct |
| 28 | `hooks.trust_project_hooks` | operator-owned | `global` only | correct |
| 29 | `mcp.servers` | trust-gated | extend; U: dropped+warn | correct |
| 30 | `mcp.curation` | *"a fresh project file inherits sensibly"* | **unconditional project assignment; it overwrites** | **CLOBBERS GLOBAL — LOW, measured** |
| 31 | `plan` / `file_cache` / `compact` | project-if-non-default | all three gates use `enabled` default **true**, so `!enabled` is false → global | correct |
| 32 | `debug` | project overrides | `Option::or`; U: `None`→global | correct |
| 33 | `observability.structured_traces` / `online_evolution` / `workflow_*` | additive opt-in | `\|\|`, all default **false** = identity | correct |
| 34 | `observability.skills_lifecycle` | explicit-false only | only `Some(false)` forwarded; `&&` with `None`→`true` identity | correct |
| 35 | `provider_chain` | project `enabled` wins | U: default (off, empty) → global | correct |
| 36 | `budget` (7 fields) | project-over-global | `Option::or`; U: dropped | correct (trust-gated) |
| 37 | `session_cap` | project block wins | `Option::or`; U: dropped | correct (trust-gated) |
| 38 | `storage` / `memory` / `browser` / `crucible` / `inbound_webhook` | project-if-present | all presence gates false on `default` → global | correct |
| 39 | `bedrock` / `vertex` | project overrides | `Option::or`; U: `None`→global | correct |
| 40 | `anvil.enabled` | kill-switch, narrowing | `global && project`, default `true` = identity | correct |
| 41 | `anvil.gate` / `driver_*` | trust-gated | U: empty/`None`→global | correct |

### The root cause the sweep exposed

`merge_config_files_with_trust` runs **unconditionally**; a missing project config loads as
`ConfigFile::default()` (`config.rs:3617-3619`). So **any merge expression that is not
identity-preserving on the project side overrides the operator's global even when the user has no
project config file at all.** A field is safe only if its serde default is the identity element of
its merge operator. Rows 24 and 30 are the two that are not, and both were confirmed by measurement,
each with a live known-positive (`max_tokens=4321` read back from the same global file):

| Field | Global | Project | Resolved |
|---|---|---|---|
| `tools.verify_edits` | `false` | **no file at all** | `true` |
| `tools.verify_edits` | `false` | explicit `false` | `false` (only a PROJECT can disable it) |
| `mcp.curation` | `Off` | **no file at all** | `TopK { k: 15 }` |

`verify_edits` is an **authority inversion** — the untrusted layer can turn it off, the trusted layer
cannot. Both graded **LOW** and routed to `BL-OPERATOR-GLOBAL-DISCARDED` rather than fixed:
`verify_edits` fails safe, `mcp.curation` grants no capability (it only trims already-connected,
already-trust-gated tools), both are outside this lane's named scope, and the severity policy sends
MEDIUM-and-below to backlog. Fixing either well needs the same presence-vs-default representation
decision `max_tokens` raises.

## 4. Verdict on `egress_allow` — I agree with the prior lane

`egress_allow` concatenating on the trusted path is **by design, and correctly graded**. Reasons, in
descending strength:

1. **It is fully dropped on the untrusted path** — `restrict_untrusted_project_config` never forwards
   it, so a cloned repo cannot add a host until the operator grants the fingerprint. Locked by
   `untrusted_project_cannot_append_to_the_egress_allowlist`.
2. **The grant it rides on is strictly more powerful than the thing granted.** The same gate admits
   project `[mcp.servers]` and `[providers]` — arbitrary tool execution and arbitrary provider
   endpoints. An actor who can add an MCP server does not need an egress allowlist entry to
   exfiltrate; conversely, allowlisting a host grants no code execution. Closing the smaller hole
   while the larger one stays open by design would be theatre.
3. **It widens, it does not disable.** `enabled` stays operator-owned, so the gate itself still runs
   and every non-allowlisted host is still checked — unlike `enabled = false`, which dropped the
   policy to a literal allow-all. That is the distinction that made the sibling defect HIGH and makes
   this one not a defect at all.
4. **Trust here is content-bound**, per §2 — adding a host to `.wayland-core.toml` changes the digest
   and revokes the grant, so this is not a post-trust escalation channel either.

Both arms are already locked in tests (`untrusted_project_cannot_append_to_the_egress_allowlist`,
`trusted_project_egress_allow_widens_the_boundary_by_design`). No change recommended.

## 5. Every control, run in BOTH directions

Three-way discrimination — the suite tells no-fix, wrong-fix and right-fix apart:

| Tree state | Result | Which tests redden |
|---|---|---|
| **Shipped clamp** | `14 passed; 0 failed; 0 ignored; 0 filtered out` | — |
| **Clamp reverted** to pre-fix pass-through | `11 passed; 3 failed` | `cannot_raise_the_resource_ceiling` (999999 vs 100), `cannot_raise_past_the_backstop` (Some(100000) vs Some(512)), `raising_the_ceiling_in_a_trusted_repo_revokes_its_own_trust` (999999 vs 100) |
| **Wrong fix** — `min` at the merge site, presence gate dropped | `11 passed; 3 failed` | `a_project_silent_on_resource_limits_leaves_the_operator_ceiling_alone` (**64000 vs 200000** — the absent-value regression, exactly as predicted), `a_trusted_project_may_still_raise_..._by_design` (100 vs 200000), `cannot_raise_past_the_backstop` |

Both reverts were applied on hetzner only and never committed; the tree was restored to
`1c1a4a5c` with `git status --porcelain` = 0 lines before the final green run.

**Can-pass controls** (§3b-iii): `untrusted_project_may_still_lower_the_resource_ceiling` and
`untrusted_project_may_add_a_turn_cap_when_the_operator_has_none` exist so a "fix" that simply
hard-wired both fields to global cannot pass — a clamp that cannot honour a lowering is a deletion,
not a clamp.

**One of my own instruments was caught and repaired mid-run.** The instrument-alive probe in the
absent-value test used `[default] model`, which the untrusted path does **not** forward, so it failed
against a project file that had loaded perfectly. Switched to `system_prompt` (which is forwarded)
and the reason is recorded in the test.

## 6. Gate results (real numbers, all read from files, never through the proxy)

| Gate | Result |
|---|---|
| `cargo test -p wcore-config` (lib + all integration) | **619 passed, 0 failed** (`574 passed; 0 failed; 0 ignored; 0 filtered out` for the lib, plus 13 binaries; one pre-existing binary reports `0 passed; 1 ignored`) |
| `cargo test -p wcore-agent --test egress_merge_polarity_test` | **14 passed; 0 failed; 0 ignored; 0 filtered out** (14 `#[test]`, 0 `#[ignore]` — count asserted against the source) |
| `cargo clippy -p wcore-config --all-targets` | **0 warnings** |
| `cargo clippy -p wcore-agent --tests` | 8 warnings, **all in `cache_ledger_engine_test` / `user_model_identity_wire`** — files I never touched; my test binary emits none |
| `cargo fmt --all -- --check` | clean (rc 0) |

## 7. Live evidence (§3.1) — the real binary, both arms

Built `target/debug/wayland-core` and ran it in real project directories with a real global config.

**Arm 1 — hostile untrusted repo raising both limits.** The product's own log:

```
WARN clamped the project config's [default] max_tokens to the global ceiling — an untrusted
     workspace may lower a resource limit but never raise it (GHSA-8r7g) requested=999999 applied=100
WARN clamped the project config's [default] max_turns  to the global ceiling — ...
     requested=Some(100000) applied=Some(5)
```

**Arm 2 — known-negative, project LOWERING its limits.** `clamp-warn count = 0`.

That zero was **initially self-passing and I caught it**: the first attempt reported 0 clamp warnings
*and* 0 warnings of any kind, so it proved nothing. Re-run with an `[mcp.servers]` block in the same
project so the "ignored executable or authority-expanding project configuration" warning — same
function, same log target, same invocation — served as the known-positive:
`KNOWN_POSITIVE authority-warn count = 1`, `MEASURED clamp-warn count = 0`. (The first repair attempt
of *that* was itself dead — a malformed probe missing `transport` failed TOML parse — which the
`parse_error_present = 0` check now guards.)

## 8. What I did NOT do

- Did **not** fix `tools.verify_edits` or `mcp.curation` — LOW, out of named scope, backlogged with
  measurements (`BL-OPERATOR-GLOBAL-DISCARDED`).
- Did **not** migrate `max_tokens` to `Option<u32>`, which is what an unconditional (non-trust-gated)
  clamp would require to be correct. Named, not attempted.
- Did **not** change `egress_allow` — adjudicated as by-design, §4.
- Did **not** run a full-workspace build (nine other lanes live; targeted `-p` runs only,
  `CARGO_BUILD_JOBS=10`).
- No `git rebase`, no `git reset --hard`, no `wcore-contract generate`, no push to `main` or to
  integration, no PR, no tag, no issue closed. No credential used or transmitted.
- Did not need to merge integration in — `config.rs` at `b2ddf113` already contained `f09d5898`.
- No shared-file fence edits (`wcore-cli/src/lib.rs` / `main.rs` untouched).

## 9. Files changed

- `crates/wcore-config/src/config.rs` — the clamp, the `&global` parameter, the corrected comment,
  the do-not-do-this note at the merge site.
- `crates/wcore-agent/tests/egress_merge_polarity_test.rs` — inverted the pinning test, +6 tests.
- `.planning/BACKLOG.md` — closed the item, recorded `BL-OPERATOR-GLOBAL-DISCARDED`.
- `.planning/RESOURCE-LIMITS-CLAMP-NOTES.md`, `.planning/RESOURCE-LIMITS-CLAMP-SUMMARY.md`.

## 10. For the orchestrator

Nothing to serialize: no protocol seam, no contract request, no shared-file edit. `config.rs` is
shared with any other lane touching config merge — this lane's hunk is inside
`restrict_untrusted_project_config` and the `[default]` block of
`merge_config_files_with_trust`, plus one changed call site.
