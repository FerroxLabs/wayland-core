# NOTES — lane/fix-tui-tool-results (UAT-T3)

Append-only working notes. Committed early and re-committed after every measurement
(LANE-BRIEF §6b-i). Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab`.

---

## T0 — premise verification (brief's traced root cause)

The orchestrator brief says its own measurements are probably stale and must be
re-verified (LANE-BRIEF "Your brief's MEASUREMENTS are probably stale"). Every claim
below was re-read at the lane base commit.

### Claim 1 — `crates/wcore-tools/src/bash.rs` returns plain text, not JSON. **TRUE.**

Three construction sites, byte-identical format string:

```
crates/wcore-tools/src/bash.rs:225   output_to_result()          (sandbox non-streaming)
crates/wcore-tools/src/bash.rs:450   Tool::execute (streaming)
crates/wcore-tools/src/bash.rs:665   Tool::execute_with_ctx
```

all building

```rust
let content = format!("Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}", exit_code, stdout, stderr);
ToolResult { content, is_error: exit_code != 0 }
```

### Claim 2 — `toolcard.rs:267` `parse_payload` degrades non-JSON to `Value::String`. **TRUE.**

```rust
fn parse_payload(card: &ToolCardModel) -> Value {
    match card.output.as_deref() {
        None | Some("") => Value::Null,
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
    }
}
```

`Value::String::get(_)` is always `None`, so every `payload.get("…")` in every
formatter returns `None` for a plain-text payload.

**NEW (not in the brief): `parse_payload` is DUPLICATED.** A second, identical copy
lives at `crates/wcore-cli/src/tui/surfaces/workspace.rs:3446` as
`parse_card_payload`, and that is the copy the *inline transcript* path uses
(`push_tool_card_lines`). The widget copy in `toolcard.rs` serves the card widget.
Both must be fixed, or the defect survives on one of the two render paths.

### Claim 3 — `tool_formatters/bash.rs:29-38` yields `"?"` / `0` / `0`. **TRUE.**

```rust
let cmd = str_or(payload, "cmd", "?");
let exit = i64_or(payload, "exit_code", 0);
let stdout_bytes = payload.get("stdout").and_then(Value::as_str).map(|s| s.len()).unwrap_or(0);
format!("Ran `{}` · exit {} · {} bytes", preview, exit, stdout_bytes)
```

Produces exactly ``Ran `?` · exit 0 · 0 bytes`` on a `Value::String` payload.
`detail_lines` reads `str_or(payload, "stdout", "")` → empty → **zero detail lines**,
which is why actual stdout is never shown.

### Claim 4 — the unit tests feed the formatter an invented payload. **TRUE.**

`tool_formatters/bash.rs` tests all construct `json!({"cmd":…,"exit_code":…,"stdout":…})`.
No test anywhere feeds a formatter the string its real tool produces.

---

## T0b — the full chain, end to end (established, not assumed)

1. `BashTool::execute*` → `wcore_types::tool::ToolResult { content: "Exit code: …", is_error }`
2. `wcore-agent/src/orchestration/mod.rs:3202-3220` — `execute_tools` emits
   `ProtocolEvent::ToolResult { output: content.clone(), output_type: Text, metadata: None }`
3. `wcore-cli/src/tui/protocol_bridge.rs:508` — `card.output = Some(output)`
4. `parse_payload` / `parse_card_payload` → `Value::String(...)`
5. formatter reads `.get(...)` → `None` → defaults

**Finding not in the brief:** `ProtocolEvent::ToolResult` already carries an unused
`metadata: Option<Value>` field (`wcore-protocol/src/events.rs:673`), and **every**
emit site in the product passes `metadata: None`. That is a fourth possible contract
(structured data on a side channel the model never sees) — but it is a wire-format
change to a contract-tested event, so it is not free. Recorded for the cross-audit.

**Finding not in the brief:** `cmd` is not present in the bash tool result *at all*
and never could be — the command lives in the tool **input**, not the output. So
option (a) "tools emit structured JSON" would still not supply `cmd` unless the bash
tool started echoing its own input back. Meanwhile the compact card ALREADY renders
the input: `render_compact` builds `<icon> <name>(<args>) · <summary>` where `<args>`
is `card.input_pretty`. So the command is on screen twice, once truthfully from the
input and once as a fabricated `?` from the formatter.

---

## Status

- [x] Premise verified (4/4 brief claims TRUE, 2 additions found)
- [ ] Task 1 — live pty repro BEFORE
- [ ] Task 2 — 13-formatter audit table
- [ ] Task 3 — cross-audit + contract decision
- [ ] Task 4 — can-fail test

---

## T1 — live repro: the 12-formatter audit result

Full table in the SUMMARY. Headline: **11 of 12 formatters mismatch the payload their
real tool produces.** Only `generic` (shape-agnostic) does not, and it still fabricates
`completed in 0.0s`. Field sweep with known-positive controls in `field-sweep.txt`.

## T1b — the live pty harness, and an instrument defect I created and repaired

`ttr-drive.sh`, derived from the merged `fix-tui-first-message` lane's harness.

**Instrument defect found on the first BEFORE run and repaired in-lane (§6b-ii).**
My first `extract_card` assumed the compact widget's ONE-line shape and matched any line
containing both `Bash(` and `·`. The path the TUI actually uses
(`workspace.rs::push_tool_card_lines`) renders **two** lines:

```
     ● Bash({ "command": "echo LINUX_UAT_TOKEN" }) · done      <- header: has Bash( and ·
       Ran `?` · exit 0 · 0 bytes                              <- body: the formatter summary
```

The old matcher locked onto the **header**, whose `· done` chip is correct on a broken
build. It would have graded the defect **ABSENT on a completely unfixed binary** — a
false green. Verified against the original UAT capture
`.planning/evidence/uat-tui-unix/l3-tui-turn.log:226-227`.

Repaired, and the self-test now carries FOUR assertions: known-positive, known-negative,
**the old matcher would have missed it**, and **it can read a fixed build** (§3b-iii — a
gate with no reachable pass state is as useless as one with no reachable fail state).

## T1c — BLOCKER on the live repro, and a SEPARATE product defect it exposed

The model will not execute the requested Bash call. Root cause measured, not guessed:

`bootstrap.rs:3139` mounts a `FileWatcher` on cwd unconditionally (no config knob). The
engine drains it per turn and bundles a synthetic user message —
`` User edited `<path>` while I was thinking — re-read it before proceeding `` — into the
user's turn. **The model answers that instead of the prompt.**

Measured across four configurations:

| cwd | config | external-edit injections | model called Bash? |
|-----|--------|--------------------------|--------------------|
| `/root/fixtui-scratch` | defaults | 8 | no |
| `/root/fixtui-scratch` | `[memory] enabled=false`, `[session] enabled=false` | 2 | no |
| `/root/fixtui-ro` (chmod 555, one pre-existing file) | same | 3 | no |
| `/root/fixtui-scratch` via TUI + warm-up turn | same | (not captured) | no |

The reported event path is the **watched directory itself** (`/root/fixtui-scratch`), not
a file under it. That is why `watch.rs::is_wcore_internal_path` — which walks components
looking for a `.wayland-core` / `.wayland` segment — never fires: creating
`.wayland-core/` inside cwd bumps **cwd's own mtime**, and the event path that surfaces
is the parent, which contains no internal-directory component.

So the engine reports **its own writes** to the model as user edits, on the first turn of
every session in a fresh directory. This is NOT this lane's defect and is NOT being fixed
here; it is reported as a finding.

Note also `[memory] enabled = false` did not stop `.wayland-core/memory/memory.db{,-wal,-shm}`
from being created — recorded, not investigated.

---

## FINAL RECORD

**Verdict: goal achieved.** Root-caused, fixed, class-audited, live-proven both directions.

### Contract decision — option D, panel unanimous 3/3 + own adversarial pass
Tools keep emitting what they emit (the model's view is untouched, zero wire risk); the
display layer parses it and never invents. (A) rejected on blast radius — `content` IS the
model's context. (C) needs a forbidden contract-fixture regeneration.
Panel verbatim: `panel-{codex,gemini,kimi}.txt`. All three legs probed alive with a real
question first; the initial attempt returned 0 bytes from all three (a redirect artefact)
and was discarded as a dead instrument rather than recorded as silence.
My adversarial pass dissented on one point the others under-weighted — D leaves the
coupling implicit and able to drift silently — and the amendment was adopted: the
regression test DERIVES its input from the producer (executes the real `BashTool`) instead
of pasting its format string, so a pasted fixture cannot become a second invented shape.

### Audit result: 11 of 12 formatters mismatched their real tool (full table in the report)
Beyond bash: every successful **web search** rendered `Found 0 results` and contributed
nothing to the Sources block (rows live at `data.web`, not `results`); `image_gen` never
surfaced the generated image URL for the same reason.

### Live proof (hetzner, real pty, real provider, real approval flow)
BEFORE `6b9b14fd…`:  `Ran ? · exit 0 · 0 bytes`  (both success AND failure; the failure
card's header said `error` while its body said `exit 0` — SELF_CONTRADICTION=YES)
AFTER  `6cd48e52…`:  `exit 0 · 15 bytes stdout`  /  `exit 2 · 0 bytes stdout · 64 bytes
stderr` + `ls: cannot access '/no-such-dir-9F3A': No such file or directory`
All four captures re-graded through ONE repaired grader: BEFORE=DEFECT_PRESENT ×2,
AFTER=DEFECT_ABSENT ×2.

### Can-fail proof, both directions on the real document
Fix reverted -> `FAILED. 2 passed; 4 failed`, messages printing the real defect string from
an actual BashTool execution. Restored via `git checkout -- <path>` -> `ok. 6 passed; 0
failed; 0 ignored; 0 filtered out`; tree clean, HEAD unchanged.

### Gates
fmt --check 0 | metadata --locked 0 | check --workspace --all-targets 0 |
clippy -p wcore-cli --all-targets -D warnings 0 |
test -p wcore-cli --no-fail-fast: 60 binaries, lib 1930 passed / 0 failed / 1 ignored /
0 filtered out; new binary 6 passed / 0 failed / 0 ignored / 0 filtered out.
3 failures, ALL pre-existing — the lane base `--no-fail-fast` yields the identical three
names. Zero new failures.

### Second instrument defect, repaired in-lane
The grader treated `0 bytes` as a defect signature. True BEFORE, FALSE AFTER — a command
that legitimately prints nothing scored as PARTIAL, a false red. Re-keyed on the
fabrication itself. (First defect: the extractor read the header, not the body, and would
have graded a totally unfixed build as CLEAN.)

### Unrun cells — a skip is not a pass
Windows 0 of everything (hetzner cannot reach seandesktop). macOS 0 of everything (Mac
cannot build; not Darwin-specific so the narrow exception does not apply) — **the live
proof is Linux only**. 1 pre-existing binary runs 0 of 10 tests (all `#[ignore]`d);
2 pre-existing binaries have 0 tests. 9 of 12 formatters have no executed-tool case.

### Findings handed off, NOT fixed here
1. MEDIUM `surfaces/mod.rs::await_session_switch` bounds its wait in 100 `yield_now()`
   calls, not a deadline — it cannot distinguish broken from busy. Measured: 4 then 3
   `*_f14` failures under subprocess load; base green 3/3, module-disabled green 2/2.
2. MEDIUM `bootstrap.rs:3139` — the engine reports its OWN writes to the model as user
   edits. Creating `.wayland-core/` bumps cwd's mtime; the surfaced event path is the
   PARENT, which has no `.wayland-core` component, so `is_wcore_internal_path` never
   filters it. The model answers the injected notice instead of the prompt. 8/2/3
   injections across three configs including a chmod-555 cwd. Cost this lane ~1 hour.
3. LOW/open — no tool card appeared under `--dangerously-skip-permissions` across ~90
   frames; with the approval flow it appears at once. Those turns were also looping, so
   this is NOT established. Stated as an open question.
4. LOW — `[memory] enabled = false` did not stop `.wayland-core/memory/memory.db` being
   created. Observed, not investigated.
