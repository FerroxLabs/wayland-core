# 30-02 — Pre-registered comparative trial protocol (F30-03)

**Frozen 2026-07-28, before any measurement of any kind existed.**

| | |
|---|---|
| Machine-readable form | `evidence/30-02/protocol.json` |
| Content address | `evidence/30-02/protocol.sha256` → `d18407e0b96bf753f66adc1eab7d21cbaeca1b9e627cecf0159095938b83ef25` |
| Decided by | four-way cross-audit panel, 4/4 `CONFIRM_WITH_AMENDMENT` |
| Panel artifacts | `30-02-decision-evidence/panel-{codex,gemini,kimi,internal}.txt` |
| Split, rationale, dissent | `30-02-decision-evidence/decision-dissent.txt` |
| Lane | `lane/30-02` |

The commit that introduces this document and `protocol.json` contains **no measurement of any
kind**. A later gate proves from git history that it is a distinct, earlier commit than the one
introducing `results.json`. That ordering is what makes this a pre-registration rather than a
document, and amending it after a number exists is the single forbidden act in this plan.

---

## 0. The scope limitation, stated before the methodology because it bounds everything below

Every measurement taken under this protocol carries the scope tag `SCRIPTED_HARNESS`.

All three tools are driven against one loopback OpenAI-compatible fixture serving **byte-identical
scripted assistant turns**. That is precisely what makes the comparison fair and credential-free —
and precisely what makes it silent about model quality. **No claim of the form "Wayland is more
correct than Hermes in the wild" is supported by anything produced under this protocol.** Real
dollar cost is not measured; the cost dimension counts *synthetic token units of metered demand*.

The panel was unanimous on this and codex's framing is adopted verbatim: call the result a
**scripted agent-harness benchmark, never an agent-quality benchmark**.

## 1. What is measured, per dimension

| Dimension | Observable | Kind | Extracted from | n |
|---|---|---|---|---|
| correctness | oracle completion rate | proportion | the workspace on disk + the fixture's violation list | 30 |
| recovery | injected-fault recovery rate | proportion | the fixture's `injected_faults` proving the fault was met, then the on-disk oracle | 30 |
| security | canary non-exfiltration rate | proportion | byte search for a unique per-trial canary across every fixture request body | 30 |
| cost | synthetic token units per attempted trial | continuous | the fixture's own usage frames | 15 |
| **cognitive tax** | **none — not measurable in this tier** | — | — | **0** |

**Never from the tool's own self-report.** Not its stdout, not its transcript, not its exit
status, not its session-cost event. Three programs' self-reported token counts are not
comparable; the number has to come from the wire between the tool and the model.

### Cognitive tax is unmeasured, and that is the finding

All three external panel members refused to proxy it, independently. Gemini: proxying via
wall-clock "measures performance, not cognitive load." Codex: "configuration-line counts, command
counts and elapsed time are not cognitive tax — encode `NOT_MEASURED`, not zero." Kimi: an
intervention count "measures prompt frequency, not cognitive load, and the three tools have
different interaction models so the counts are not on a common scale."

Accepted in full. **All three cognitive-tax legs are UNPROVEN by construction of this protocol,
decided before any trial ran.** F30-03 is therefore incomplete on one of its five dimensions, and
this plan says so rather than inventing a number. The substitution point is named: a live-tier
protocol with a scripted human operator and a pre-registered intervention taxonomy.

## 2. Bounds, and the four verdict states

- Proportions: **Wilson score, 95%**. Clopper–Pearson rejected by both members who addressed it —
  conservative, wider at n=30, and it does not repair an underpowered design.
- Difference of proportions: **Newcombe's Wilson-based interval**. Not the subtraction of two
  Wilson endpoints, and not Wald. Codex raised this; the others did not.
- Continuous: **percentile bootstrap, 95%, 10 000 resamples**, seeded from the protocol so a rerun
  reproduces the bounds exactly, with the raw min/max reported alongside. Identical observations
  emit `ZERO_EMPIRICAL_VARIANCE` rather than presenting a zero-width interval as precision.

**Four verdict states, not three.** The plan specified three (ahead / behind / indistinguishable).
Kimi's objection was decisive and codex reached the same shape independently:

> "'CI contains zero → tie' silently converts low power into declared equivalence."

So:

```
WAYLAND_AHEAD                  iff delta_ci.lower >  +tie_band
PEER_AHEAD                     iff delta_ci.upper <  -tie_band
PRACTICALLY_INDISTINGUISHABLE  iff delta_ci.lower >= -tie_band AND delta_ci.upper <= +tie_band
INCONCLUSIVE                   otherwise
```

Both added states are **non-directional**, so the plan's hard rule is preserved exactly and in
fact strengthened: a directional verdict now additionally requires the nearer endpoint to clear
the tie band. **A directional verdict is refused whenever the delta interval contains zero. No
exception.** This deviation from the plan's letter is in the direction of refusing *more* claims,
and is recorded in `decision-dissent.txt` §A3 and in the summary.

Tie bands: 0.05 absolute for proportions; 5% on the mean-ratio scale for cost — the tighter of the
two proposals, because a wider band declares more ties and a tie is the verdict that costs nothing
to claim. A `lower cost` direction may be reported only when the cheaper tool is not worse on
correctness beyond the correctness tie band, so fail-fast is not rewarded as efficiency.

## 3. Trial mechanics

Fresh fixture instance, fresh workspace, fresh process tree and a unique canary **per trial**. The
starting position proposed one shared fixture; two members objected that a single FIFO cursor
couples trials. The fixture binds port 0, so this costs nothing. Build and install time is outside
the scored trial.

Stop rule `STOP_RULE_V1`: 120 s to first fixture request, 120 s inactivity (no request, no
stdout/stderr byte, no workspace mutation), 600 s absolute; SIGTERM to the process group, 5 s,
SIGKILL. **A timeout is a scored failure and is never discarded** — "excluding hangs flattens
flaky tools into looking reliable; a hang is a failure the user experiences." Three consecutive
timeouts halt that leg as `FAILED_INCOMPLETE` with the trials actually run recorded.

**Conformance gate.** Each tool must first pass an *unscored* run proving it can reach the meter at
all. A tool that fails conformance has its legs recorded **UNPROVEN with the captured failure** —
it is *not* scored as failing the dimensions. Protocol incompatibility must be distinguishable
from task failure.

## 4. The pins, and a correction to this plan's own premise

| Peer | Version | Commit | Base-URL override at the pin |
|---|---|---|---|
| Hermes Agent | 0.17.0 | `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21` | `OPENAI_BASE_URL` |
| OpenClaw | 2026.6.2 | `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc` | `OPENAI_BASE_URL` |

`HEAD-2026-07-26` is explicitly **not** the baseline and is never substituted for it.

**The 30-02 plan is wrong about Hermes, and the correction is recorded here rather than quietly
applied.** The plan states Hermes uses `HERMES_BASE_URL`. At the pinned commit that variable is a
`tui_gateway/server.py:13017` setting and an env-allowlist **test** fixture
(`tests/tools/test_execute_code_approval_cluster.py:291`, where it is asserted to be *dropped*) —
it is not the LLM base URL. The actual override at the pin is `OPENAI_BASE_URL`, at
`agent/auxiliary_client.py:1946` and `hermes_cli/auth.py:191`
(`base_url_env_var="OPENAI_BASE_URL"`). This correction makes the trial *more* feasible, not less:
both peers take the same variable. It is re-confirmed under task 3 and captured there.

## 5. No credential, ever

Nothing here reads a real API key, a real account or a real registry token — Wayland's or a
peer's. The only environment value resembling one is the synthetic literal
`wayland-frontier-trial-not-a-secret`, which authenticates nothing and never leaves loopback.
Peer provisioning runs each peer's **own** lockfile at its **own** pinned commit, in a disposable
directory on the build host, with no credential present. A peer that cannot be installed without a
credential or a private registry becomes UNPROVEN — it is not worked around.

The live-provider tier is fully specified in `protocol.json` and declared `DECLARED_UNPROVEN`, so
the day credentials exist there is a runnable protocol and not a design exercise.

## 6. The bias this protocol does NOT close

**All four panel members independently named the same defect.** Gemini: "if the fixture strictly
expects [A→B] and OpenClaw naturally requests B then A, OpenClaw gets a 409 and fails." Codex:
"protocol and task affinity masquerading as product superiority." Kimi: "the fixture already
serves Wayland's contract tests, so its SSE dialect, usage-frame placement, error-body shape and
tool-call encoding are literally the dialect Wayland was built against… the bias needs no intent
to operate."

Their countermeasures — a content-routing fixture (gemini), a peer-derived corpus with external
sign-off (kimi), an independently-controlled task corpus (codex) — all require editing the shared
meter or authoring a peer corpus. `crates/wcore-eval-scenarios/src/fixtures/openai.rs` is a hard
scope fence in this plan, gate-checked untouched, because editing the meter mid-phase changes what
every earlier measurement meant.

So the bias is **real, named by every member, and open.** What this protocol does instead is bound
it so it cannot be banked:

> **A fixture `unexpected_request` violation is classified `HARNESS_INCOMPATIBLE` — neither a
> success nor a task failure.** It is reported as an observation about the *meter*, with the
> tool's request count.

Without that rule, a difference in request *order* is silently converted into a Wayland win.
With it, the difference is visible and unscored. It is a bound, not a fix. **Carried forward as a
seam request; any 30-03 claim resting on a cross-tool comparison inherits it.**

## 7. Frozen — the list of acts this protocol forbids after this commit

- changing any metric, observable, extraction, trial count, interval method, seed, tie band or
  stop rule;
- adding, removing or reinterpreting a dimension;
- re-running a leg and keeping the more favourable of two runs;
- reclassifying a scored failure as an infrastructure failure after seeing the number;
- reporting a directional verdict whose delta interval contains zero.
