# 23B-H1-journal — lane NOTES (append-only, committed continuously per LANE-BRIEF §6b-i)

Lane branch `lane/23b-h1-journal`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23b-h1-journal`,
branched from `gh/plan/f20-unified-audit-repair` @ `f8b8ec25372fb4ed4280a5aa365873ae8465abfc`
(asserted equal to `git ls-remote gh plan/f20-unified-audit-repair`).

---

## 0. FIRST FINDING — my dispatch brief is STALE. Recorded before anything else.

My brief describes `23B-H1` as **HIGH, open, no fix, no repair path**, and quotes 23B-01's
original numbers (8/8, 9/10, 0/3; ~203 KB vs ~71 KB; pre-existing at `15971d1b`) verbatim.

**At integration head `f8b8ec25` that description is false in three separate ways.** Measured,
not assumed:

1. **A write-path fix is already in the tree.**
   `crates/wcore-agent/src/session_journal/model.rs:40` defines `is_absent_json_value`, used at
   `model.rs:697` and `model.rs:1083`. Control for a live instrument: `effect_receipt` returns 6
   hits in the same file.
2. **A read-side repair path is already in the tree.**
   `session_journal.rs:2185 recover_legacy_effect_receipt`, plus the `legacy_effect_receipt`
   envelope flag (`session_journal.rs:72`) and a scoped encoding
   (`model::LegacyEffectReceiptEncoding`) consulted by `computed_checksum` at
   `session_journal.rs:108`. So the brief's "there is no repair path" is out of date — a
   *narrow* one exists.
3. **The finding has already been re-graded.** `.planning/BACKLOG.md:1496`
   `BL-23B-H1 … (MEDIUM, non-reproducing)` supersedes the original HIGH row my brief points at
   (`.planning/BACKLOG.md:1771`). Both rows are live in the same file; the brief cites the older.

Per LANE-BRIEF §2a the correct response to a stale premise is to assert against an
independently-obtained truth and abort the premise, not the work. I am not treating the brief's
"open, unfixed" as a fact.

## 1. What the record actually says (three prior lanes, not one)

| Lane artifact | Claim |
|---|---|
| `23B-01-LIVE-EVIDENCE.md` §3 | ORIGINAL SIGHTING. 8/8, 9/10 under load; 0/3 quiet. Pre-existing at pristine `15971d1b`. |
| `23B-H1-DISPOSITION.md` | ROOT-CAUSED to `Some(Value::Null)` + `skip_serializing_if="Option::is_none"`; fixed write-side; proved red→green **at the journal layer, deterministically**. Explicitly states it could NOT reproduce the field sighting (34 runs, 0 repros, load to 130). |
| `BACKLOG.md:1496` (later lane) | **92 runs, 153 tool events, 0 mismatches** — at HEAD, at the PRE-FIX binary, and at 23B-01's own base commit. Found the inherited harness pointed at `127.0.0.1:1` so **no run ever dispatched a tool event**, and it folded non-reaching runs into `resume_ok`. Downgraded MEDIUM. |

**The load-bearing consequence, which the brief omits entirely:** the pre-fix binary at the
original base commit *also* does not reproduce on a reach-proven harness. Therefore the
`effect_receipt` null fix **is not what changed the outcome**, and the root cause of the
*original 23B-01 sighting* is still unidentified. Mechanism A (null receipt) is real and
deterministically demonstrable, but it has not been shown to be the mechanism that fired in the
field.

So there are TWO questions, and the record conflates them:
- **(A)** the null-receipt encoding asymmetry — real, fixed, repaired, proven at the unit layer;
- **(B)** what actually made a clean run at `15971d1b` write an unreadable journal at seq 16 —
  **still open**.

## 2. Structural frame I am working from (read from source, not inherited)

`computed_checksum` (`session_journal.rs:107-121`) hashes a **re-serialization** of the decoded
`SessionEvent` (`ChecksumMaterial`), NOT the bytes on disk. The on-disk bytes already have their
own SHA-256 as the frame digest (`encode_frame`, `session_journal.rs:2032`; verified at
`parse_complete_frames`, `session_journal.rs:2116`).

Therefore `ChecksumMismatch` is reachable **only** when
`serialize(deserialize(bytes)) != bytes` — because frame-digest (check 1) and
previous-checksum (check 2) both passed first. The integrity check depends on serde encode/decode
being a bijection over `SessionEvent`. That is a *class* of defect, of which the null receipt is
one instance. Chasing instances one at a time is what produced the current state of the record.

### Candidate mechanisms for the class, and their status

| # | Mechanism | Status |
|---|---|---|
| A | `Some(Value::Null)` + `skip_serializing_if="Option::is_none"` | CONFIRMED, fixed + repaired |
| B | Unordered collection (`HashMap`/`HashSet`) re-serialized in a different per-process order | **RULED OUT in `model.rs`** — every map/set in the journal model is `BTreeMap`/`BTreeSet` (17 sites, `model.rs:414-1756`). Control: `Option<` = 53 hits, so the grep discriminates. NOT yet ruled out for types embedded from other crates. |
| C | Other `Option<T>` fields with the same `skip_serializing_if` shape | to measure |
| D | float / integer re-formatting (`f64` round-trip, `-0.0`, exponent form) | to measure |
| E | `serde_json::Value` object key order (BTreeMap vs `preserve_order` IndexMap) | to measure — depends on whether any dep enables the `preserve_order` feature |
| F | `#[serde(flatten)]` / `untagged` re-ordering | to measure |
| G | non-canonical string escaping (`é` vs raw UTF-8) surviving decode | to measure |

Mechanism B is the one that would best fit the *field* signature — process-random, so
intermittent; nothing to do with load except that load correlates with reaching a larger event —
and it is exactly what "sequence 16 specifically" and "203 KB vs 71 KB" would look like. It is
dead in `model.rs`. Whether it is dead in the embedded foreign types is the next measurement.

## 3. Working plan

1. Determine, mechanically rather than by reading, whether ANY value of `SessionEvent` can fail
   the round-trip. A hand-enumerated shape list is what the previous two lanes each produced, and
   each one missed the next shape.
2. Decide (B) on evidence, not inheritance: re-measure the original sighting only if a mechanism
   predicts it.
3. Grade honestly. A downgrade with evidence is a valid result; so is an upgrade. The current
   MEDIUM rests on a *non-reproduction*, which LANE-BRIEF §3b-i names as the single easiest
   assertion to pass without doing any work.

## 4. Open / next

- [ ] Rule mechanism B in or out for foreign types embedded in `SessionEvent`.
- [ ] Build a round-trip invariant that is not shape-enumerated.
- [ ] Judgement on generalising the repair path beyond the literal null receipt.
- [ ] Final severity grade with evidence.
