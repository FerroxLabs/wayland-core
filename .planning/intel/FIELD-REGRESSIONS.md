# Live Field Regression Register — CTRL-03

Current packaged/runtime evidence outranks historical acceptance. An old green receipt does not close a newer contradictory report.

Statuses: `OPEN`, `REPRODUCED`, `ROUTED`, `FIXED`, `PACKAGED_PROVEN`. A source fix or unit test cannot skip directly to `PACKAGED_PROVEN`.

| ID | Symptom | Likely seam | Status | Admission route |
|---|---|---|---|---|
| FIELD-MAC-001 | macOS sandbox blocks ordinary developer tools, browser/loopback, or hides the actionable allow path | Core policy + protocol telemetry + Desktop control | OPEN | reproduce on shipped bundle; route by enforcement vs presentation owner |
| FIELD-MCP-001 | connected MCP is absent from session tool manifest or stdio launcher cannot inherit the configured executable environment/PATH | Core MCP lifecycle/readiness + Desktop launch/config | OPEN | deterministic stdio/late-MCP fixture and packaged host replay |
| FIELD-CONFIG-001 | Desktop-hosted sessions and raw Core resolve different config roots/overrides without visible effective-source truth | Core config precedence + Desktop lowering | OPEN | paired standalone/host fixture and effective-config receipt |
| FIELD-SPACE-001 | temporary workspaces accumulate or retention/pruning truth is unclear after chats are deleted | Core workspace lifecycle + Desktop UX | OPEN | clock-controlled retention/restart/prune corpus |
| FIELD-MEDIA-001 | image understanding/generation or MCP image creation is visible but lacks usable credential/readiness propagation | Core provider/MCP capability truth + Desktop credential lowering | OPEN | built-in/MCP-only/late-MCP packaged corpus in Phase 27 |
| FIELD-SPAWN-001 | two parallel `Spawn` siblings both die on a journal-head CAS collision and the loser's budget authority is left PERMANENTLY FAULTED | Core session journal reducer (`session_journal/reducer.rs:708`) | FIXED — Linux re-proved, **Windows unverified** | measured against the shipped binary during Phase 21's attribution corpus; admitted under the CTRL-01 contradictory-live-evidence clause |
| FIELD-JOURNAL-002 | a session journal already on disk carrying an explicit `"effect_receipt":null` fails its checksum on read — silent, permanent user data loss | Core session journal serialization round-trip (`session_journal/model.rs`, `snapshot.rs`) | PACKAGED_PROVEN | reproduced deterministically in Phase 23B, recovered against CI release artifacts on all three platforms with pre-fix negative controls |

Each entry must gain exact version, platform, reproduction, owner, fix candidate, focused proof, packaged proof, and limitation before closure. These findings do not derail unrelated Phase 20 source packets unless reproduction proves a Phase 20 invariant is false.

---

## Entries owed by the CTRL-01 admission rule

`COMPETITIVE-LEDGER.md`'s admission rule: *"Contradictory live/customer evidence reopens
the row **and enters `FIELD-REGRESSIONS.md`**."* The **TXN-\*** row was demoted from
`EFFECTIVE` to **`REACHED — REOPENED`** on 2026-07-28 (panel unanimous 4-0) on the strength
of the two defects below, both measured **against the shipped binary** after `EFFECTIVE`
had been awarded. The ledger recorded its companion action as owed and unperformed — *"both
defects belong in `FIELD-REGRESSIONS.md`, which this lane does not own"* — and **CTRL-01
cannot close until these entries exist.** They exist now. CTRL-01 remains open on the
Windows re-proofs named under each.

### FIELD-SPAWN-001 — parallel `Spawn` siblings die on a journal-head CAS collision

| Field | Value |
|---|---|
| **Finding ID** | `F21-04-03` (HIGH), raised in `phases/21-child-authority-and-budget-inheritance/21-04-ATTRIBUTION-RESULTS.md` |
| **Exact version** | measured at SHA `f2d186f6` during Phase 21 plan 04, against the **shipped binary** driving the product's advertised fan-out path |
| **Platform** | **Linux** (`hetzner-dsm`) **3 of 8** live runs — a race, 4 completing normally. **Windows** (`SeanD@seandesktop`) **6 of 6** json-stream runs: at that SHA two parallel siblings did not once complete on that platform |
| **Symptom** | `tool_result` reports `2 of 2 sub-agent(s) failed or terminated early`; each sibling's terminal carries `Sub-agent error: Session persistence authority unavailable: budget journal operation failed: invalid journal state transition: budget authority prior cursor does not match the current journal head`. The loser does **not** retry — its terminal reads `budget authority is permanently faulted`. The session is then left carrying `turn … has nonterminal tool execution`, so the failure is not even clean |
| **Reproduction** | drive two parallel `Spawn` siblings against the shipped binary in one turn |
| **Seam** | `crates/wcore-agent/src/session_journal/reducer.rs:708` — rejects a budget-authority append whose `prior_cursor.journal_sequence` no longer equals `state.last_seq`. Two concurrent siblings each capture the head; the second loses |
| **Owner** | Core |
| **Fix candidate → fix** | make the head capture atomic with the append. **Landed at `1eb9b5ca`**: `build_and_append` now calls `journal.append_built_from_head(...)`, moving the head read **inside** the append's writer-lock acquisition; the read-modify-write that spanned two acquisitions is gone. Confirmed by reading the code at HEAD, not from the commit message (`21-REVERIFICATION.md` §1) |
| **Focused proof** | `21-REVERIFICATION.md`, graded at `ac94b1d5`: **6 of 6** live two-sibling json-stream runs produced two distinct `parent_call_id`s with **both** siblings serving turns. Linux moved 3/8 → 0/6 |
| **Packaged proof** | **NOT TAKEN on Windows.** `21-REVERIFICATION.md` lists it in its own `behavior_unverified_items`: run two parallel `Spawn` siblings against the shipped binary on `SEANDESKTOP`, 24 iterations, expecting 0/24 permanently faulted where the pre-fix SHA measured **23/24**. Windows hardware was not reachable from that lane |
| **Limitation** | the fix is platform-neutral (an atomic capture-and-append inside one writer-lock acquisition) and Linux moved decisively, **but that is inference, not measurement, on Windows.** Status stays `FIXED`, not `PACKAGED_PROVEN` |
| **Why it reopened TXN-\*** | the `9821ef76` seal did **not** falsify this — **it never ran two parallel siblings.** Phase 21's attribution corpus was the first thing on this program to do so. The tri-family proof that earned `EFFECTIVE` therefore never covered the defect; `EFFECTIVE` was **over-stated rather than invalidated** |
| **Blast radius** | Phase 22 supervises fleets of children. Per `21-04-PHASE-VERDICT.md`: any Phase 22 fan-out proof hits this before it hits anything of its own |

### FIELD-JOURNAL-002 — legacy explicit-null `effect_receipt` fails its checksum on read

| Field | Value |
|---|---|
| **Finding ID** | `23B-H1` (HIGH), `phases/23B-continuous-agency/23B-H1-DISPOSITION.md`, residual closed in `23B-H1-RECOVERY-SUMMARY.md` |
| **Exact version** | pre-fix binaries at `source b75e640c` (pre-dates even 23B-H1's own write fix); post-fix at `source 4b9512a0` (Linux, built on `hetzner-dsm`) and `source 12bd0834` (macOS/Windows CI artifacts) |
| **Platform** | Linux, macOS (arm64), Windows (x64) — all three |
| **Symptom** | both `effect_receipt` fields are `Option<serde_json::Value>` with `skip_serializing_if = "Option::is_none"`, so `Some(Value::Null)` writes an explicit `"effect_receipt":null`, decodes back to `None`, and re-serializes to nothing. The recomputed checksum covers different bytes and **the reader rejects a journal the writer wrote correctly — permanently**, since every operator verb reads the journal. **Silent, permanent user data loss in the product whose durability claim rests on journals surviving** |
| **Reproduction** | deterministic. `scripts/f23-h1-legacy-journal.py` plants the artifact; a fresh reader then fails `ChecksumMismatch { seq: 1 }` end to end through the real writer |
| **Owner** | Core |
| **Fix candidate → fix** | **write path** fixed first (`23B-H1-DISPOSITION.md`), red before green by reverting only the predicate and keeping the tests: **2 failed / 3 passed** before, **5 passed / 0 failed** after. **Read path / residual** fixed in `crates/wcore-agent/src/session_journal.rs`, `session_journal/model.rs`, `session_journal/snapshot.rs` (commits `308832f4`, `4b9512a0`, `24e6694e`, `d070cfbe`, `d1719ece`) — **without loosening the integrity check**; the obvious repair (teaching the check to accept two encodings) was explicitly rejected, because an integrity check with a compatibility branch is one an attacker gets to choose the branch of |
| **Focused proof** | `hetzner-dsm` @ `4b9512a0`: `session_journal` **66 passed**, `session_journal_test` **48**, `session_journal_crash_matrix_test` **4**, `journal_envelope_roundtrip` **5** (23B-H1's own invariant tests, untouched), `session_journal_compaction_test` **24**; `cargo nextest -p wcore-agent --profile ci` **2928 passed / 0 failed**; clippy `-D warnings` clean |
| **Packaged proof** | **taken, on all three platforms, in both directions** — six rows, each a CI release artifact whose `--build-info` source SHA was asserted: pre-fix binaries **PASS the negative** (exit 1, `journal checksum mismatch at sequence 1`) on Linux, macOS and Windows; post-fix binaries **read the same journal, exit 0**, on Linux, macOS and Windows. Nonce generated by the caller at run time and planted in the session id, so a stale log cannot pass. Logs: `phases/23B-continuous-agency/evidence/23Bb-h1-{linux,macos,windows}-drive.log` |
| **Limitation** | **`--resume` itself was not driven** — the recovery sits in `parse_complete_frames`, which every reader including `--resume` traverses, but the literal invocation needs a real provider, so it is inferred from a shared code path, not observed. **The unit suite ran on Linux only** (CI on that branch fails earlier at a **pre-existing** `Check Desktop protocol contract corpus drift` gate that fails identically on the untouched base — run `30232008236` — and repairing it means `wcore-contract generate`, which is release-coordination). **Still unrecoverable, by design:** a genuinely corrupt journal; an explicit-null receipt on an event whose `effect_contract.reconciler` is `None` (a third-party producer could write one; this engine could not — and it now fails with a structurally accurate error instead of a checksum error, which is not a regression); and content the journal never carried. **23B-H1's original reproduction gap is unchanged** — the prior lane's 34 harness runs produced 0 reproductions because the harness never reached a tool event; this work makes that moot for data loss rather than narrowing it |
| **Why it reopened TXN-\*** | it is a durability defect in the journal, measured against the product, after `EFFECTIVE` was awarded on a proof that did not cover it |

