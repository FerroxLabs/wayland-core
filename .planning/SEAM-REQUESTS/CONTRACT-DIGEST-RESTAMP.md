# SEAM REQUEST — re-stamp the Desktop contract digests (bookkeeping only)

**Status:** OPEN, fenced. Owed as a coordinated Core+Desktop step. **Deliberately not performed.**
**Raised:** 2026-07-27, after three separate lanes independently reported CI red on macOS and
self-hosted Windows and attributed it to a platform problem.

---

## 1. What drifted, measured rather than assumed

`wcore-contract check` fails on the integration branch. Five files report drift:
`manifest.json`, `events/ready.json`, and three `adversarial/events/*.jsonl` that embed the
digests. Regenerating in a **scratch tree** and diffing field-by-field shows exactly what moved:

| Field | Old | New |
|---|---|---|
| `fixture_digest` | `sha256:0704cd43a86e52da…` | `sha256:5ff73855fcc56afe…` |
| `source_inputs_digest` | `sha256:9d5928b47f0cf943…` | `sha256:7384146c72a8a873…` |
| **`schema_digest`** | **unchanged** | **unchanged** |

**No event added or removed. No command added or removed. No capability changed. No JSON schema
changed. Still minor 8.** The cause is ordinary source edits to files that `source_inputs_digest`
covers, cascading into `fixture_digest` because the fixtures embed it.

So the wire *shape* is identical. This is a digest re-stamp, not a contract alteration — which
is a materially different thing from what the standing "do not run `wcore-contract generate`"
rule was written to prevent, and why it was worth deciding rather than assuming.

The scratch regeneration was **discarded**; nothing was committed.

## 2. The decision, and the argument that carried it

Put to the 4-way panel. **Unanimous 3-0 against regenerating** (Codex 5.6 Sol, Gemini 3.1 Pro,
Kimi K3), against the orchestrator's own initial leaning.

The decisive argument was not about this change, which is harmless. It was about the precedent:
**an agent who regenerates whenever CI goes red destroys the only guard that would catch a
genuine wire change.** The next drift gets the same reflex, and that one might be real. That is
this program's most frequently recurring failure mode — a check quietly retuned until it agrees
with the code — and it has already been found here in a self-passing gate, a stale canary keyed
to one spelling, and a lint rule written against its own motivating example.

The orchestrator's counter-argument was that CI red on two of three platforms blocks native
certification. On inspection that was **weaker than it looked and is recorded as refuted**: the
`build` job that produces per-target binaries is a *separate job*, and artifacts have already
been downloaded successfully from a run whose overall conclusion was `failure`. Lanes also test
directly on real hardware rather than through CI. The genuine loss was test **visibility** on
macOS and Windows, not capability.

## 3. What was done instead

The contract check **moved to after the test step** in `.github/workflows/ci.yml`. Ordering only
— the command, its strictness and its exit status are untouched, and the job still fails on
drift. It now reports test results first, so a digest drift stops hiding two platforms' worth of
test signal. The guard is preserved exactly; only its position changed.

## 4. What is owed, and by whom

A coordinated Core+Desktop re-pin, at release time, not on a branch:

1. Run `wcore-contract generate` in the Core release cut.
2. Publish the digest movement the way `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md` §3.0 did
   for revision 1→2 — both digests named, with the statement that `schema_digest` did not move.
3. Ship the Desktop re-pin **in the same release train**. `observation.rs:329` makes a digest
   mismatch a hard error at `ready` negotiation: the session is refused outright, it does not
   degrade and it does not drop the line. An un-re-pinned Desktop against a re-stamped Core
   refuses to connect.

Batch this with the three open items in `F21-04-01.md` so the whole set costs **one** coordinated
release rather than four.

## 5. Re-verifying this before acting on it

```bash
cargo run -p wcore-protocol --bin wcore-contract -- check     # expect exit 1, 5 files
# in a SCRATCH worktree only:
cargo run -p wcore-protocol --bin wcore-contract -- generate
git diff -- crates/wcore-protocol/contracts/                  # expect: digests only
```

**If `schema_digest` has also moved by the time anyone reads this, none of the above applies** —
that is a genuine wire change and needs a real contract bump with a Desktop-side review, not a
re-stamp.
