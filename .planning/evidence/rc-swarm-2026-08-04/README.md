# RC blocker swarm — 2026-08-04, integration `7accc0c1`

Six parallel investigators over the five Windows CI failures, the MCP-uncallable
defect, and an RC-readiness re-grade. Each finding was to be adversarially
refuted as it landed. **Read the disposition column before trusting any of these
— three refuters never ran.**

No patch in this directory has been applied, compiled, or tested. They are
proposals.

## Disposition

| # | Defect | Self-reported | Adversarial pass | Disposition |
|---|---|---|---|---|
| 1 | `wcore-swarm` ×2 — retained checkout descriptor | probable | **refuter DIED** (session limit) | **unchallenged**, but carries real measured Windows evidence |
| 2 | `exec-backend` container `exit-125` | probable | **REFUTED** | root cause rejected — do not build on it |
| 3 | `deterministic_openai_loop` divergence | confident | survived a real refutation | **strongest finding here** |
| 4 | `mcp_assistant_scoping_e2e` unchecked/not_found | partial | **refuter DIED** (session limit) | unchallenged, self-declared partial |
| 5 | MCP tools connect but are uncallable | probable | **refuter DIED**, then **REFUTED BY MEASUREMENT** — see below | root cause rejected |
| 6 | RC-readiness re-grade | partial | survived | actionable, one hard tag blocker |

**A missing verdict is not a pass.** The orchestrating script treated a null
verdict as "survived", which is a defect in the harness, not evidence about the
finding. Rows 1, 4 and 5 were never challenged by anything except, for row 5, a
direct measurement afterwards.

## Row 5 — refuted by measurement, and this matters

The agent's mechanism: `record_hydrated_tools` (`engine.rs:15327`) parses the
ToolSearch body with one `serde_json::from_str` and `else { return; }`. In
production the body first passes `truncate_result` (`orchestration/mod.rs:3380`,
default `max_result_size` = **50,000** chars, which ToolSearch does not
override). A truncated array is invalid JSON, the parse fails, the `else` arm
silently returns, nothing is ever hydrated, and the model loops until the
no-progress guard aborts. The agent honestly flagged as its unknown that it had
never measured the real payload.

**Measured against the live tvcontrol server (101 tools):**

| ToolSearch query | matches | body bytes |
|---|---|---|
| `tv_health_check` | 1 | **283** |
| `health` | 4 | 1,348 |
| `tv` | 14 | 5,891 |
| `chart` | 35 | 17,813 |

The observed failure used the query `tv_health_check` — **283 bytes against a
50,000-char threshold**. Truncation cannot explain it. The root cause is
rejected for the observed failure.

**The mechanism is still a real latent defect** and worth closing on its own
merits: a silent `else { return; }` on an unparseable tool result means
hydration fails invisibly, and a broad query over a larger MCP server can cross
50 kB. But it is not the masterclass blocker.

**So the MCP-uncallable defect remains UN-ROOT-CAUSED.** Anyone picking this up
starts from: hydration demonstrably does not take effect (the identical
ToolSearch result recurs 10 turns running, and `tool_search.rs:87` skips
non-deferred tools, so a landed hydration would have changed the second result),
the payload is far too small to be truncated, and the agent traced
curation → cap → deferral and could not break the admission half. The break is
somewhere between `record_hydrated_tools` being called and the next request's
`tools[]` being built. That is now a narrow search.

## Row 6 — the one hard tag blocker, independently confirmed

`[workspace.package] version = "0.12.25"` (`Cargo.toml:155`), latest tag
`v0.12.25`. `release.yml` derives the release from the tag with no guard, so
tagging `v0.12.26-rc.1` today ships a binary whose `--version` reports `0.12.25`
and contradicts its own signed manifest. Confirmed directly, not taken on the
agent's word. Cheap to fix; must happen before any tag.

The re-grade also reports `27-C4` was declared release-blocking by our own
`CRITERIA-STATUS.md` on 2026-07-31 and never got a row in `RC-READINESS.md`.
That claim has **not** been independently checked.

## Row 2 — refuted, recorded anyway

The adversarial verifier rejected the container-backend root cause with verified
citations. Kept here because the refutation itself is evidence, and because the
next person should not re-derive the same rejected theory.
