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

## 4. Judgement call — trust-gated or unconditional? RESOLVED: trust-gated

The neighbouring GHSA-8r7g clamps (`approval_mode`, `auto_approve`,
`allow_list`) are UNCONDITIONAL — they clamp the trusted path too. But
`[budget]` (`max_cost_usd`, `max_wall_time_secs`, …) — a strictly more powerful
resource ceiling, denominated in dollars — is `project.or(global)` with **no
clamp at all** on the trusted path, and is simply dropped on the untrusted one.
That is the closer precedent for a *resource* ceiling.

**Panel (LANE-BRIEF §4): 3/3 for trust-gated.** codex `gpt-5.6-sol`, gemini
`3.1-pro-preview`, kimi K3 all picked trust-gated independently, all three
naming the same strongest reason (a trusted project can already register
`[mcp.servers]` = arbitrary tool execution, so a token ceiling buys no security)
and — unprompted — all three naming the same strongest counter.

**The counter, and why it fails — MEASURED, not argued.** All three dissents
reduce to: *"trust is sticky while repo content is not; a workspace trusted on
Monday still applies on Friday after a hostile PR raises `max_turns`, so the
trusted path is a post-trust escalation channel."* That premise is **false in
this codebase**:

- `fingerprint_workspace` (`workspace_trust.rs:151-218`) hashes the **content**
  of `.wayland-core.toml` itself (`:162` puts it in `candidates`, `:206-211`
  feeds its bytes into the SHA-256).
- `WorkspaceTrustStore::resolve` (`:99-104`) recomputes that digest on every
  resolve and grants trust only on `digest == &fingerprint.digest`.

So the moment a hostile commit edits `.wayland-core.toml` to raise `max_turns`,
the digest changes, the stored grant no longer matches, the workspace reverts to
UNTRUSTED, and the clamp applies. **The escalation channel the dissent posits
does not exist**, and it is closed by the very edit that would exploit it. This
is locked by a test (`raising_the_ceiling_in_a_trusted_repo_revokes_its_own_trust`).

Minority position retained for the record: the internal-consistency argument
(unclamped knob among clamped neighbours) is real but is a *style* cost, and
kimi independently noted that fixing it properly needs a `max_tokens:
Option<u32>` migration to tell "unset" from "64000" — out of scope here, and
recorded as a finding rather than done badly.

## 5. Status — COMPLETE

- [x] premise verified (all six brief claims held)
- [x] full sweep table → SUMMARY §3; found 2 further instances, both measured
- [x] clamp written, trust-gated, + the SMART_MAX_TURNS backstop hole my first draft left
- [x] both-direction controls: 14/14 green, 3 red on revert, 3 red on the wrong-fix variant
- [x] `egress_allow` verdict: agree with the prior lane, by-design → SUMMARY §4
- [x] live-proven on the real binary, both arms, known-positive in the negative arm

Full write-up: `.planning/RESOURCE-LIMITS-CLAMP-SUMMARY.md`.

### Correction to §2 of these notes

§2 said the `max_tokens` presence gate is load-bearing. **Measured false and corrected in-source:**
enumerated over 30 `(project, global)` pairs it is equivalent to an unclamped `min` at that site
(0 differing cases), because the merge's own downstream presence gate already rescues the absent
case. The absent-value landmine is real but bites a different variant — `min` at the MERGE site with
the gate dropped — which regresses 200000 → 64000. Kept the gate, kept the test, fixed the claim.
