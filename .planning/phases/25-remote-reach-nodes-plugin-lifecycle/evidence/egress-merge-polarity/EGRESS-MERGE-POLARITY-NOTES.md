# NOTES — lane/egress-merge-polarity

Base: `a3e68a31e9e63767c505345eb996f5eeab2341f9` (asserted against `git ls-remote gh plan/f20-unified-audit-repair`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-egress-merge-polarity`.

## Brief claims — verification status (read-only pass 1)

| # | Claim | Status | Evidence |
|---|-------|--------|----------|
| 1 | `config.rs:4431` is `enabled: global.security.enabled && project.security.enabled` | **HELD** | verbatim at line 4431 |
| 2 | in-source comment calls it "most-restrictive `enabled`" | **HELD** | line 4421 |
| 3 | `SecurityConfig::enabled` is `#[serde(default = "default_true")]` | **HELD** | line 296-297; `fn default_true() -> bool { true }` line 1181 |
| 4 | `enabled = true` means the egress gate is ON | **HELD (doc)** | line 287 "Master switch for the egress gate. On by default." — still to confirm at the consumer |
| 5 | `read_only: global \|\| project` two lines below | **HELD** (line 4123, in `DefaultConfig`, ~300 lines above not below) |
| 6 | function says twice "a project config is untrusted (checked into a cloned repo)" | **HELD** | lines 4094, 4109, 4141, 4245 (four times) |
| 7 | `--i-accept-exfil-risk` does not exist | to re-verify | doc at 291-295 asserts it doesn't (prior lane `25-c4-egress`) |

## NEW finding not in the brief (pass 1)

`restrict_untrusted_project_config` (line 4525) — the function that exists **specifically** to
neutralize untrusted project configs — **explicitly forwards** the field:

```rust
// A repository may tighten egress and disable Anvil, but cannot add an
// origin, command gate, provider, MCP server, hook or executable skill
// permission until its independently stored fingerprint is trusted.
restricted.security.enabled = project.security.enabled;   // line 4546
```

So there is **no clamp elsewhere**; the untrusted path deliberately carries the loosening value
forward under the belief that it is a *tightening*. This makes the finding worse, not better:
the defect is present on the **untrusted** path too, which is the default state of a fresh clone.

Note `restricted.security.egress_allow` is NOT forwarded → an **untrusted** project probably
cannot append allowlist hosts. Bears on work-order item 4 (`egress_allow`). Must measure.

## Open / next

- [ ] Confirm consumer semantics of `security.enabled` (find the egress gate).
- [ ] Build the real-merge measurement (global ON + project `enabled = false`).
- [ ] Measure `egress_allow` concat on the trusted path.
- [ ] Sweep other security-relevant fields for polarity.
