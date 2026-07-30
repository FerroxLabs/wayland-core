# NOTES — lane/resource-limits-clamp

Base: integration `b2ddf113`. Target: `BL-UNTRUSTED-RESOURCE-LIMITS` (MEDIUM) plus a
full field-by-field sweep of `merge_config_files_with_trust` /
`restrict_untrusted_project_config`.

## 1. Brief premise verification (do this before acting — LANE-BRIEF)

| Brief claim | Verified at `b2ddf113`? | Evidence |
|---|---|---|
| comment "Resource limits … can only reduce power" | HELD | `config.rs:4554` |
| `max_tokens` at `:4088` never compares against global | HELD | `config.rs:4088-4092`, `if project != default_max_tokens() { project } else { global }` |
| `max_turns` at `:4093` never compares against global | HELD | `config.rs:4093`, `project.or(global)` |
| proven in an existing test 100→999999, 5→100000 | HELD | `crates/wcore-agent/tests/egress_merge_polarity_test.rs:441-471` |
| `[budget]` / `[session_cap]` not forwarded on the untrusted path | HELD | `restrict_untrusted_project_config` sets neither; `restricted` starts from `ConfigFile::default()` |
| sibling fix landed at `f09d5898` | HELD | merge commit, `security.enabled = global.security.enabled` |

Line numbers in the brief are exact at this base. No refutations.

## 2. Type facts that drive the clamp shape (the absent-value landmine)

- `max_tokens: u32`, `#[serde(default = "default_max_tokens")]` → **64000**
  (`config.rs:1144`). An absent project `[default] max_tokens` is therefore
  **indistinguishable from an explicit 64000**, and 64000 is NOT the identity
  element for `min`. A naive `min(project, global)` would let a project file
  that says nothing at all about `max_tokens` CLAMP DOWN an operator global of
  e.g. 200000 to 64000 — the exact mirror of the `global || project` trap that
  was measured defective on `security.enabled`.
  ⇒ The existing `!= default_max_tokens()` presence heuristic must be KEPT as
  the gate, and `min` applied only inside it.
- `max_turns: Option<usize>`, `#[serde(default)]` → **None**. `Option` models
  absence exactly, so there is no landmine here — but note **`None` is the MOST
  permissive value** (`engine.rs:9061-9068` "an OPTIONAL override"; `tui/app.rs:1310`
  "`None` = no cap"). So `Some(p)` with global `None` is a NARROWING and must be
  honoured; only `(Some, Some)` compares.

## 3. Clamp being written

Placed inside `restrict_untrusted_project_config` (which gains a `&global`
parameter) rather than in the merge body, because that is exactly where the
false comment lives and it keeps the clamp trust-gated. See §4 for why
trust-gated and not unconditional.

```
max_tokens: present(project) ? min(project, global) : absent
max_turns : (Some(p), Some(g)) => Some(min(p,g)) ; otherwise project unchanged
```

## 4. Open judgement call — trust-gated or unconditional?

The neighbouring GHSA-8r7g clamps (`approval_mode`, `auto_approve`,
`allow_list`) are UNCONDITIONAL — they clamp the trusted path too. But
`[budget]` (`max_cost_usd`, `max_wall_time_secs`, …) — a strictly more powerful
resource ceiling, denominated in dollars — is `project.or(global)` with **no
clamp at all** on the trusted path, and is simply dropped on the untrusted one.
That is the closer precedent for a *resource* ceiling, so the emerging pick is
**trust-gated**. To be cross-audited (§4 of LANE-BRIEF) before shipping.

## 5. Status

- [x] premise verified
- [ ] full sweep table
- [ ] clamp written
- [ ] both-direction controls
- [ ] `egress_allow` trusted-path verdict
