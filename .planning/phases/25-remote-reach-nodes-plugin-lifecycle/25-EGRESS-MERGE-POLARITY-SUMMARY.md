# SUMMARY — lane/egress-merge-polarity

**Verdict: the finding HELD, and it was worse than the brief described.** Fixed, measured
end-to-end, and proven with controls in both directions. One additional loosening found and
recorded; one secondary question (`egress_allow`) measured and deliberately NOT closed, with
reasons.

- Branch: `lane/egress-merge-polarity`
- Base: `a3e68a31e9e63767c505345eb996f5eeab2341f9` (asserted against `git ls-remote gh`)
- Integration `27c30527` merged forward into this branch; it changed only `.planning/`
  docs and touches `crates/wcore-config/src/config.rs` not at all, so the merge is clean and
  every result below was re-proven after it.
- Build host: `hetzner-dsm`, worktree `/root/wayland-egress-merge-polarity`
- Severity of the fixed defect: **HIGH**

---

## 1. Which of the brief's claims held

| # | Claim | Verdict | Evidence |
|---|-------|---------|----------|
| 1 | `config.rs:4431` reads `enabled: global.security.enabled && project.security.enabled` | **HELD**, verbatim | line 4431 at base |
| 2 | The in-source comment calls it "most-restrictive `enabled`" | **HELD** | line 4421 at base |
| 3 | `enabled = true` means the egress gate is ON | **HELD at the consumer**, not just the doc | `egress/install.rs:28` → `enforcing`, else `disabled()`; `policy.rs:143` returns `Allow` unconditionally when `posture == Off` |
| 4 | `SecurityConfig::enabled` is `#[serde(default = "default_true")]` | **HELD** | decl `config.rs:296`; `fn default_true() -> bool { true }` at `config.rs:1181` |
| 5 | `read_only: global \|\| project` is the established correct polarity for a safety posture | **HELD** | `config.rs:4123` |
| 6 | The same function says a project config is untrusted "checked into a cloned repo" | **HELD, four times** | lines 4094, 4109, 4141, 4245 |
| 7 | Five neighbouring fields get tighten-only clamps | **HELD** | `approval_mode` 4099, `system_prompt` 4115, `auto_approve` 4153, `allow_no_sandbox` 4154, `allow_list` 4166 |
| 8 | `--i-accept-exfil-risk` does not exist | **HELD** (prior lane `25-c4-egress`'s measurement, corroborated by the doc at 291-295 and by there being no interlock anywhere on the `posture == Off` path) |
| 9 | The bug requires an explicit `[security] enabled = false` in the project config | **HELD** — no path yields `false` by default; a project omitting `[security]` deserializes `true`, and all 7 of my tests' sentinel assertions confirm the merge read the intended files |

**No claim was refuted.** Claim 3 was upgraded from a doc claim to a consumer measurement.

### What the brief did NOT have — and it makes the finding worse

`restrict_untrusted_project_config` (`config.rs:4525`), the function whose entire job is
neutralizing an untrusted project config, **explicitly forwarded the field**:

```rust
// A repository may tighten egress and disable Anvil, but cannot add an
// origin, command gate, provider, MCP server, hook or executable skill
// permission until its independently stored fingerprint is trusted.
restricted.security.enabled = project.security.enabled;   // line 4546 at base
```

So the defect was live on the path taken by **every freshly cloned repository** — untrusted is
the default state — and the neutralizing function was carrying it forward under a comment
asserting the opposite. There was no clamp anywhere else. The brief asked me to stop if one
existed; none did.

There was also an existing test, `untrusted_project_executable_configuration_is_inert_but_narrowing_survives`,
whose assertion `assert!(!merged.security.enabled)` **pinned the vulnerability as intended
behaviour** — an attacker-supplied `false` recorded as a "narrowing" to preserve. Corrected, with
the reasoning left in-source.

---

## 2. Measured impact

Not an argument about a boolean: `crates/wcore-agent/tests/egress_merge_polarity_test.rs` writes a
real global `config.toml` and a real project `.wayland-core.toml` to disk, loads them through
**`Config::resolve_with_provenance`** (the real path), builds the real policy with
**`policy_from_config`**, and issues a real `POST https://collector.attacker-example.com/ingest`
through `EgressPolicy::check`.

At base, with global `[security] enabled = true` and project `[security] enabled = false`, on the
**untrusted** (default) path, that request is **allowed**. The merged `enabled` is `false`, the
policy is `AgentEgressPolicy::disabled()`, and the gate is a no-op. Same on the trusted path.

**Severity: HIGH.** It is remotely triggerable by content alone — cloning a repository and running
the agent in it is enough, no user action beyond the normal workflow — it silently disables the
project's only exfiltration boundary, it fails **open**, it is invisible (the merge emits no warning
for `[security]`, unlike the hooks/providers paths which do), and there is no interlock behind it.
It is not CRITICAL: it does not itself exfiltrate anything, and it cannot re-enable a rail or grant
tool privileges — every other privilege-granting field in the same merge is correctly clamped.

---

## 3. The fix — and why it is NOT the brief's prescribed `||`

The brief said to mirror `read_only`: `global || project`. **I measured that and it is defective;
I did not ship it.**

`read_only` can use `||` because it defaults to **`false`** — absence is the identity element for
`||`. `security.enabled` defaults to **`true`**, which is the identity for `&&` and *absorbing* for
`||`. So under `||`, a project file that says nothing whatsoever about `[security]` deserializes to
`true` and **overrides the operator's deliberate global `enabled = false`**.

Measured, not argued (`evidence/.../02-RED-naive-or-variant.log`): the `||` variant applied in
isolation at `da8e46b5` closes both exfil tests and **reddens both off-switch tests** —
`5 passed; 2 failed; 0 ignored; 0 filtered out`. It trades an exfil hole for a different
correctness bug.

Shipped instead (`config.rs:4456`):

```rust
let security = SecurityConfig {
    enabled: global.security.enabled,
    egress_allow: [global.security.egress_allow, project.security.egress_allow].concat(),
};
```

The egress master switch is **operator-owned**, read from the trusted layer alone — the same
pattern the function already uses for `[execution]`, `[provider_policy]` and
`hooks.trust_project_hooks`. This preserves the operator's documented config-file off switch, and
it is the switch the product's own TUI writes: `tui/surfaces/config.rs:713` persists the egress
toggle via **`patch_global_config`**, i.e. to the global file. There is no TUI surface that writes
`[security]` into a project file. The product already treated this as an operator control; the
merge was the only thing that disagreed.

The dead-and-misleading forward in `restrict_untrusted_project_config` was removed, with the
reasoning and the Anvil contrast recorded in-source.

**Scope fence respected:** I did not implement, stub, or design toward the
`--i-accept-exfil-risk` CLI interlock, and the open owner decision about it is unaffected by this
fix either way. My change concerns only whether an *untrusted* layer may loosen a *trusted* one.

---

## 4. `egress_allow` — measured, and it does NOT share the flaw. Named, not silently closed.

`egress_allow` concatenates global-then-project, so a project appending a host **widens** the
boundary rather than disabling it. Two states, both measured:

- **Untrusted (the default):** the project's entries are **dropped entirely**.
  `restrict_untrusted_project_config` never forwards `security.egress_allow`, so it stays empty
  alongside providers, MCP servers, hooks and executable skill permissions. The exfil POST to the
  host the project tried to allowlist is still **denied**.
  → `untrusted_project_cannot_append_to_the_egress_allowlist`
- **Trusted:** the entries concatenate and the host becomes reachable.
  → `trusted_project_egress_allow_widens_the_boundary_by_design`

**I did not close this, and it is not a second defect of the same class.** It is trust-gated
exactly like project `[providers]`, `[mcp.servers]`, `tools.skills.allow` and
`tools.env_passthrough`: inert until the operator grants the workspace fingerprint. Adding an
egress host is strictly *less* powerful than adding an MCP server (an arbitrary child process),
which the same gate already permits. The distinguishing property of the fixed defect is that it
loosened on the **untrusted** path; this does not. Both behaviours are now locked in tests so the
distinction is explicit rather than ambient.

---

## 5. Polarity sweep — every other security-relevant merge

Method: the untrusted path is the one that matters, so I enumerated exactly what
`restrict_untrusted_project_config` forwards and checked each against its merge polarity.
Everything it does not forward is inert for untrusted projects by construction.

**Correctly polarised (no action):**

| Field | Merge | Why correct |
|-------|-------|-------------|
| `default.read_only` | `global \|\| project` | defaults false; either layer asking for the posture wins |
| `default.approval_mode` | project only if non-default AND at least as strict | tighten-only |
| `default.system_prompt` | project value neutralized | defanged |
| `tools.auto_approve` | `global` only | project may never enable |
| `tools.allow_no_sandbox` | tighten-only clamp | project may not exceed global |
| `tools.allow_list` | project ∩ global | narrow only |
| `tools.skills.deny` | concat | denials only |
| `tools.verify_edits` | `global \|\| project` | defaults false; a safety check either layer wants |
| `anvil.enabled` | `global && project` | **correct here**: `false` removes an automation rail, so `&&` really is a narrowing — the opposite polarity to the egress gate, which is why the two must not be merged the same way |
| `observability.skills_lifecycle` | presence-aware, only `Some(false)` forwarded | narrowing only; the `Option` exists precisely to survive the default-true problem |

**Correct but trust-gated only** (inert while untrusted; effective once the operator grants
trust — consistent with each other and with the product's trust model): `security.egress_allow`,
`tools.skills.allow`, `tools.env_passthrough`, `tools.sandbox`, `mcp.servers`, `mcp.curation`,
`providers`, `profiles`, `browser.policy`, project `hooks` (which additionally require a global
`trust_project_hooks = true`). `hooks.dispatch_enabled` merges `&&`, which would let a project
suppress the operator's global guard hooks — but it is **not forwarded** on the untrusted path, so
it falls in this bucket.

**One further loosening found — MEDIUM, recorded not fixed.** `restrict_untrusted_project_config`
forwards six `[default]` fields under the comment *"Resource limits and read-only/approval
requests can only reduce power."* **That claim is false for two of them.** `max_tokens` merges
"project wins if non-default" (`config.rs:4088`) and `max_turns` merges `project.or(global)`
(`config.rs:4093`) — neither compares the two values, so an untrusted project raises both past the
operator's ceiling. **Measured**, not read:
`untrusted_project_can_raise_the_resource_ceiling_backlog_not_a_boundary` asserts global
`max_tokens = 100 / max_turns = 5` against project `999999 / 100000` and gets the project's values.

Left unfixed deliberately, per the phase severity policy (MEDIUM → backlog, non-blocking): the
blast radius is spend and wall-clock, both of which have separate enforcement in `[budget]` and
`[session_cap]` — **neither of which the untrusted path forwards** — and making "stricter"
comparable for an `Option<usize>` is a design call, not a polarity typo. The test exists so the
next reader meets a measurement instead of the false comment.

**`security.enabled` was the only mis-polarised field on the untrusted path.**

---

## 6. Evidence bar

Three-assertion self-test, all three satisfied:

1. **Known-positive passes.** `control_gate_denies_exfil_when_the_project_is_silent_on_security` —
   the harness reaches `Deny`. Without it, "the boundary held" would be unfalsifiable.
2. **Known-negative genuinely fails.** Verbatim, at base `da8e46b5`
   (`evidence/.../01-RED-base-da8e46b5.log`):

```
running 7 tests
test untrusted_project_config_must_not_disable_the_egress_gate ... FAILED
test trusted_project_config_must_not_disable_the_egress_gate ... FAILED

---- untrusted_project_config_must_not_disable_the_egress_gate stdout ----
thread 'untrusted_project_config_must_not_disable_the_egress_gate' (3262628) panicked at crates/wcore-agent/tests/egress_merge_polarity_test.rs:274:5:
assertion `left != right` failed: the merged `security.enabled` still equals the pre-fix `global && project` result, so the polarity fix is not in effect
  left: false
 right: false

---- trusted_project_config_must_not_disable_the_egress_gate stdout ----
thread 'trusted_project_config_must_not_disable_the_egress_gate' (3262625) panicked at crates/wcore-agent/tests/egress_merge_polarity_test.rs:306:5:
a project `[security] enabled = false` must not disable the operator's egress boundary even in a trusted workspace

test result: FAILED. 5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

3. **The old shape would have missed it.** `untrusted_project_config_must_not_disable_the_egress_gate`
   computes `global && project` inline over the same inputs and asserts the merged result
   *disagrees* with it (`assert_ne!`) — which is exactly the assertion that produced the
   `left: false / right: false` red above. A test asserting only "denied" would also pass on a tree
   where the request never reached the classifier.

**Controls in both directions (§3b-iii).** *Can it fail?* — yes, shown above. *Can it pass?* — the
gate has a reachable green in **both** postures: `control_gate_denies_exfil_...` reaches `Deny`
and `control_operator_global_off_switch_disables_the_gate` reaches `Allow`. That second control is
load-bearing: a "fix" that hard-wired `enabled = true` would satisfy every other test in the file
and this one alone catches it — and it is the test that caught the brief's `||`.

**Counts, from an unproxied `cargo` on hetzner** (`/root/.cargo/bin/cargo`, absolute path, written
to a lane-unique path inside my own worktree, read back with `/usr/bin/grep`):

| Run | Commit | Result |
|-----|--------|--------|
| `egress_merge_polarity_test` (RED, base) | `da8e46b5` | `5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out` |
| same, naive `\|\|` variant | `da8e46b5` + `\|\|` | `5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out` (the *other* two fail) |
| `egress_merge_polarity_test` (GREEN) | `2897db01` | `8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `wcore-config --lib` | `c81fc657` | `574 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `cargo check --workspace --all-targets` | `449826c3` | **rc=0** (workspace-wide, never `-p`) |
| `cargo fmt --all -- --check` | `2897db01` | rc=0 |
| **all three re-proven AFTER merging integration `27c30527`** | **`8aa943a9`** | `8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; `574 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; `check --workspace --all-targets` **rc=0** — `evidence/.../07-POSTMERGE-REPROVE.log` |

The post-merge capture also shows the corrected assertion passing by name:
`config::tests::untrusted_project_executable_configuration_is_inert_but_narrowing_survives ... ok`.

### A second instrument rule landed mid-lane, and I re-verified against it

Integration `27c30527` added a rule to `LANE-BRIEF.md` §3b: `git diff --numstat` returns **wrong
numbers** and `git show HEAD:<path> | grep -c <needle>` returns **0 for a string that is present**,
*even via `/usr/bin/git` and `/usr/bin/grep`* — the absolute path is not sufficient because the
rewrite reaches the pipe. Prescribed repair: redirect to a file, then read the file with the Read
tool.

Some of my local source checks had used exactly the banned shape (a piped `grep` asserting a
**zero**, the single easiest assertion to pass without doing any work). I re-verified every
load-bearing local fact through the file-redirect path, with a known-positive **and** a
known-negative in the same capture — `evidence/.../06-VERIFY-no-pipe.txt`:

- the shipped `enabled: global.security.enabled,` at `config.rs:4456` — **one hit**;
- the old `global.security.enabled && project.security.enabled` shape — present **only** at
  `config.rs:4425`, inside my own explanatory comment, and nowhere as code. This doubles as the
  live known-negative: the grep found the comment, so it was demonstrably alive when it reported no
  code occurrence;
- every `restricted.*` assignment in `restrict_untrusted_project_config` listed in full:
  `restricted.security.enabled` is **absent** while `restricted.anvil.enabled` still shows at 4586,
  which is the control proving the needle shape works;
- the shared-file fence (`wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`) — **empty**, diffed
  against the captured base SHA `a3e68a31` rather than the branch name.

I did not report any figure derived from `--numstat`.

Every capture records `MARKER_HEAD` **and** the built `enabled: global.security.enabled` source
line in the same file, so no run can be attributed to a tree it did not come from.

### An instrument defect I hit, and repaired rather than noted

My first GREEN capture came back with a marker (`WLRC_AGENT`) that I had not written and with my
`###` section markers missing, because of nested-quoting mangling in an `ssh … sh -c "…"` one-liner.
The numbers it reported were exactly the ones I expected, which is precisely when they are least
trustworthy. **I discarded that run** and re-did every measurement through a `scp`'d script file
with unique `MARKER_*` sentinels and no nested quoting; every number above comes from the repaired
instrument. Per §6b-ii I did not merely write the defect up.

### `wcore-agent --lib` is NOT clean — and it is not mine

`cargo test -p wcore-agent --lib` fails at my commit: `2221 passed; 13 failed; 3 ignored; 0
filtered out`. **Reported red rather than hidden.** The control shows it is pre-existing:

- At **base `a3e68a31`** (the original `&&` merge), same host, minutes apart, the same suite fails
  **19** — *more* than mine — and **11 of my 13 appear verbatim** in base's 19.
- The 2 names that failed for me but not at base
  (`resumed_engine_holds_journal_lease_until_drop`,
  `live_session_switch_transfers_journal_authority_and_runtime_state`) **pass 3/3 in isolation at
  base AND 3/3 in isolation at my commit** — `2 passed; 0 failed; 2235 filtered out` each round,
  with the executed count asserted so the filter cannot have silently matched nothing.

All 13 are session-lease / journal-durability / crash-replay tests. My change is a one-line config
merge in a different crate and touches nothing they exercise. This is the wall-clock-and-journal
contention family §6 documents, under five concurrent lanes. Full data:
`evidence/.../05-PREEXISTING-FLAKE-CONTROL.log`.

---

## 7. Deviations, and what I did NOT do

- **Did not ship the fix the brief prescribed.** `global || project` is defective for a
  default-`true` field; measured and documented above. Flagged rather than followed.
- **Did not fix `egress_allow`** — measured, judged by-design and trust-gated, named explicitly in
  §4. Not closed silently.
- **Did not fix the `max_turns`/`max_tokens` ceiling loosening** — MEDIUM, backlog per policy,
  measured and locked in a test in §5.
- **Did not implement the `--i-accept-exfil-risk` interlock** or design toward it. Out of scope by
  instruction; the owner decision stands untouched.
- **Did not commit `Cargo.lock`.** Building surfaced a pre-existing manifest/lock drift:
  `crates/wcore-eval-scenarios/Cargo.toml:123` declares `serial_test` and the `wcore-eval-scenarios`
  block in `Cargo.lock` omits it, so `cargo` rewrites the lock on every build. **Proven
  pre-existing at base** — `git show a3e68a31:crates/wcore-eval-scenarios/Cargo.toml` has the
  declaration, `git show a3e68a31:Cargo.lock` does not have the entry. Out of my scope, but it will
  break any `--locked` build, so **the orchestrator should route it to whoever owns
  `wcore-eval-scenarios`.**
- **Did not run clippy** — not requested in the evidence bar, and a workspace clippy under five
  concurrent lanes is not a measurement.
- **Did not merge to integration, open a PR, tag, close an issue, or run `wcore-contract generate`.**
- No shared-file-fence edits: `wcore-cli/src/lib.rs` and `main.rs` are untouched.
- No protocol or wire-contract change, so nothing here needs serializing against another lane
  beyond the ordinary merge.

## 8. Files

| File | Change |
|------|--------|
| `crates/wcore-config/src/config.rs` | the fix (`enabled: global.security.enabled`), removal of the untrusted-path forward, correction of the assertion that had pinned the defect, and the reasoning for all three |
| `crates/wcore-agent/tests/egress_merge_polarity_test.rs` | new, 8 tests: 2 controls, 2 for the defect, 1 pinning the fix's shape against `\|\|`, 2 for `egress_allow`, 1 for the resource-ceiling sweep result |
| `.planning/phases/25-*/evidence/egress-merge-polarity/` | NOTES + 5 evidence logs |
