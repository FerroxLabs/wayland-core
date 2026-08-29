# THE PLAN — wayland-core 0.13.12

> **GENERATED FILE. Do not edit.** Regenerate with `just plan`. Every fact here is
> joined from `.planning/ledger/` (state), `plan-verification.json` (independent
> verification) and `PLAN-ROUTING.json` (assignment). If this disagrees with anyone's
> recollection, this is right and the recollection is wrong — that is the entire point.

Rendered 2026-08-29 16:15 UTC

## VERDICT: BLOCKED

**221 criteria block the 0.13.12 release.** Full list in §3.

| state | count | means |
|---|---:|---|
| DONE | 30 | met, evidence resolves, independently verified |
| CLAIMED | 177 | met but NOT yet independently verified — never report as done |
| OPEN | 227 | outstanding work |
| HANDOFF | 1 | another team's half, with a filed ticket carrying it |

## §3 BLOCKING — the definition of done for 0.13.12

### `atomic-write` — Atomic publish: a rollback that exchanged nothing is reported as success

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1202 | restore() distinguishes an actual exchange from Swap::Vacant / Swap::Unsupported, and a non-exchange is a restore FAILURE |
| `c2` | wl#1202 | On a restore failure the displaced pre-image is preserved by the existing keep_displaced path, and the caller does NOT report the destination as uncha |
| `c3` | wl#1202 | A test drives a refused publish where the destination name disappears between the two exchanges and asserts the original bytes survive; shown RED agai |
| `c4` | wl#1202 | The existing happy-path, successful-rollback, absent-destination and mode/long-path tests stay green |

### `atref-residuals` — @-ref secret guard: the residuals #339 shipped past

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#322 | The TUI @-ref directory walk gives the same treatment to a store reached under another name |

### `atref-walk` — @-ref directory walk: silent empty payloads and a FIFO wedge

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c3` | core#335 | The decided behaviour is pinned by a test covering both escaping spellings, absolute and ..-relative |
| `c1` | core#377 | @../repo/ and @/abs/outside/ either attach their files or are REFUSED with a message the user sees -- never Ok with files: [] |
| `c2` | core#377 | The `continue` in walk_dir that drops an entry increments the skipped counter, so AtWarning::SkippedFiles is emitted whenever any entry is dropped |
| `c3` | core#377 | A test drives resolve() for both escaping spellings and asserts the outcome, shown RED against today's code |
| `c4` | core#377 | The control stays green: @<absolute path of the workspace root>/ still attaches its files |
| `c1` | core#381 | @./ in a workspace containing a named pipe completes rather than blocking |
| `c2` | core#381 | walk_dir skips any entry that is not a regular file BEFORE admit() opens it |
| `c3` | core#381 | A test plants a FIFO next to an ordinary file, calls resolve(@./, root) under a timeout, and fails if the call does not return; shown RED against toda |
| `c4` | core#381 | Controls stay green: an ordinary file in the same directory is still attached, and a FIFO named .env is still not read |

### `browser-revive` — Browser tool non-functional by default

Runs on: **hetzner**  ·  3-unrouted-pickup

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#113 | The deny-by-default browsing posture is recorded as a decision on the issue and the issue is dispositioned |

### `bwrap-race` — bwrap ownership race with ENOENT, and a containment test that retries into a pass

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#362 | The ENOENT is traced to a named path and a named window: what is resolved, what opens it, and what removes it in between |
| `c2` | core#362 | It is established BY MEASUREMENT whether the race reproduces on a plain Linux host with the sandbox enabled, or only under CI's nested-bwrap-in-docker |
| `c3` | core#362 | If it reaches a plain host, the ownership acquisition is made race-free and a red arm is quoted verbatim from before the fix |
| `c4` | core#362 | bwrap_confines_filesystem_writes_outside_allowlist cannot retry into a pass having never run its probe: a backend that refuses to start the probe fail |
| `c5` | core#362 | Measured at --retries 0 over N >= 20 on the CI image, with the rate recorded |

### `cache-truth` — Cache and spend ledgers: legacy decode, false invalidations, session keys

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1203 | install_spend_guard receives the authoritative budget_session_id() at all three call sites, not uuid::Uuid::new_v4() at two of them |
| `c2` | wl#1203 | A test asserts the SpendAuditRecord.session_id written on a fresh construction, on a resume, and after rebind_provider are the SAME id |
| `c3` | wl#1203 | A live run is quoted: one conversation's records in ~/.wayland/budget/spend-audit.jsonl share one key across a /model switch and across a --resume |
| `c1` | wl#1205 | LEDGER_SCHEMA is bumped, and a v1 row's uncached_equivalent_usd: 0.0 no longer decodes as a genuine priced zero |
| `c2` | wl#1205 | cache report over a v0.13.9-format ledger no longer prints saving_truth=priced with a negative saving; the run output is quoted |
| `c3` | wl#1205 | cache list's store total does not sum a legacy 0.0 into the counterfactual, and cache verify does not return trustworthy=true for it |
| `c4` | wl#1205 | A test reads a fixture ledger in the v1 on-disk shape and asserts the verdict; shown RED against today's #[serde(default)] decode |
| `c1` | wl#1206 | A turn whose cache_read is unchanged from the previous turn while total input grows is not attributed to TtlExpiry |
| `c2` | wl#1206 | No InvalidationCause::Expired is written to the cache ledger for such a turn |
| `c3` | wl#1206 | A test drives the measured sequence -- three turns at cache_read=40,000/input=500, then one at cache_read=40,000/input=150,000 -- and asserts Healthy; |
| `c4` | wl#1206 | genuinely_healthy_trace_stays_healthy and warm_session_healthy_ratio_does_not_warn stay green, and at least one new case sits at the boundary rather t |
| `c1` | wl#1207 | A decision is recorded for compact.cache_diagnostics defaulting to false: on, off, or off with the reason stated |
| `c2` | wl#1207 | Whichever way it goes, the ledger entry for wayland#1166 carries a criterion covering it, so the ticket's fifth numbered defect is visible in the 'all |
| `c6` | wl#559 | The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1 |

### `channel-caps` — Message caps: matrix/msteams probe shape, Telegram UTF-16, WhatsApp

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#360 | The bridge's cap is measured against a real baileys or whatsapp-web.js backend, or the borrowed Some(4096) is replaced by something honest |
| `c8` | wl#934 | Telegram's unit question is settled: the cap is characters or UTF-16 code units, measured rather than assumed |

### `ci-evidence` — CI evidence: the wrapper that cannot write, the floor that cannot see, the anchors that cannot rot

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c2` | core#325 | A run whose sibling job failed posts a red report instead of closing the tracker green |
| `c3` | wl#1134 | A lint catches the class in CI, with a paired-direction self-test run immediately before it |
| `c1` | wl#1177 | A failure on attempt 1 followed by a pass on attempt 2 leaves evidence the required report check can read |
| `c2` | wl#1177 | A test demonstrates that visibility and fails against today's workflow |
| `c3` | wl#1182 | The test no longer needs its entry in .config/flaky-allowlist.txt under the #1169 retry-flake gate |
| `c1` | wl#1197 | Either the lint audits writes inside helper functions reachable from a test (the enclosing-fn machinery and closure() are already present), or the exc |
| `c2` | wl#1197 | --self-test carries a helper fixture in BOTH directions, so the classifier's blind spot cannot be reintroduced silently |
| `c3` | wl#1197 | Removing #[serial_test::serial] from the caller of PinnedRetryBudget::pin at engine.rs:29581 makes the lint exit non-zero -- or #1134 c3's text is res |
| `c1` | wl#1198 | A file: anchor carries a required content fragment that must be present at or near the named line, or bare line anchors are refused on files above a s |
| `c2` | wl#1198 | Both #1134 anchors are re-anchored to something that resolves to what the criterion claims |
| `c3` | wl#1198 | --self-test proves both directions: an anchor whose content moved goes red, and a correct anchor stays green |
| `c1` | wl#1215 | run-tests-with-attempt-evidence.sh creates its attempt directory successfully on the hosted runner in the presence of a root-owned target/ left by the |
| `c2` | wl#1215 | A real CI run is cited BY URL in which 'Run tests (nextest CI profile)' executes and uploads junit.xml |
| `c3` | wl#1215 | In that same run the rm -f at wrapper:82 and the cp at wrapper:90 succeed -- an attempt file is preserved and named |
| `c1` | wl#1216 | The evidence floor is per-leg: a leg that uploads zero junit files fails the required report check rather than being covered by another leg's upload |
| `c2` | wl#1216 | Preserved outer-attempt-*.xml files are not counted toward the coverage figure they are meant to prove |
| `c3` | wl#1216 | A test under .github/scripts/tests/ drives both directions and is wired into lint.yml so a failure reds the step |
| `c1` | wl#1220 | .config/flaky-allowlist.txt on the integration branch no longer carries the gh#1182 line |
| `c2` | wl#1220 | The allowlist is graded against the MERGED tree rather than against a commit hash: a check refuses an entry whose owning ledger criterion claims it wa |
| `c3` | wl#1220 | The check is proven in both directions, including a resurrection introduced by a MERGE -- git log -S skips merges by default, which is how this one pa |

### `cli-messages` — User-facing message shape: collapsed whitespace in three crates

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1204 | The message at cache_cmd.rs:486 and the sibling bail at :476 read as one sentence with single spaces between words |
| `c2` | wl#1204 | A test asserts the message SHAPE, not only that it contains 'cache list' |
| `c3` | wl#1204 | The same collapse at acp.rs:266 and main.rs:8616/:8624 is fixed in the same pass, or a lint refuses a user-facing string literal containing three or m |

### `container-latch` — Container backend latches on a leftover name, and attests a run that never happened

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#365 | A container left in Created under the name a new task would take does not fail that task -- either the name carries the nonce, or the submit path clea |
| `c2` | core#365 | A daemon-level refusal is not reported as a task exit: exit-125 from docker run produces a distinct outcome, never a receipt asserting the task ran an |
| `c3` | core#365 | The daemon's stderr reaches the operator on a daemon-level failure rather than being captured into a receipt nobody reads |
| `c4` | core#365 | A red arm is quoted verbatim: the new guard reverted, the test failing, restored and green, with the mutation shown to have landed on code |
| `c5` | core#365 | conformance_matrix passes on a host that has run it before with a leftover container present -- the regression test creates the wedged container itsel |
| `c6` | core#365 | The orphan-scan path is checked for the same latch: state whether docker ps -a --filter label=wayland.task.nonce= would have found these two, and if n |

### `container-latch-2` — Container orphan scan is nonce-scoped and can never see an earlier run's leftover

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#366 | The product can enumerate wayland-created containers WITHOUT being given a nonce -- a key-presence scan reachable from an operator-facing surface, not |
| `c2` | core#366 | The nonce-scoped scan_orphans(nonce) contract is left intact for cancel(); the unscoped scan is an addition, not a widening, and the moved and unmoved |
| `c3` | core#366 | An operator surface reports a leftover it did not create in this process -- one whose nonce is not in the live registry |
| `c4` | core#366 | The conformance check at conformance.rs:340 either gains an arm that plants a labelled container and requires the scan to FIND it, or its limits are s |
| `c5` | core#366 | A regression test plants a labelled leftover under a nonce the running process has never used and asserts the unscoped scan reports it, creating and c |

### `decompose` — File each cross-team remainder as its OWN ticket with a contract

Runs on: **gh**  ·  2-decompose

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#314 | A grant refusal is machine-readable rather than untyped English prose in an Info frame |
| `c2` | wl#1088 | The user-visible half — the chat interface no longer reports Read/Glob/Write/Edit as restricted |
| `c3` | wl#1151 | The transcript stops assembling out of order |
| `c4` | wl#388 | The remaining four bullets of this ticket's own Expected Behavior list are met |
| `c3` | wl#434 | The alias-resolves-server-side path is closed end to end |
| `c5` | wl#998 | Desktop sends the per-tool field on the ACP path |

### `desktop-run` — Live Desktop session measurement

Runs on: **Desktop app**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | wl#559 | This ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read |

### `doc-truth` — Shipped operator docs that overclaim what the code does

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#340 | The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution |

### `flake-584` — Shared-process lib suite: the #584 fixture misses its truncation boundary under load

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#361 | The mechanism is named: what makes output lack truncated under load is identified in code, not inferred |
| `c2` | core#361 | The failure is reproduced deliberately at least once, with the command and environment recorded, before any fix is written |
| `c3` | core#361 | The fixture reaches the truncation boundary deterministically, independent of scheduling |
| `c4` | core#361 | Both assertions survive: the anti-vacuity control at mod.rs:5744 and the fragment assertion at :5749 |
| `c5` | core#361 | A red arm is quoted verbatim: the fixture failing before the change, from a real run |
| `c6` | core#361 | After the fix, cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on the build host, and the run count is recorded |

### `flux-contract` — Anvil/Elevation loop-ownership contract

Runs on: **hetzner**  ·  3-unrouted-pickup

| criterion | issue | what must become true |
|---|---|---|
| `c3` | wl#863 | F1 confirmed for the current deployment: Elevation is unreachable by default from flux-fast, flux-standard, flux-reasoning and flux-auto |
| `c4` | wl#863 | F3 server half: requests carrying loop_owner or a client nonce bypass or vary the Flux semantic cache |
| `c5` | wl#863 | F4: the bandit routes loop_owner requests to a tool-calling-capable arm, or a flux-agentic alias with that guarantee exists |

### `gepa-selection` — GEPA online evolution: close the selection loop before defaulting it on

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#372 | The online path scores the CANDIDATE, not the session: incumbent and child are scored on the same observations and the child is only eligible if it be |
| `c2` | core#372 | The online path goes through PromptStore::record_variant + CuratorPort::submit -> Decision::Promote\|Archive, never a bare file; the $WAYLAND_HOME/evol |
| `c3` | core#372 | Promotion requires accumulated evidence, not n=1; the threshold is stated and justified |
| `c4` | core#372 | The mutator does not use the session's frontier model, and the per-session cost is measured and recorded |
| `c5` | core#372 | Selection is tested BOTH ways with the fixture-replay provider: child better -> Promote, child worse -> Archive, child equal -> Archive |
| `c6` | core#372 | A two-session integration test proves the loop CLOSES: session 1 promotes a variant, session 2's SkillRouter demonstrably uses it |

### `instrument-integrity` — Prove the instruments can fail: mutation + measurement arms

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c5` | core#352 | A red arm is quoted verbatim for each platform arm |

### `instrument-integrity-3` — Scoped-subscriber ERROR visibility: name the mechanism, measure at n>=100

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#373 | The mechanism is named in code: what makes the ERROR not reach the scoped subscriber -- not inferred |
| `c2` | core#373 | A red arm is quoted verbatim from a real run, and the rate is measured at n >= 100 on the same instrument as the fix arm |
| `c3` | core#373 | The fix arm scores 0 failures at n >= 100 on that same instrument, and the baseline is re-measured at the same n in the same session |
| `c4` | core#373 | Both osv_check log-visibility tests keep their exact-equality assertion on [Level(Error)]; weakening either is refused |
| `c5` | core#373 | cargo test --workspace --lib --no-fail-fast passes N >= 10 consecutive times on hetzner-dsm, with the run count recorded (core#361 c6, handed over int |

### `json-stream-consent` — json-stream: an egress consent the host is never shown

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1219 | An EgressVerdict::Ask on the json-stream path reaches the host as an approval request the host can answer |
| `c2` | wl#1219 | No path installs BridgeConsentDoorbell where the sink cannot emit -- either with_hitl_suspend is called on the json-stream sink, or the doorbell is no |
| `c3` | wl#1219 | A consent that was never shown is never reported to the user as 'declined at the consent prompt' |
| `c4` | wl#1219 | A test drives the json-stream sink through an egress Ask and asserts an ApprovalRequired frame is written; shown RED against today's hitl_suspend_enab |

### `macos-ci` — macOS arms via the lane/** CI wildcard

Runs on: **macOS CI**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#352 | macOS: the pgrep arm is EXECUTED in CI at least once with the run cited, or deleted as unreachable |

### `mcp-gate-mode` — MCP malware gate: explicit permissive/strict operator choice

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c7` | core#354 | The already-shipping non-session MCP launch path reads the operator's chosen mode, not the uninstalled permissive default |

### `mcp-transports` — tools/list_changed on the SSE and Streamable-HTTP transports

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1175 | An MCP server attached at runtime has its tools/list_changed honoured, or the product plainly says it will not be |
| `c1` | wl#1213 | An SSE server's notifications/tools/list_changed reaches McpManager::refresh_signalled_tools, or the product says plainly at attach time that this tra |
| `c2` | wl#1213 | The same disjunct is satisfied for StreamableHttpTransport |
| `c3` | wl#1213 | SseTransport's listener no longer drops id-less frames unconditionally; a test asserts a notification frame is dispatched |
| `c4` | wl#1213 | If take_tools_changed is implemented for StreamableHttpTransport, is_alive/close are fixed in the same change and RemoveMcpServer withdraws the entry, |

### `model-limits` — Model limits: host-variable open-weights ids and the provider-blind ceiling

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c5` | wl#1176 | The third preserved rule holds: no static arm is added for an open-weights family served at wildly different limits by different hosts |
| `c1` | wl#1214 | The minimax-m* and deepseek-v4* arms either resolve per provider, or are removed under the rule host_variable_open_weights_stay_unknown already enforc |
| `c2` | wl#1214 | The passthrough module doc's claim 'No open-weights family is listed here' is either true or removed |
| `c3` | wl#1214 | PASSTHROUGH_IN_SCOPE's minimax/deepseek floor patterns no longer instruct a release owner to add an arm for a family whose hosts disagree |
| `c4` | wl#1214 | The host spread measured here is reproduced against a fresh models.dev pull and recorded, so the decision is made on data rather than on the arms that |

### `plugin-quarantine` — Plugin quarantine: teardown after setsid, and the Windows primitive

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c2` | core#338 | Any prompt raised inside a quarantine operation is distinguishable by the user from a prompt raised by Wayland itself |
| `c1` | core#379 | The quarantine timeout path kills the whole session/process group it created, not the direct child alone |
| `c2` | core#379 | A test spawns a quarantine git child that backgrounds a descendant, trips the timeout, and asserts no descendant survives; shown RED against today's k |
| `c3` | core#379 | The teardown decision is written down beside the setsid decision (DECISIONS.md Q-338c4 or its successor), as MASTER-PLAN.md:202 required |

### `prompt-cache` — Prompt-cache collapse and re-billed context

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c4` | wl#1150 | Accumulated prior tool RESULTS are not re-sent whole on every turn, and prompt/KV cache is reused where possible |

### `provider-urls` — Provider endpoints: Anthropic /v1 doubling and the self-hosted locality predicate

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1211 | https://api.openai.com?x=@127.0.0.1 and https://h?a=@10.0.0.1 are classified NOT self-hosted |
| `c2` | wl#1211 | The authority is parsed with a URL parser, or cut at the first of '/', '?' and '#' |
| `c3` | wl#1211 | Both spellings are added to the_locality_predicate_rejects_public_hosts, which is shown RED against today's rsplit('@') |
| `c4` | wl#1211 | With every credential variable unset, --base-url 'https://api.openai.com?x=@127.0.0.1' refuses to start; the run is quoted |
| `c1` | wl#1212 | A keyless self-hosted council member is not classified CouncilProviderError::Keyless |
| `c2` | wl#1212 | The two gates consult ONE predicate rather than two re-implemented chains, or a test asserts they agree on identical config |
| `c3` | wl#1212 | A test drives a council member at a keyless self-hosted endpoint and asserts it is not skipped; shown RED against today's code |
| `c1` | wl#1217 | https://api.anthropic.com/v1 and https://api.anthropic.com/ as base_url both produce POST /v1/messages |
| `c2` | wl#1217 | The joiner is wcore_config::compat::join_endpoint, the one #1178 already added, not a second bespoke trim |
| `c3` | wl#1217 | anthropic.rs:676 (/v1/models) and cohere.rs:136 are fixed in the same pass, or each is stated as out of scope with the reason |
| `c4` | wl#1217 | A test asserts the built URL for both spellings; shown RED against today's format! |

### `reasoning-history` — Reasoning filter writes the durable record: truncation and the missing flush

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1221 | The input measured here -- 'Use the <thinking> tag to wrap reasoning. Then answer.' -- survives intact in the stored assistant text |
| `c2` | wl#1221 | An unclosed reasoning tag does not silently eat the remainder of the DURABLE record: either the filter applies to display only, or an unclosed open is |
| `c3` | wl#1221 | If any content is dropped from stored history the user is told; the empty-turn notice at engine.rs:15941 is not the only guard |
| `c4` | wl#1221 | A test asserts the stored assistant ContentBlock::Text for that input; shown RED against today's engine.rs:14786 |
| `c1` | wl#1222 | 'the answer is 5 <' and 'result: <th' survive byte-exact through a completed turn |
| `c2` | wl#1222 | The filter gains a flush/finish on its public surface and the engine calls it at turn end |
| `c3` | wl#1222 | The decision for a pending InThinking buffer at end of stream is recorded rather than left implicit |
| `c4` | wl#1222 | The controls stay byte-exact: 'if a < b then', 'if a <b then c', '<div>hello</div>' |
| `c3` | wl#908 | The remaining reported sub-symptom is reproduced and addressed |

### `small-window` — Small served/configured windows: notice truth, reserves, ceilings, budgets

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#382 | The notice's wording is true when it is emitted: conditional on sizing_window() being Some and on supports_compaction(served), or it says plainly that |
| `c2` | core#382 | When corroboration lands and sizing actually moves, the user is told -- the once-per-figure suppression at context_window.rs:355 does not swallow the  |
| `c3` | core#382 | A test asserts the notice TEXT against the sizing state in the same body, for both the first-regression and the corroborated case; shown RED against t |
| `c5` | wl#1150 | Tool schemas AND SKILLS are injected only when relevant or explicitly activated, rather than on every ordinary chat turn |
| `c3` | wl#1172 | COMPENSATION: the learned window feeds the pre-flight guard and autocompact, so the truncation stops |
| `c2` | wl#1179 | The autocompact threshold on a small window sits above core's own baseline turn rather than below it |
| `c1` | wl#1199 | get_char_budget(None) no longer grants a budget derived from 200k: the unknown-window case is sized against UNVERIFIED_CONTEXT_WINDOW or refuses to gu |
| `c2` | wl#1199 | A test boots the bootstrap prompt with known_context_window = None and asserts the skills budget; shown RED against today's DEFAULT_CHAR_BUDGET |
| `c3` | wl#1199 | The measured arithmetic here is reproduced after the change: an unlisted model's skill listing no longer receives 8,000 chars while every other bounda |
| `c1` | wl#1200 | The tool-result budget is derived from the resolved context window when one is known, with today's constants as the unknown fallback |
| `c2` | wl#1200 | A test asserts that on a 32,768-token window the worst-case carried payload (total_budget_bytes + keep_recent x max_result_size) fits the window; show |
| `c3` | wl#1200 | The ledger entry for wayland#1150 c4 states WHICH guarantee is delivered -- carried bytes stop growing, or carried bytes fit the window -- rather than |
| `c1` | wl#1210 | emergency_limit_tokens resolves through the same narrowed window as resolve_preflight_window and autocompact_threshold_now, or the exemption is docume |
| `c2` | wl#1210 | A test asserts that on a corroborated 8,192 learned window the reported emergency limit and the autocompact threshold derive from the same window |
| `c3` | wl#1210 | wayland#1172 c3's ledger note stops claiming the guard, the trigger and the reported threshold cannot disagree -- or is made true |
| `c1` | wl#1218 | size_output_cap and scaled_reserves agree: the max_tokens sent is never larger than the reserve the ceiling withheld |
| `c2` | wl#1218 | A test asserts reserve >= the max_tokens that will be sent, across the window range 4,096..49,152 for an unlisted model; shown RED against today's UNK |
| `c3` | wl#1218 | The measured cases here no longer hold: learned 8,192 -> ceiling 5,053 / reserve 2,730 / max_tokens 8,192, and configured 16,384 -> 10,104 / 5,461 / 8 |

### `telegram-topic` — Telegram forum-topic target sent as reply_to_message_id, never message_thread_id

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c6` | core#363 | Discord does not regress: a thread channel id never reaches message_reference |

### `test-instruments` — Instruments that cannot fail: the wrapping ratchet and the 60s test

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c2` | core#336 | The post-resize predicate can only be satisfied by a frame that is actually 80 columns wide |
| `c1` | core#378 | The runtime is ATTRIBUTED by measurement: whether the >30s is product latency on the spill/read-back path or the fixture's own cost, established rathe |
| `c2` | core#378 | Either the test completes inside the default profile's 30s slow-timeout, or it carries an explicit per-test budget with the reason recorded in .config |
| `c3` | core#378 | If it stays load-sensitive it is listed in .config/flaky-allowlist.txt with an expiry and a measured rate, so an unlisted flake cannot redden the requ |
| `c1` | core#385 | The ratchet cannot pass while OwnedTree's descendant walk is a stub -- either it asserts a behavioural property, or #352/#1156 stop citing it as class |
| `c2` | core#385 | A red arm is quoted: with the descendant walk neutered, the gate cited as class closure goes RED |
| `c3` | core#385 | harness_owns_spawned_trees cannot be skipped, quarantined or made platform-conditional without the ratchet going red too |
| `c2` | wl#1155 | The same guarantee holds on Windows |

### `test-instruments-2` — Harness ownership on Unix, and the guard against merging a red arm

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c2` | core#367 | A red-arm instrument cannot be merged by accident: a test or CI grep fails when a shipped source file under crates/ contains black_box(true) or a RED  |
| `c3` | core#367 | Whoever integrates NAMES the failing tests in a run rather than counting them |
| `c1` | core#371 | The Linux descendant snapshot (or the fixture's ordering, whichever the measurement indicts) is fixed, with which one established BEFORE either is cha |
| `c2` | core#371 | The Windows twin's anti-vacuity shape is kept: the grandchild is asserted to be inside what the guard owns BEFORE anything is killed, so the test cann |

### `tool-wire-order` — Tool wire order and the frozen system prefix

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c1` | wl#1208 | A session that crosses midnight either reports the real date or stops telling the model the baked date is authoritative |
| `c2` | wl#1208 | The channel-gateway engine pool is covered: a long-lived per-channel engine does not answer date-bound questions with the day the gateway started |
| `c3` | wl#1208 | A test drives a day rollover against the prompt builder and asserts the outcome; current_date_present_and_stable_in_cached_system_prefix is restated t |
| `c1` | wl#1209 | In stub mode a hydration leaves the wire prefix stable: the measured turn1/turn2 arrays no longer differ at index 1 |
| `c2` | wl#1209 | A test asserts prefix stability under hydration in BOTH catalog modes, with the catalog=true path as the positive control; shown RED against today's s |
| `c3` | wl#1209 | If the shift is deliberate in stub mode, the config key's documentation says that turning the fold off also gives up cache stability, and the user is  |

### `vfs-store-reach` — VFS/policy: store reach through GrepTool, the weaker resolver, and a dead predicate

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c3` | core#244 | The store is also unreachable to a shell subprocess, not only to the in-process VFS |
| `c4` | core#323 | A rule added to the @-ref guard's own file-name check is also honoured by the tools deny walk |
| `c2` | core#339 | The @dir walk decides recursion on symlink_metadata and computes rel_to_root from the canonical path |
| `c4` | core#356 | If both resolvers remain, the reason each call site picked one is stated AT the call site |
| `c1` | core#375 | Grep(pattern, path='.git') and Grep(pattern, path='.svn') return no bytes from .git/lfs/objects or .svn/pristine under WorkspacePolicy::contained |
| `c2` | core#375 | A test drives the production contained stack (SandboxedFs over SecretDenyFs over RealFs) for both parent-named spellings and is shown RED against toda |
| `c3` | core#375 | The admitted-file decision uses the same predicate the VFS deny uses, so a store reached under any parent name is covered -- not a second name list in |
| `c4` | core#375 | The negative controls stay green: Grep(path='.') still withholds ignored matches, and an ordinary in-workspace search is unchanged |
| `c1` | core#376 | The per-operation cost is MEASURED before anything changes: a benchmark or a counted-syscall figure for Read/exists/list/metadata on an ordinary path, |
| `c2` | core#376 | If the measurement shows the cost is material, arm 2 of is_vcs_content_store no longer rebuilds the store list on the common path -- either an early-o |
| `c3` | core#376 | A test pins the number of canonicalize/exists calls for one ordinary-path guard, so the regression cannot return silently |
| `c4` | core#376 | If the measurement shows the cost is NOT material, this issue is closed with the figure recorded rather than left open on a code reading |
| `c1` | core#383 | policy.is_project_secret(<in-root dangling symlink to a missing .env>) returns true |
| `c2` | core#383 | A test asserts all three arms measured here -- live link to an existing .env, dangling link to a missing .env, and the direct name -- shown RED agains |
| `c3` | core#383 | workspace_policy.rs no longer carries two resolvers with different escape properties, or every remaining site names which resolver it uses and why |
| `c1` | core#384 | Either is_session_write_granted is the predicate the mutating VFS path actually asks, or it is deleted together with the two tests that grade it |
| `c2` | core#384 | If it is deleted, the doc comment claiming it is the enforcement point goes with it and SandboxedFs::contain_granted/live_grant_roots carries that doc |
| `c3` | core#384 | A grep gate or a test proves no other documented-but-uncalled enforcement predicate remains in workspace_policy.rs |

### `win-owned-tree` — OwnedTree kills the process tree on Windows, not the leaf

Runs on: **hetzner**  ·  1-build

| criterion | issue | what must become true |
|---|---|---|
| `c6` | core#358 | clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -D warnings is clean |

### `win-quarantine` — Windows: what a quarantine child can still do to the console

Runs on: **SeanDesktop**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#380 | A Windows test drives harden_against_credential_prompt and establishes what a quarantine child can do to the user's console, exercising both AllocCons |
| `c2` | core#380 | Either the Windows arm delivers the same elimination property as the unix arm -- the child cannot put a prompt on the user's console -- or the product |
| `c3` | core#380 | The Windows result is quoted VERBATIM from a real run on SeanDesktop; a cross-compile is not a runtime proof |

### `win-runs` — Windows measurement arms - serialize, ONE box

Runs on: **SeanDesktop**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c4` | core#238 | A Windows probe records whether bare NUL is a device on the build under test and whether fs::metadata reports is_file() true for it |
| `c1` | core#324 | An instrumented run establishes whether the failure is a product race in AppContainer ACE application or a race in the test fixture |
| `c2` | core#324 | concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N of at least 20 on the AppContainer-capable host |
| `c3` | core#324 | Whichever arm the measurement indicts, the deny half of the test is still non-vacuous afterwards |
| `c3` | core#342 | The same guarantee holds on Windows, where the product ships |
| `c5` | core#342 | The two arms asserting a Unix-only guarantee state the Windows truth, from a measured Windows rate |
| `c5` | core#350 | The issue's own close condition is met: a green nightly-windows-soak run against this tree |
| `c2` | core#358 | A test grades the grandchild case ON WINDOWS: a direct child with a detached grandchild, guard dropped while unwinding, both gone afterwards |
| `c3` | core#358 | The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change |
| `c5` | core#358 | The CI run that executed the Windows arm is cited by URL |
| `c5` | wl#1164 | A live confirmation run on SeanDesktop is recorded before the change lands |

### `win-runs-2` — Windows measurement arms from the 0.13.12 sweep - serialize, ONE box

Runs on: **SeanDesktop**  ·  2-platform

| criterion | issue | what must become true |
|---|---|---|
| `c1` | core#368 | concurrent_allow_and_deny_identities_do_not_interfere passes at retries=0 over N >= 20 on an AppContainer-capable host |
| `c2` | core#368 | The deny half stays non-vacuous: a change that makes the allow arm pass by making the deny arm stop denying fails this issue |
| `c1` | core#369 | A lease that cannot be recovered is quarantined and reported, not retried forever at the cost of the whole backend |
| `c2` | core#369 | is_available() == false can say WHY without the caller having to provoke an execute() |
| `c3` | core#369 | Whatever recorded \\?\C:/Users/<user> as a single allow intent is found and closed |
| `c4` | core#369 | A decision is recorded for the ACEs already leaked onto the home directory of any machine that hit this |
| `c1` | core#370 | The two named arms pass at retries=0 over N >= 20 on Windows, OR they are gated with the measured Windows rate recorded and a separate arm grading wha |
| `c2` | core#370 | A negative control proves the silent-degrade path is observable when it fires |
| `c1` | core#374 | The test has a Windows arm that produces a genuinely non-NotFound fs::metadata failure -- an over-long path, a path under a directory the process cann |
| `c2` | core#374 | The existing ENOTDIR provocation is kept for Unix |
| `c3` | core#374 | The premise assertion is NOT weakened to make it pass: a test that stops checking its premise is the vacuity this one was written to avoid |

## §4 DECOMPOSED — another team's half, tracked

These are NOT partials. Core's half is closed; the remainder is filed against a named
owner with its own contract. A blocked criterion with no ticket does not appear here —
it appears in §3 as blocking, because that is what it is.

| criterion | issue | owner | carried by |
|---|---|---|---|
| `c5` | wl#934 | maintainer | FerroxLabs/wayland#1186 |

## §5 CLAIMED BUT UNVERIFIED — 177

Marked `met` with resolving evidence, but no independent verifier has confirmed the lane.
Historically this is exactly where a partial hides: a criterion written thin reads `met`
while the reported bug is still live. Do not report these as done.

- **core#113** — c1, c2, c3, c4
- **core#238** — c1, c2, c3, c5, c6
- **core#244** — c1, c2
- **core#253** — c1, c2, c3, c5, c6, c7
- **core#314** — c1, c2, c3, c4
- **core#322** — c1, c2, c3
- **core#323** — c1, c2, c3
- **core#325** — c1, c3, c4
- **core#335** — c1, c2, c4
- **core#336** — c1
- **core#337** — c1, c4
- **core#338** — c1, c3, c4
- **core#339** — c1, c3, c4, c5, c6
- **core#340** — c2, c3, c4, c5
- **core#342** — c1, c2, c4
- **core#350** — c1, c2, c4
- **core#352** — c1, c2, c3
- **core#353** — c1, c2, c3, c4
- **core#354** — c1, c2, c3, c4, c5, c6
- **core#356** — c1, c2, c3
- **core#358** — c1, c4
- **core#360** — c3, c6
- **core#363** — c1, c2, c3, c4, c5
- **core#367** — c1
- **wl#174** — c1, c2, c3, c4, c5
- **wl#305** — c1
- **wl#388** — c1, c2, c3
- **wl#434** — c1
- **wl#559** — c1, c2
- **wl#863** — c1, c2
- **wl#908** — c1, c2
- **wl#934** — c1, c2, c3, c4, c6
- **wl#998** — c1, c2, c3, c4
- **wl#1088** — c1
- **wl#1134** — c1, c2, c4, c5
- **wl#1150** — c1, c2, c3
- **wl#1151** — c1
- **wl#1155** — c1, c3
- **wl#1156** — c1, c2
- **wl#1161** — c1, c2
- **wl#1162** — c1, c2
- **wl#1163** — c1, c2, c3
- **wl#1165** — c2
- **wl#1166** — c1, c2, c3, c4, c5
- **wl#1168** — c1, c2, c3
- **wl#1170** — c1, c2, c3, c4
- **wl#1171** — c1, c2, c3
- **wl#1172** — c1, c2
- **wl#1173** — c1, c2, c3
- **wl#1174** — c1, c2, c3
- **wl#1175** — c2, c3
- **wl#1176** — c1, c2, c3, c4
- **wl#1177** — c3
- **wl#1178** — c1, c2, c3, c4, c5
- **wl#1179** — c1, c3, c4, c5
- **wl#1180** — c1, c2, c3, c4
- **wl#1181** — c5
- **wl#1182** — c1, c2
- **wl#1186** — c1, c2, c3, c4, c6

## §6 OUT OF SCOPE for 0.13.12 — feature work

Excluded by explicit instruction: defects ship, feature requests wait. The work still
gets built and its branch pushed; it just does not gate this release.

- **wl#305** — [Feature]: improve Win/WSL interop

## §7 DONE — verified

Every criterion met, evidence resolves in the tree, and an independent adversarial
verifier re-ran the gate and confirmed it.

- **core#253** — c4
- **core#336** — c3, c4
- **core#337** — c2, c3
- **core#350** — c3
- **core#353** — c5
- **core#355** — c1, c2, c3, c4
- **core#360** — c2, c4, c5
- **wl#434** — c2
- **wl#559** — c3, c5
- **wl#934** — c7
- **wl#998** — c6
- **wl#1151** — c2
- **wl#1155** — c4
- **wl#1164** — c1, c2, c3, c4
- **wl#1165** — c1
- **wl#1181** — c1, c2, c3, c4

