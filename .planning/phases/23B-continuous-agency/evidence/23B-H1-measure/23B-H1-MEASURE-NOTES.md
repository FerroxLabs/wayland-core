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
