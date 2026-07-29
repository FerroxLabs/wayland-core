# 23B-H1-MEASURE — append-only notes

Lane `lane/23b-h1-measure`. Base `3cfc336fd2d82f57b5a24716262a71e759cb4a24`
(`plan/f20-unified-audit-repair`). Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23b-h1-measure`.

**No credential value appears in this file or any artifact this lane writes.**

---

## T+0 — mandate and inherited state

Mandate: a `flux-router` credential now exists (`~/.wayland-secrets/flux.env`, mode 600,
outside every repo). Use it to repair the durability harness's *reach* — get it dispatching
real tool events — then reproduce or disprove `23B-H1` at HEAD.

Inherited from `23B-H1-SUMMARY.md` (lane `lane/23b-h1`), read in full before acting:

- Finding OPEN at HIGH. A cleanly-exited run writes a journal the product cannot read back
  (`journal checksum mismatch at sequence N`), and every operator verb that reads it fails
  identically.
- **The previously-claimed root cause is WRONG and I do not need to re-derive it.** The
  repaired mechanism (`Option<Value>` + `skip_serializing_if = Option::is_none` holding
  `Some(Value::Null)`) is engine-unreachable: `durable_receipt()` serialises a derived
  named-field struct (`FilesystemEffectReceiptV1`) → always `Value::Object`; the other three
  `prepare_tool_effect` call sites pass literal `None`. 23B-01's repro was headless, no
  third-party producer, so that shape cannot have been written.
- Three candidates remain, one already excluded: (1) a write that never completes its final
  record — **excluded**, `ChecksumMismatch` is check *3* of 3 in `verify_chain_from`, so the
  frame SHA and the chain link both passed; (2) a schema the reader rejects; (3) a reader
  stricter than the writer. §2 of the prior summary lands on category (3), i.e. an encoding
  that is not a round-trip fixed point — but the specific member is unknown.
- Prior lane's §4b census is the strongest lead: **~34 fields** in `session_journal` carry
  `#[serde(default, skip_serializing_if = P)]` where the skipped value has an explicit JSON
  spelling that decodes back to itself. Only **2** (`is_absent_json_value`) were repaired.
  `Option::is_none` × 21 (`null`), `BTreeMap::is_empty` × 5 (`{}`), `Vec::is_empty` × 4
  (`[]`), `BTreeSet::is_empty` × 1, `is_zero_u32` × 1 (`0`).
- Harness reach: `scripts/f23-h1-repro.sh` writes a config with a fake key and
  `base_url = http://127.0.0.1:1` (closed port). Every run therefore ends
  `status=OK_DISPATCH_FAILED`; **no tool event is ever recorded**, 0/12 and 0/34 before that.
  Non-reproduction from an instrument that cannot reach the defect is the evidentiary form of
  a gate that cannot fail.

## T+0 — what I must establish

1. Harness reaches a real tool event (counted, not asserted).
2. Reproduce or disprove at HEAD, with counts, neither forced.
3. If it reproduces: name WHICH of the remaining candidates, with evidence, before fixing.
4. Only then fix, plus a repair path for an already-unreadable journal (reclaim + quarantine,
   the shape this program used in the sandbox), not a permanent refusal.

## T+0 — measured caveat carried in from the mandate

`flux-fast` is a **reasoning** model. A 16-token budget returned HTTP 200 with empty content
and all 16 tokens spent as `reasoning_tokens`. Budget completions properly or an empty
completion will be misread as a product defect.

## T+0 — open question being decided now

Where to run. Hetzner is the only host that builds. The mandate says the key must never be
written into a file; the lane brief says never copy a credential off the Mac. Checking first
whether a HEAD-provenance `wayland-core` binary already exists on the Mac, which would satisfy
both. Decision and its evidence appended below.

---

## T+35 — decisions taken, with evidence

**Where to run: hetzner.** The Mac's installed binary is `wayland-core 0.12.12`
(`/opt/homebrew/bin/wayland-core --version`) — six minors stale, so it cannot measure HEAD, and
the lane brief forbids `cargo` on the Mac. Built on hetzner instead:
worktree `/root/wayland-23b-h1-measure` @ `231b2469`, `cargo build -p wcore-cli --release` →
`BUILDRC=0`. The commit had to be pushed Mac→hetzner over ssh (`git push
hetzner-dsm:/root/wayland lane/23b-h1-measure:hz/23b-h1-measure`): hetzner's `origin` is an
https GitHub remote with no credential helper, so `git fetch` there fails auth.

**Credential path: env only, never a file.** `wcore-config::resolve_api_key_from_env`
(`config.rs:2849`, `ProviderType::FluxRouter`) reads `$FLUX_API_KEY`, so the harness needs no
`api_key` in `config.toml`. The key is piped Mac→hetzner over ssh **stdin**, read with one
`IFS= read -r`, exported into the child env only. Never in argv, never on hetzner's disk. Every
transcript the harness preserves is passed through a fixed-string `awk` redactor first.

**Provenance caveat, stated rather than papered over.** This build's `--version` prints
`wayland-core 0.12.25` with **no source sha** — unlike the string the previous lane quoted. So
`--version` alone cannot pin a commit. The harness prints the binary's sha256 prefix and the
caller records the checkout HEAD; the version guard is coarse and is labelled as coarse.

## T+35 — flux-router verified live, before spending anything on runs

- `GET /v1/models` → `MODEL_COUNT=77`.
- **Tool calling works and is the reach we need.** `flux-fast`, one call with a `Write` function
  schema: `finish=tool_calls tool_calls=1`, argument
  `{"file_path": "/tmp/aardvark.txt", "content": "aardvark"}`, `cost_usd=0.00072`,
  102 completion tokens. The mandate's caveat is real — a small budget yields empty content, so
  the harness passes `--max-tokens 4000`.

## T+35 — the harness's reach defect, and how the new one differs

`scripts/f23-h1-repro.sh` writes `base_url = "http://127.0.0.1:1"` with a placeholder key
(lines 58-69), so **by construction** no run can reach a tool event. Its counters have no bucket
for that: a run that never dispatched increments `resume_ok` via the `OK_DISPATCH_FAILED` arm.
0/12 and 0/34 were therefore not measurements of the defect.

`scripts/f23-h1-repro-live.sh` (new) fixes the reach AND the counter:
- real provider, `--dangerously-skip-permissions` so the Write tool executes unattended;
- **`F23_H1_REACH=` per run counts `tool_intent_recorded` occurrences in the journal and whether
  the target file exists on disk** — the positive path proved with counts, not assumed;
- a run with zero tool events lands in its own `no_tool_event` bucket and is **not** counted as
  a non-reproduction.

## T+35 — source reading done while the build ran (narrows the candidates)

`computed_checksum` (`session_journal.rs:107`) hashes `ChecksumMaterial{schema_version,
session_id, seq, previous_checksum, event}` — i.e. it **re-serialises the deserialised event**
and compares to the stored digest. So `ChecksumMismatch` ⟺ `serialize(deserialize(bytes))
!= bytes` for the event. Two structural hazards checked and **excluded**:

- **`HashMap`/`HashSet` iteration-order nondeterminism** — the classic cause of exactly this
  signature (passes the frame digest and the chain link, fails only check 3, and is
  content-and-load-sensitive). `grep -cE 'HashMap|HashSet' session_journal/model.rs` → **0**.
  Designed out. `preserve_order` is not enabled either, so `serde_json::Value` maps are
  `BTreeMap` and re-serialise sorted.
- **A write-side previous_checksum/checksum race** — `append` (`session_journal.rs:1341`)
  builds the envelope with `JournalEnvelope::create`, which computes the checksum from the same
  `previous_checksum` it stores, under `&mut self` behind an exclusive lease. No fix-up window.

`LegacyEffectReceiptEncoding` (`model.rs:65`) is a thread-local encoding switch, but the guard
restores on drop, so it is not a leak hazard.

---

## T+70 — REACH ACHIEVED. This is the precondition nobody had.

`scripts/f23-h1-repro-live.sh`, hetzner, HEAD release binary `97a2602e1ef9a3c4`:

```
F23_H1_REACH=1 id=ee638d231dded0 tool_events=1 file_written=yes seed_exit=0 bytes=94409
```

A real provider call, a real `Write` tool dispatch, a real file on disk, a real
`tool_intent_recorded` in the journal. Every previous 23B-H1 measurement — 12 by the last
lane, 34 before it — was taken by a harness that could not get here.

### Batch 1 — HEAD, quiet host (`b1.log`)

```
F23_H1_LIVE runs=10 tool_runs=10 tool_events=30 no_tool_event=0 resume_ok=10
            checksum_mismatch=0 other_journal_failure=0 seed_failure=0
```
10/10 runs reached tool events, 30 events total, journals 94 KB – 242 KB.

### Batch 2 — HEAD, host under concurrent `cargo build` (`b2.log`)

```
uptime before: load average: 10.18 …
F23_H1_LIVE runs=12 tool_runs=12 tool_events=21 no_tool_event=0 resume_ok=12
            checksum_mismatch=0 other_journal_failure=0 seed_failure=0
```
Honest caveat: load reached only ~10-11, not the ~28 that 23B-01 correlated with. This is
**not** a load arm; it is a second quiet-host arm and is labelled as such.

### Batch 3 — the PRE-FIX binary, `dba9b9e5` = `a7beafe5^` (`pf1.log`)

Binary `8a6e9ee4f434a771`, built from the parent of the commit that claimed to fix 23B-H1.

```
F23_H1_LIVE runs=12 tool_runs=12 tool_events=18 no_tool_event=0 resume_ok=12
            checksum_mismatch=0 other_journal_failure=0 seed_failure=0
```

**This is the most important number so far. The pre-fix binary does not reproduce either,
with full reach.** So `a7beafe5` is not what changed the outcome — consistent with the
previous lane's reachability argument, and now shown empirically rather than only
structurally.

Running total with reach: **0 reproductions in 34 runs across two binaries, 69 tool events.**

## T+70 — a size correlation that reframes 23B-01

23B-01 recorded failing journals ≈ **203 KB** and passing ≈ **71 KB**, and inferred "the
failing runs get further through the turn". My runs split cleanly the same way:

| tool events in run | journal size |
|---|---|
| 1 | 94 – 100 KB |
| 4 – 7 | 224 – 242 KB |

The ratio is the same, and the thing that moves it is **the number of tool records**. So
23B-01's failing runs almost certainly DID contain tool events and its passing ones did
not — which means reach was a real confound all along, and also that my harness is now
producing journals of the shape that failed.

## T+70 — control still outstanding

Neither HEAD nor `a7beafe5^` reproduces. The remaining control is 23B-01's own base,
**`15971d1b`** (61 commits before `dba9b9e5`, 841 before HEAD). Building it now. Either:
- it reproduces → there is a bisectable window `15971d1b..dba9b9e5` containing the real
  fix, 23B-H1 is genuinely closed, and `a7beafe5` was not the thing that closed it; or
- it does not → my harness still differs from 23B-01's procedure in a way I have to name,
  and I will name it rather than bank the zeros.

---

## T+150 — the control arms, and the one that mattered

- `ob1` — **23B-01's own base `15971d1b`**, quiet: 12/12 clean, 15 tool events, 0 mismatches.
  Binary pinned independently of `--version`: `sha256[0:16]=dc147bdd9db507ed`, and
  `session --help` **exits 1** because the subcommand does not exist in that build — the same
  pristine-binary check 23B-01 used on itself.
- `ob2` — same binary, CPU load 63 → 66 (2.3× the original 28): 12/12 clean.
- `ob3` — same binary, `--jobs 6`, load 70 → 114: 12/12 clean.
- `ob4` — same binary, `--seed-max-turns 1` so the loop is cut off right after the tool
  executes (the interrupted-turn shape): 10/10 clean, journals 62-63 KB.
- `ob5` — same binary, `--jobs 4`, under **12 parallel `dd oflag=dsync` writers at 11 139 IOPS
  / 1.25 GB/s plus a real concurrent `cargo build -p wcore-cli --release`**: 12/12 clean.

`ob5` exists because my own adversarial pass found that "4× load" was CPU-only while 23B-01's
stressor was concurrent *compilation* — heavy I/O and fsync pressure. The journal write path is
`write_all` + `sync_all`, i.e. fsync-bound, so a CPU-only arm may not have touched the real
stressor at all. Three panelists had already voted MEDIUM on that framing. Closing the hole was
worth more than caveating it.

**Total: 92 measurement runs, 153 tool events, 0 reproductions, three binaries.**

## T+150 — second instrument defect in my own harness, found live and repaired

The aggregator summed with `grep -o 'F23_H1_REACH=[^\n]*'`. In a POSIX BRE `[^\n]` excludes the
characters `\` and `n`, NOT the newline — and `tool_events` contains an `n`, so every match
stopped before the field. The harness printed `tool_events=0` for a run whose own per-run lines
read `tool_events=1` and `tool_events=7`. Repaired with `.`; self-test extended to six
assertions, and `SELFTEST_6_OLD_AGG_BLIND` replays the broken pattern to show it returns 0
where truth is 8. Repaired in this lane, not written up and left.

Third one, in the panel harness: `codex exec` blocks on an inherited stdin pipe even with the
prompt passed as an argument — two 400 s timeouts, vote silently absent both times. Fixed with
`< /dev/null`. New member of the brief's §4 list.

## T+150 — no fix made, deliberately

Root cause not named → per the mandate's own sequencing, no fix. Verified instead that the
missing general **quarantine/reclaim path for an unreadable journal** is real: the only recovery
in the tree is `recover_legacy_effect_receipt` (`session_journal.rs:2185`), keyed literally to
`"effect_receipt":null`, and all twelve `session` verbs read the journal so a mismatch takes
every operator move down at once. Recommended as a standalone backlog item.
