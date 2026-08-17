# Answer — Desktop's two MCP-scoping questions

**Date:** 2026-08-08
**Asked by:** Wayland Desktop lane
**Answered by:** Core lane (`area:core`)
**Tree read:** `origin/main` @ `be503d76`

Both answers are grounded in the code named below, not in intent. Where the
honest answer is "undecided", it says so and gives the options with a pick.

---

## 0. What the mechanism actually is

Three call sites, one type. Read these before the answers.

| Site | Code | Effect |
|---|---|---|
| The attribute | `crates/wcore-config/src/config.rs:205` — `McpServerConfig::only_for_assistant: Option<Vec<String>>` | Per-server allow-list of assistant identities. `#[serde(default)]`, so absent ⇒ `None`. |
| The predicate | `config.rs:224` — `is_visible_to_assistant(active)` | `None` **or** `Some([])` ⇒ `true` (global). `Some(list)` ⇒ `active.is_some_and(|a| list.contains(a))`. Fail-closed for `None`/unknown. |
| The filter | `config.rs:255` — `McpConfig::servers_for_assistant(active)` | Retains config servers whose predicate passes. |

The filter is applied at exactly **two** config-injection choke points:

- `crates/wcore-agent/src/bootstrap.rs:1640-1643` (non-deferred connect), and
- `crates/wcore-cli/src/main.rs:4518` (the `#551` deferred-connect path).

The identity itself is `--assistant NAME` / `WAYLAND_ASSISTANT`
(`crates/wcore-cli/src/main.rs:299-306`), threaded to both sites.

Runtime (`add_mcp_server`) declarations take a **different** path:

- `crates/wcore-cli/src/main.rs:3597-3605` — `scope_host_runtime_mcp` refuses
  without a non-blank identity, then calls
  `McpServerConfig::scoped_to_assistant(Some(active))` (`config.rs:211`), which
  **overwrites** `only_for_assistant` with exactly `[active]`.
- `main.rs:4863` is the sole caller; on refusal it emits an `error` frame and an
  `mcp_failed{name, reason}` and `continue`s.
- The connect that follows (`main.rs:4952`,
  `McpManager::connect_all_with_policy(&single_configs, …)`) hands the manager a
  one-entry map **directly**. It does not go through `servers_for_assistant`.

### The consequence that matters, stated plainly

**For a runtime-added server, the `only_for_assistant` value that
`scope_host_runtime_mcp` computes is never read as a gate inside the emitting
process.** Its only consumers are
`crates/wcore-cli/src/runtime_diagnostics.rs:104` (`record_runtime_declaration`,
which hardcodes `visible_to_assistant: true`) and `insert_declaration`'s
`assistant_scoped` field (`runtime_diagnostics.rs:167-170`), which is reported
to the host in `runtime_diagnostics_snapshot`.

So today the requirement is an **identity-provenance assertion**, not an access
control. It guarantees every runtime declaration carries the identity that
created it; it does not currently prevent anything from being used. That does
not make it wrong — it makes it a contract we are choosing to hold before the
paths that would need it exist (persistence of a runtime declaration into
config; a future multi-assistant process). It does mean nobody should describe
it as a security boundary.

---

## Q1 — Is mandatory assistant scoping the intended long-term contract for host-provided runtime MCP?

### Answer: keep it for 0.12.26 — but the reasoning below is weaker than the first draft claimed, and there is a third option that was missed.

Recommendation: **keep the refusal for this release**, because it ships, it is
fail-closed, and changing host-visible behaviour again inside a patch release is
worse than documenting it. But this is now a *provisional* pick, not a settled
contract, and Sean should read option (c) below before treating it as one.

Reasons, in order of weight — two of which have been corrected downward:

1. **It attaches an identity to a wire-added declaration.** Before
   `scoped_to_assistant`, a runtime server is byte-identical to a global config
   server (`to_mcp_server_config`, `main.rs:3569-3592`, sets
   `only_for_assistant: None`); afterwards the diagnostics snapshot can name
   *which* assistant it belongs to.
   **Correction to an earlier draft of this note and of the release notes:** this
   is *not* what distinguishes a runtime declaration from a config one. That is
   carried by `McpDeclarationOrigin::RuntimeCommand`, set by
   `record_runtime_declaration` (`runtime_diagnostics.rs:98-106`) with no
   reference to any assistant identity, and it survives whatever we decide here.
   So "provenance would be lost" is not an argument for refusal — only "the
   server would not be scoped" is.
2. **The alternative is *not* a binary between refuse and go-global.** An earlier
   draft claimed making the identity optional means `only_for_assistant: None`,
   i.e. the v0.12.25 global behaviour. That is false, and the counter-example is
   already in this codebase: the TUI's equivalent path,
   `scope_tui_runtime_mcp` (`tui/engine_bridge.rs:832-842`), scopes an
   identity-less runtime add to a private sentinel
   `"\0wayland:tui:standalone-runtime"` — neither refused nor global. The server
   works, and it is scoped to something no config file can name (a `\0` prefix is
   unwriteable in TOML), so it cannot be widened by accident.

   This means Core currently behaves **two different ways for the same
   operation**: the TUI accepts and sentinel-scopes, the json-stream host is
   refused. That inconsistency is itself undocumented and is exactly the class of
   defect this whole exercise was opened to fix.
3. **The refusal is already observable.** It emits both an `error` and an
   `mcp_failed` (`main.rs:4863-4870`); `mcp_failed` is in the shipped contract at
   `criticality: "safety"`. Desktop confirmed it handles both. The cost of the
   strictness is therefore paid in a frame a conformant host already renders.

The correction: **stop calling it enforcement.** Per §0, nothing in-process
consults the value. Core will document it as a provenance requirement. If we
later want it to be enforcement, the work is to route the runtime connect through
the same `servers_for_assistant` choke point the config path uses — that is a
real change, not a doc change, and it is not scheduled.

### The three options, costed

| | Behaviour with no identity | Cost | Verdict |
|---|---|---|---|
| **(a) Refuse** — ships today | `add_mcp_server` rejected; `error` + `mcp_failed` | Breaks every 0.12.25 host that never passed `--assistant`. Fail-closed and loud. | **Provisional pick for 0.12.26.** It is what shipped. |
| **(b) Go global** | `only_for_assistant: None`, v0.12.25 behaviour | Silently widens scope for a host that simply forgot a flag. | Rejected. Wrong direction for a default. |
| **(c) Sentinel-scope** — *already implemented for the TUI* | scoped to `"\0wayland:tui:standalone-runtime"`-style private owner | The server works, is scoped, and cannot be named by any config file. Host needs no flag. Cost is one more scope value to reason about, and it makes "who declared this" less specific than a real identity. | **The strongest candidate for 0.12.27**, and the one the first draft failed to consider. It removes the breaking change entirely while still being fail-closed. |

The honest position: (a) is what we shipped and it is defensible, but (c) would
have achieved the same scoping goal *without* a breaking change, and Core is
already running (c) on the TUI path. If Sean prefers (c), the change is small —
give `scope_host_runtime_mcp` the same sentinel fallback
`scope_tui_runtime_mcp` already has — but it is another host-visible behaviour
change and should not be made twice in one release.

### Q1b — Does a host-supplied PER-CHAT assistant identity deliver the per-chat MCP narrowing `--mcp-server` / `--no-mcp-servers` was asking for?

### Answer: no. That request is **not** retirable. It should stay open.

This is the load-bearing half of the question, so here is the reasoning
explicitly.

`only_for_assistant` lives on the **server declaration**, in config, written
ahead of time. `servers_for_assistant` matches the live identity against that
pre-written list. Therefore:

- An **unmarked** config server is global (`config.rs:227`, `None | Some([]) ⇒
  true`). No value of `--assistant` — per-chat or otherwise — removes it. A
  per-chat identity narrows nothing for the servers most users actually have.
- A **marked** config server is matched against a list a human typed into
  `config.toml`. A per-chat identity is generated at runtime
  (`chat-7f3a…`), so it can never appear in that list. Switching Desktop from a
  constant `"wayland-desktop"` to a per-chat identity would make every marked
  server **disappear from every chat**, not narrow to some of them.

Per-chat identity is thus strictly worse than the constant Desktop passes today:
it costs them their marked servers and buys no narrowing. Desktop is right to
have withheld the commitment.

There is a second reason the two ideas do not substitute. Assistant scoping is a
**declaration-time attribute** answering "who may ever see this server". A
per-chat selection is a **session-time selector** answering "which of the servers
I may see do I want this run". Those are different questions and one cannot be
made to answer the other without a default-deny flip, which §Q2 rejects.

One caveat, stated so it is not mistaken for a general property: within the
*runtime* path, a per-chat identity would be harmless, because Desktop spawns a
Core process per chat (the v0.12.26 notes describe Desktop's "per-chat generated
workspace" at `docs/releases/v0.12.26.md:413`) and a runtime server is added
to that process only. But harmless is not the same as useful, and the config-side
damage above happens in the same process.

**Recommendation to Desktop:** keep passing one constant identity
(`"wayland-desktop"`, or one per shipped persona if you ever want config-level
persona scoping). Do **not** move to per-chat identities. Keep the
`--mcp-server` / `--no-mcp-servers` request open; Core should implement it as a
session-level selector (see Q2, option A).

---

## Q2 — Should `only_for_assistant` stay restrict-only?

### Answer: yes, restrict-only. The asymmetry is intended — but it was never written down, and it is not sufficient on its own.

The asymmetry Desktop identified is exactly right and is visible in one line
(`config.rs:227`): `None | Some([]) ⇒ true`. An unmarked server is always
injected, so `only_for_assistant` can express "only these identities may see
server X" but can never express "identity Y sees only servers X and Z". It is a
subtractive marker, not an allowlist.

Keeping it that way, because the alternative is a silent, unbounded regression:

- Every existing `config.toml` in the field has unmarked servers. Flipping the
  default to deny removes **all** of them from **every** session at once, with
  no error — the same "session with zero tools" failure this finding is about,
  at a much larger blast radius.
- The fail-closed direction that already exists is the one worth having: a
  *marked* server is hidden from an unidentified session
  (`is_visible_to_assistant(None) == false`). That protects the case the
  attribute was built for (`#111`: a read-only Concierge diag server that must
  not leak to a bare CLI). Nothing about that case wants default-deny.

But restrict-only genuinely cannot express Desktop's need, so the answer is not
"restrict-only, therefore you are done" — it is "restrict-only, and the exact
allowlist belongs at a different layer."

### The options, with the pick

**A — (Recommended) Keep `only_for_assistant` restrict-only; add a session-level selector.**
`--mcp-server <name>` (repeatable) and `--no-mcp-servers`, applied at the same
two choke points as `servers_for_assistant`, composed as an intersection *after*
the assistant filter. Default-deny **by construction** when either flag is
present, and no change at all when absent.
Why: exactly answers "this chat gets these servers"; zero effect on existing
configs; no new config surface; the two filters compose without either changing
the other's meaning. It also directly closes Desktop's original request rather
than redirecting it.
Cost: one CLI surface, two call sites, plus the protocol question of whether the
same selection should be settable over the wire (probably yes, later).

**B — Add an `mcp.default = "allow" | "deny"` config key.**
Makes `only_for_assistant` a true allowlist when `deny`.
Rejected: it is *config*-level, so it still cannot vary per chat — it does not
solve the question that was asked. It also puts a foot-gun in a file an
untrusted workspace already tries to influence (`[mcp.servers.*]` is stripped
from untrusted workspaces per v0.12.26 trust gating; a `default = "deny"` key
would need the same treatment and one more reviewer to remember it).

**C — Add `except_for_assistant` (a deny-list twin).**
Rejected: two overlapping attributes with an undefined precedence when both are
set, and it still cannot express an exact allowlist — only "all but these".

**Pick: A.** B and C both add config surface without answering the per-chat
question; A answers it and leaves `only_for_assistant` semantically untouched.

### Status of the pick

Option A is a **recommendation, not a scheduled commitment.** No Core issue is
open for it as of this note. The decision it needs from Sean is whether it lands
in 0.12.27 or later; the design above should not need revisiting either way.

---

## What Core has already changed as a result

Documentation only — the mechanism is untouched, per Desktop's own statement
that `scope_host_runtime_mcp` and its `mcp_failed` reporting are correct.

The undocumented breaking change is now stated in all four places a host
integrator reads:

- `docs/releases/v0.12.26.md` — Upgrading table + a paragraph.
- `docs/releases/v0.12.26-desktop-integration.md` — §5 behaviour-change row.
- `docs/json-stream-protocol.md` — §2.8 `add_mcp_server`, with the refusal frames.
- `docs/mcp.md` — a new "Assistant scoping" section covering both the config
  attribute and the runtime requirement.

Those four documents are held to the source string by
`crates/wcore-protocol/tests/host_runtime_mcp_assistant_docs.rs`, which extracts
the refusal reason out of `crates/wcore-cli/src/main.rs` and requires each
document to quote it. Rewording the refusal reddens the docs.
