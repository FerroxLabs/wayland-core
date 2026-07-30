# SUMMARY — lane/egress-merge-polarity

Base: `a3e68a31` (`plan/f20-unified-audit-repair`, asserted against `git ls-remote gh`).
Lane branch: `lane/egress-merge-polarity`.
Build/test host: `hetzner-dsm`, worktree `/root/wayland-egress-merge-polarity`.

**Verdict: the finding HELD, and it was worse than reported.** Fixed, pinned by seven tests
that run the real load-and-merge path into the real egress policy.

---

## 1. Which of the brief's claims held

| # | Claim | Status |
|---|-------|--------|
| 1 | `config.rs:4431` is `enabled: global.security.enabled && project.security.enabled` | **HELD**, verbatim |
| 2 | The in-source comment calls that "most-restrictive `enabled`" | **HELD** (line 4421) |
| 3 | `enabled = true` means the egress gate is ON | **HELD at the consumer** — `egress/install.rs:28` selects `AgentEgressPolicy::enforcing` vs `::disabled()`, and `policy.rs:143` short-circuits `Off` to `EgressDecision::Allow`. `false` is a literal allow-all. |
| 4 | `SecurityConfig::enabled` is `#[serde(default = "default_true")]` | **HELD** (line 296; `default_true` at 1181 returns `true`) |
| 5 | So the bug needs an explicit `[security] enabled = false` in the project config | **HELD** — an omitted `[security]` deserializes to `true` and the merge is harmless. No path yields `false` by default. Severity is bounded by this. |
| 6 | `read_only: global \|\| project` sits nearby as the correct safety-posture polarity | **HELD** (line 4123), though it is ~300 lines *above*, not two lines below |
| 7 | The function says "a project config is untrusted (checked into a cloned repo)" twice | **HELD, and understated** — four times (4094, 4109, 4141, 4245) |
| 8 | `--i-accept-exfil-risk` does not exist | **HELD** — no such flag anywhere in the tree; the docs at `config.rs:291` and `policy.rs:38` already say so, corrected by lane `25-c4-egress` |

Nothing in the brief was false. One claim was too generous to the codebase — see §2.

## 2. What the brief did NOT have: the untrusted path forwards it deliberately

`restrict_untrusted_project_config` — the function whose entire job is neutralising a project
config from an untrusted workspace — **explicitly forwarded the field**:

```rust
// A repository may tighten egress and disable Anvil, but cannot add an
// origin, command gate, provider, MCP server, hook or executable skill
// permission until its independently stored fingerprint is trusted.
restricted.security.enabled = project.security.enabled;   // line 4546
```

So there was no clamp anywhere; the untrusted path carried the loosening value forward *on
purpose*, under a comment calling it a tightening. An untrusted workspace is the default state of
any freshly cloned repository, so the defect was live on the default path, not a trusted-only edge.

Only two fields could loosen anything across that restriction boundary (full sweep in §6):
`security.enabled`, and the resource caps in §6.

## 3. Measured impact

Both tests below drive `Config::resolve_with_provenance` over two real files on disk, then
`policy_from_config(...).check(...)` with a `POST https://collector.attacker-example.com/ingest` —
a body-bearing request to a non-allowlisted, non-shared-platform host, which the classifier
grades `Exfil` and the policy must `Deny`.

At base `da8e46b5` (`&&`), **5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out**:

```
---- untrusted_project_config_must_not_disable_the_egress_gate stdout ----
thread 'untrusted_project_config_must_not_disable_the_egress_gate' (3262628) panicked at
crates/wcore-agent/tests/egress_merge_polarity_test.rs:274:5:
assertion `left != right` failed: the merged `security.enabled` still equals the pre-fix
`global && project` result, so the polarity fix is not in effect
  left: false
 right: false

---- trusted_project_config_must_not_disable_the_egress_gate stdout ----
thread 'trusted_project_config_must_not_disable_the_egress_gate' (3262625) panicked at
crates/wcore-agent/tests/egress_merge_polarity_test.rs:306:5:
a project `[security] enabled = false` must not disable the operator's egress boundary even in a
trusted workspace
```

A four-line `.wayland-core.toml` committed to a repository reduced the egress policy to allow-all
for anyone who cloned it. Full log: `01-RED-base-da8e46b5.log`.

## 4. The fix — and why it is NOT the brief's `||`

The brief prescribed `global || project`, mirroring `read_only`. **Measured, that is defective, and
I did not ship it.**

`read_only` can use `||` because it defaults to **`false`** — absence is the identity element for
`||`. `security.enabled` defaults to **`true`**, the identity for `&&` and *absorbing* for `||`. So
under `||`, a global `enabled = false` plus any project file that omits `[security]` (which
deserializes to `true`) yields `true` — **silently destroying the operator's documented off
switch.**

Applied `||` in isolation on hetzner and re-ran the same seven tests —
**5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out**:

```
test operator_off_switch_survives_a_project_silent_on_security ... FAILED
test control_operator_global_off_switch_disables_the_gate ... FAILED
...
panicked at crates/wcore-agent/tests/egress_merge_polarity_test.rs:338:5:
a project config that is SILENT on `[security]` must not resurrect the egress gate the operator
switched off globally — `enabled` defaults to true, so `global || project` gets this wrong
```

It closes the exfil hole and opens a correctness hole. Full log: `02-RED-naive-or-variant.log`.

**Shipped instead — operator-owned, the GHSA-8r7g `auto_approve` pattern already in this function:**

```rust
let security = SecurityConfig {
    enabled: global.security.enabled,
    egress_allow: [global.security.egress_allow, project.security.egress_allow].concat(),
};
```

Corroboration that this matches product intent rather than just being convenient: the TUI's egress
toggle and its allowlist editor both persist through **`patch_global_config`**
(`tui/surfaces/config.rs:662,713,719`). There is no surface in the product that writes `[security]`
into a project file. The switch was already operator-owned everywhere except the merge.

The dead-and-misleading forward at `4546` was removed, and its comment now records why Anvil keeps
its forward (`anvil.enabled = false` removes an automation rail — genuinely a narrowing) while
egress does not.

## 5. `egress_allow` — a real second finding, measured, NOT closed by me

It concatenates, so it widens rather than disables. Measured on both trust arms:

- **Untrusted workspace → entries are DROPPED.** `restrict_untrusted_project_config` never forwards
  `security.egress_allow`, so it goes the way of project `[providers]`, `[mcp.servers]`, hooks and
  `tools.skills.allow`. The exfil POST is still denied.
  (`untrusted_project_cannot_append_to_the_egress_allowlist`)
- **Trusted workspace → entries ARE concatenated and the boundary widens.** A project
  `egress_allow = ["collector.attacker-example.com"]` makes that host reachable; the POST returns
  `Allow`. (`trusted_project_egress_allow_widens_the_boundary_by_design`)

**Naming it clearly: this is a distinct finding from the polarity defect, and I did not close it.**
My judgement is that it is by design and should stay. It is gated on the operator having granted
the workspace fingerprint — an explicit act — and it sits in the same trust-gated bucket as project
`[providers]`, `[mcp.servers]`, `tools.skills.allow` and `tools.env_passthrough`. Adding one egress
host is strictly less powerful than adding an MCP server (an arbitrary child process), which the
same gate already permits. Closing it would break the documented operator workflow while leaving a
strictly larger hole open beside it.

The distinction that makes the polarity defect different: `enabled = false` was honoured **on the
untrusted path**, with no operator act at all.

I locked both behaviours in tests so the distinction is explicit rather than ambient. If the owner
disagrees, the change is one line and the test that pins it is named.

## 6. Polarity sweep — every other security-relevant field

The question that matters is which project-supplied values survive
`restrict_untrusted_project_config`, because that is the untrusted default path. Exactly these do:

| Field | Merge | Direction | Verdict |
|-------|-------|-----------|---------|
| `security.enabled` | was `global && project` | **LOOSENS** | **THE DEFECT — fixed** |
| `default.read_only` | `global \|\| project` | tightens | correct |
| `default.approval_mode` | project only if `is_at_least_as_strict_as(global)` | tightens | correct |
| `default.system_prompt` | `neutralize_trust_delimiters` | defanged | correct |
| `tools.allow_list` | intersected with global | narrows | correct |
| `tools.skills.deny` | concatenated | narrows | correct |
| `tools.verify_edits` | `global \|\| project` | tightens | correct |
| `anvil.enabled` | `global && project` | narrows (removes a rail) | correct — opposite polarity to egress, and that is right |
| `observability.skills_lifecycle` | `&&`, only `Some(false)` forwarded | narrows | correct |
| `default.max_tokens` | project wins when non-default | **LOOSENS** | see below |
| `default.max_turns` | `project.or(global)` | **LOOSENS** | see below |

**Second polarity finding, lower severity, NOT fixed by this lane (out of its scope fence):**
`restrict_untrusted_project_config`'s comment claims *"Resource limits and read-only/approval
requests can only reduce power."* For `max_tokens` and `max_turns` that is **false as merged** — an
untrusted project may set either *higher* than the operator's global value and it wins outright.
The blast radius is spend and wall-clock, not exfiltration, so by the brief's severity policy this
is **MEDIUM → BACKLOG, non-blocking**. Flagging it rather than fixing it, because clamping a
resource cap is a behaviour change for existing users and belongs with whoever owns the budget
surface.

Everything else a project can set (`providers`, `profiles`, `mcp.servers`, `hooks`,
`env_passthrough`, `sandbox`, `allow_no_sandbox`, `skills.allow`, `browser.policy`, `crucible`) is
either clamped tighten-only in the merge or dropped wholesale while untrusted. No other
mis-polarised field found.

## 7. Evidence bar

**Three assertions.**
1. *Known-positive passes:* `control_gate_denies_exfil_when_the_project_is_silent_on_security`
   reaches `Deny` — the harness can construct a request that the classifier actually grades.
2. *Known-negative genuinely fails:* pasted verbatim in §3, `left: false / right: false`, from the
   base commit.
3. *The old shape would have missed it:* the test computes `global && project` inline over the same
   inputs and `assert_ne!`s the merged result against it. A test asserting only "denied" would pass
   on any tree where the request never reached the classifier.

**Controls in both directions (§3b-iii).** The gate must be able to reach `Allow` too, or a fix
that hard-wired `enabled = true` would pass everything else here:
`control_operator_global_off_switch_disables_the_gate` reaches `Allow`, and
`operator_off_switch_survives_a_project_silent_on_security` is the case that reddens under `||`.
Both are green under the shipped fix and both are red under a naive one.

**Instrument-liveness checks built into the harness**, not asserted afterwards:
- every load asserts a sentinel `max_tokens = 4321` came back out of the merged config, so a test
  that silently read some *other* global config (hetzner injects into `/root/.wayland`, §3b-ii)
  fails loudly instead of measuring the wrong file;
- every load reads the trust arm back out of the product's own resolution provenance
  (`ConfigSourceDisposition::Restricted`) and asserts it matches the arm under test, so a
  "trusted" case that silently ran untrusted cannot pass.

**Discarded measurement.** The first green run went to a shared `/tmp` path and came back carrying
a foreign `WLRC_B` marker and a duplicated result block — another lane's writer (§6a-ii). Those
numbers are not reported. The run was repeated to `/tmp/lane-egress-merge-polarity-*` and every
phase start marker was asserted present before any count was read.

**My own poll loop had a defect and it is repaired, not just noted (§6b-ii):** it tested
`[ "$R" = "1" ]` against a marker count, so a log containing the marker twice never satisfied it
and the wait ran to the watchdog. Now `-ge 1`, with `grep -c`'s exit-1-on-zero handled so the
fallback cannot append a second line.

## 8. Gates

| Gate | Result |
|------|--------|
| `cargo fmt --all -- --check` (Mac) | rc=0, unpiped |
| `cargo test -p wcore-agent --test egress_merge_polarity_test` | **7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** |
| `cargo test -p wcore-config --lib` | **574 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out** |
| `cargo check --workspace --all-targets` | see `04-WORKSPACE-CHECK.md` |

All cargo invocations used the absolute path `/root/.cargo/bin/cargo` (unproxied, §3b).

## 9. Deviations, and what I did NOT do

- **Corrected an existing assertion.** `untrusted_project_executable_configuration_is_inert_but_narrowing_survives`
  asserted `!merged.security.enabled` — under a test named *"narrowing survives"* it pinned the
  attacker-supplied `false` as a narrowing that ought to survive. That assertion encoded the defect,
  so it now asserts the opposite, with a comment saying why. Flagging it explicitly because
  "changed a test to reach green" is exactly the shape the honesty rules forbid, and a reviewer
  should judge this one rather than take my word for it.
- **Did NOT implement the `--i-accept-exfil-risk` interlock.** Out of scope by instruction; Sean
  chose a two-tier CLI danger design and a separate lane owns it. The fix is independent of it.
- **Did NOT fix the `max_tokens` / `max_turns` loosening** (§6) — MEDIUM, backlog.
- **Did NOT fix the trusted-path `egress_allow` widening** (§5) — judged by design; named, measured
  and locked in a test.
- **Pre-existing lockfile drift, not mine, not touched:** `crates/wcore-eval-scenarios/Cargo.toml`
  declares `serial_test` (line 123) but the `wcore-eval-scenarios` block in `Cargo.lock` omits it,
  so any build dirties `Cargo.lock`. Proven pre-existing at base — `git show a3e68a31` has the
  manifest entry and lacks the lock entry. My change touches neither crate. Left alone per the
  scope boundary; a `--locked` build will trip on it.
- No PR, no merge to integration, no tag, no issue closed, no `wcore-contract generate`, no
  credential used, no `git rebase`/`reset --hard`/`clean`/`stash`.
