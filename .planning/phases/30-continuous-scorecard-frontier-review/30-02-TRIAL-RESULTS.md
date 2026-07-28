# 30-02 — Frontier comparative trial results (F30-03)

## 0. Read this before any number below

**These trials hold the model constant by construction.** All tools were driven against one
loopback OpenAI-compatible fixture serving byte-identical scripted assistant turns. That is what
makes the comparison fair and credential-free, and it is exactly what makes it **silent about
model quality, about real-world task success, and about dollar cost.** Every measurement carries
the scope tag `SCRIPTED_HARNESS`. This is a scripted agent-harness benchmark, never an
agent-quality benchmark.

**An UNPROVEN leg is not a win for anybody.** Nine of the fifteen legs did not run. No comparative
result exists for any dimension in which a tool did not run — `ComparativeResultV1` cannot be
constructed without every compared tool's measurement, so "we could not run the competitor, so we
win" is not expressible in this harness at all.

**This document measures. It does not position.** What these numbers mean for Wayland is 30-03's
question, and 30-04 grades the phase.

| | |
|---|---|
| Protocol | `evidence/30-02/protocol.json`, sha256 `d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25` |
| Pre-registered at | commit `a7bd5d87`, which contains no measurement of any kind |
| Results | `evidence/30-02/results.json`, verified by the shipped `wayland-scorecard trials verify` |
| Legs | 15 accounted for exactly once: **6 RUN, 9 UNPROVEN** |
| Host | `hetzner-dsm`, Linux. `df -h /root` before provisioning: 750G available; after: 744G |

---

## 1. The leg accounting, before any number

| Leg | Tool | Dimension | Status | Why, if unproven |
|---|---|---|---|---|
| LEG-01 | wayland | correctness | **RUN** | |
| LEG-02 | wayland | recovery | **RUN** | |
| LEG-03 | wayland | security | UNPROVEN | meter records body digests, not bodies |
| LEG-04 | wayland | cost | **RUN** | |
| LEG-05 | wayland | cognitive_tax | UNPROVEN | not measurable in this tier (unanimous panel) |
| LEG-06 | hermes | correctness | **RUN** | |
| LEG-07 | hermes | recovery | **RUN** | |
| LEG-08 | hermes | security | UNPROVEN | same meter limitation |
| LEG-09 | hermes | cost | **RUN** | |
| LEG-10 | hermes | cognitive_tax | UNPROVEN | not measurable in this tier |
| LEG-11 | openclaw | correctness | UNPROVEN | peer never delivered to the build host |
| LEG-12 | openclaw | recovery | UNPROVEN | peer never delivered |
| LEG-13 | openclaw | security | UNPROVEN | peer never delivered + meter limitation |
| LEG-14 | openclaw | cost | UNPROVEN | peer never delivered |
| LEG-15 | openclaw | cognitive_tax | UNPROVEN | not measurable + peer never delivered |

Index: `evidence/30-02/legs.tsv`. Every leg names a capture that exists and holds real content —
per-trial JSON Lines for a RUN leg, a named blocker with its substitution point for an UNPROVEN one.

---

## 2. The measurements, with bounds

Every figure comes from the fixture or from the workspace on disk. **Never from a tool's own
stdout, transcript, exit status or self-report.**

| Tool | Dimension | n | Estimate | 95% interval | Method |
|---|---|---|---|---|---|
| wayland | correctness | 30 | 0.0000 | [0.0000, 0.1135] | Wilson score |
| wayland | recovery | 30 | 0.0000 | [0.0000, 0.1135] | Wilson score |
| wayland | cost | 15 | 20.00 units | [20.00, 20.00] | ZERO_EMPIRICAL_VARIANCE |
| hermes | correctness | 30 | 1.0000 | [0.8865, 1.0000] | Wilson score |
| hermes | recovery | 30 | 1.0000 | [0.8865, 1.0000] | Wilson score |
| hermes | cost | 15 | 20.00 units | [20.00, 20.00] | ZERO_EMPIRICAL_VARIANCE |

Note the Wilson bound on Hermes' perfect score: **30/30 gives a lower bound of 0.8865, not 1.0.**
A clean sweep of 30 trials does not license a claim of better-than-95% reliability, and the
interval says so without anyone having to remember it.

Both cost legs were perfectly deterministic — 2 fixture requests and 20 synthetic token units on
every single trial for both tools. That is reported as `ZERO_EMPIRICAL_VARIANCE` rather than as a
zero-width interval dressed up as precision.

### The comparatives

| Dimension | Delta interval (Wayland − Hermes) | Tie band | Verdict |
|---|---|---|---|
| correctness | [−1.0000, −0.8395] | 0.05 | **PEER_AHEAD** |
| recovery | [−1.0000, −0.8395] | 0.05 | **PEER_AHEAD** |
| cost | [0.0000, 0.0000] | 0.05 | PRACTICALLY_INDISTINGUISHABLE |

**Two of the three comparable dimensions go against Wayland**, by an interval that clears the tie
band with room to spare. That is what the frozen rules entail and it is published unaltered.
No comparative exists against OpenClaw on any dimension.

---

## 3. What actually caused the correctness and recovery result — a defect in MY instrument

Stated plainly, because a reader who takes §2 at face value will draw a conclusion the evidence
does not support.

The frozen protocol specifies **one canonical fixture script** whose tool call is named
`write_file`. Measured on the build host:

- **Hermes accepts a tool call named `write_file`** and wrote `TRIAL-ARTIFACT.txt` with the exact
  oracle content, 30/30 on correctness and 30/30 on recovery after an injected HTTP 503.
- **Wayland Core's equivalent tool is named `Write`.** It reached the meter, consumed both script
  steps and exited 0 on every trial — but produced no artifact, because the scripted tool call
  named a tool it does not expose.

So the dominant cause of the 0/30 is that **the frozen script happens to speak Hermes' tool
dialect and not Wayland's.** Panel member codex predicted exactly this and prescribed the fix —
"if tool schemas differ, compile one canonical semantic script into tool-native response dialects
and hash all translations; do not falsely claim byte identity" — and **my protocol failed to adopt
it.** That is a defect in the instrument, found by running it.

Three consequences, all of which I am bound by:

1. **The numbers stand as measured.** Amending the protocol after a measurement exists is the
   single forbidden act in this plan. The legs are not re-scripted and not re-run.
2. **The result is not suppressed.** It is unfavourable to the product whose vendor ran the
   benchmark, and suppressing an unfavourable result is precisely the forgery this plan exists to
   prevent. The asymmetry of hiding it would be more damning than the number.
3. **It must not be read as a capability verdict.** Within scope, the honest statement is narrow:
   *given an OpenAI-compatible provider emitting a tool call named `write_file`, Hermes 0.17.0
   completed the task on 30 of 30 trials and Wayland Core 0.12.25 completed it on 0 of 30.*
   Whether that is a Wayland defect in tool-name interoperability or an artifact of my script is
   **not settled by this evidence**, and 30-03 must not position from it as though it were.

A protocol v2 with per-tool dialect compilation and committed translation digests is the
substitution point. It is a new pre-registration, not an amendment to this one.

## 4. A second finding, about our own tool, from the conformance gate

`wayland-core` **refused to start at all** on the headless build host:

```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked.
```

It reached a provider only after being given a `[credentials] backend = "encrypted-file"` config
and a vault passphrase. **Neither peer needed an equivalent step**; Hermes needed a
`~/.hermes/config.yaml` naming the custom provider, which is ordinary provider configuration
rather than a startup authority. This is recorded as `workspace_seed_files` **data** in the
invocation rather than hidden in the driver, precisely so that the extra setup our own tool
required stays visible to a reader.

This is an operator-completeness observation, not a scored dimension. It is exactly the class of
defect that only a live run finds.

## 5. Why the security legs are UNPROVEN — and why I did not narrow the extraction

The protocol's security extraction is a **byte search for the per-trial canary across every
request body the fixture recorded**. The meter records `body_sha256`, `semantic_body_sha256` and
per-leaf semantic hashes — **it does not retain bodies.** So the frozen extraction cannot be
performed.

An exact-leaf comparison would detect a canary sent as a whole message value but not one embedded
in surrounding prose. That is a **strictly narrower** extraction than the frozen one, and running
it while calling the result protocol-conformant would be the forbidden act by another route. The
legs are UNPROVEN. `src/fixtures/openai.rs` is a hard scope fence in this plan and was
gate-checked untouched.

## 6. Why OpenClaw is entirely UNPROVEN, and what was NOT done instead

The pinned commit `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc` was bundled **read-only** from Sean's
reference checkout — 392,186,966 bytes. The transfer to the build host **dropped at 164,766,720
bytes (42%)** with `Read from remote host: Operation timed out / lost connection`, and
`git clone` correctly refused the truncated bundle:

```
error: index-pack died
fatal: remote transport reported error
```

A resumable `rsync --partial --append` retry was started, resumed from 164,766,720 bytes, and had
not completed within this plan's window.

**What was NOT done, and this is the load-bearing part:** the `HEAD-2026-07-26` snapshot was not
substituted, no npm-registry copy was fetched, no other version was measured, and no leg was
scored from a peer that did not run. OpenClaw contributes **zero** comparatives. Its absence
makes Wayland's position *less* evidenced, not better.

Substitution point: complete the transfer (it resumes from byte 164,766,720), clone at the pin,
install from the repository's own committed lockfile, re-run the legs unchanged.

## 7. Cognitive tax: unmeasured on purpose, decided before any trial ran

All three external panel members independently refused to proxy it. Gemini: proxying via
wall-clock "measures performance, not cognitive load." Codex: "configuration-line counts, command
counts and elapsed time are not cognitive tax — encode `NOT_MEASURED`, not zero." Kimi: an
intervention count "measures prompt frequency, not cognitive load, and the three tools have
different interaction models so the counts are not on a common scale."

**F30-03 is therefore incomplete on one of its five dimensions**, and this plan reports that
rather than inventing a number.

## 8. The bias this whole exercise does not close

All four panel members named it independently: the fixture is FIFO-cursored and is the dialect
Wayland's own contract tests were built against. The protocol bounds it — a 409
`unexpected_request` is scored `HARNESS_INCOMPATIBLE`, neither success nor failure — and, as it
happens, **no trial in this run triggered that state at all**. The bound was never exercised.
The bias remains open; closing it needs a content-routing meter and a peer-derived corpus, both
outside this plan's fence.

**Seam request for the orchestrator:** `crates/wcore-eval-scenarios/src/fixtures/openai.rs` needs
(a) request-body retention under a redaction policy, or leaf-hash exposure, so a canary
non-exfiltration measurement becomes possible; and (b) content-routed rather than FIFO-cursored
script matching, so a peer whose request order differs is not penalised. Both change the shared
meter and belong to a release-coordinated change, not to this lane.

## 9. Credential verification

No real credential was used, read, printed or committed anywhere. Every trial child process was
spawned with a **cleared environment** (`env_clear()`) plus an explicit non-secret allowlist:
`PATH`, `HOME` (pointed at the trial's own throwaway workspace), `LANG`, the tool's base-URL
variable pointed at loopback, and the synthetic literal
`wayland-frontier-trial-not-a-secret`, which authenticates nothing. Peer provisioning ran with
`env -u OPENAI_API_KEY -u ANTHROPIC_API_KEY -u GITHUB_TOKEN`. Every model response came from the
loopback fixture; no provider was contacted.

## 10. Reproducing this

```bash
wayland-scorecard trials run      --protocol protocol.json --invocation inv-<tool>.json \
                                  --dimension <dim> --trials <n> --workspace-root WS --out records/<tool>-<dim>.jsonl
wayland-scorecard trials assemble --protocol protocol.json --records-dir records/ \
                                  --blockers blockers.json --out results.json
wayland-scorecard trials verify   --protocol protocol.json --results results.json
```

Both bootstrap seeds are recorded in the protocol and the resampler is deterministic, so a rerun
reproduces every bound exactly.
