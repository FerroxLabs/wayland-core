# 25-01 — The one hibernating cloud reference backend

**Decision: `fly-machines` (Fly.io Machines API).**
**Basis: `majority` — 4 of 4 recorded panel positions.**
**Date: 2026-07-26. Decided autonomously; only the credential is reserved.**

---

## The question

Which single hibernating cloud backend is the F25 reference implementation?

PROJECT.md fences this: *"Cloud-provider proliferation — F25 proves one hibernating cloud reference
backend plus an extensible contract, not every vendor."* Exactly one vendor, chosen once; the
contract carries the extensibility.

## The four options

| id | name | outcome |
|---|---|---|
| `fly-machines` | Fly.io Machines | **COMMITTED** |
| `e2b-sandboxes` | E2B sandboxes | lost — see dissent |
| `ec2-hibernate` | AWS EC2 instance hibernation | lost — see dissent |
| `no-cloud` | Decline the cloud leg | lost — see dissent |

All four went to every panel member as ONE shared bundle,
`evidence/25-01-panel-prompt.txt`. Two deliberate departures from this plan's own framing, both
required by the plan: the `(Recommended)` prefix was stripped from `fly-machines`, and the four were
presented in rotated order (`e2b-sandboxes`, `ec2-hibernate`, `no-cloud`, `fly-machines`) so the
panel would not simply echo this plan's prior.

## The four verbatim responses

Captured byte-for-byte, each with a first line naming the shared bundle:

| member | position | capture |
|---|---|---|
| codex (`gpt-5.6-sol`) | `fly-machines` | `evidence/25-01-panel-codex.txt` (329 lines) |
| gemini (`gemini-3.1-pro-preview`) | `fly-machines` | `evidence/25-01-panel-gemini.txt` (136 lines) |
| kimi (K3) | `fly-machines` | `evidence/25-01-panel-kimi.txt` (39 lines) |
| internal adversarial | `fly-machines` | `evidence/25-01-panel-claude-adversarial.txt` |

The adversarial pass did **not** rubber-stamp. It sustained two attacks on the consensus's reasoning
(the panel decided on ergonomics a question that is tied at zero on the criterion that grades it;
and the panel underweighted that Fly's *ordinary* idle behaviour is stop-not-suspend), and it
declined to switch only because neither attack identifies an option that is better on the graded
criterion. Both attacks survive as binding conditions below.

## The three measurements

**Leg 1 — dependency satisfiability** (`evidence/25-01-livetest-deps.txt`). Re-taken against this
tree, not trusted from the plan. `aws-sigv4 = "=1.4.3"` is present, `aws-sdk-ec2` is absent from
`Cargo.lock`, 1015 packages baseline. **All three vendors are satisfiable with zero new crates** —
so dependency satisfiability does not discriminate, and the widely-repeated claim "EC2 needs a new
crate" is measured FALSE.

**Leg 2 — unauthenticated reachability and API shape from hetzner-dsm**
(`evidence/25-01-livetest-reach-linux.txt`). All three reachable, no SDK, no redirect. The single
measured discriminator turned out to be response ENCODING, which the plan's option text did not
anticipate:

| option | status | content-type | body |
|---|---|---|---|
| `fly-machines` | 401 | `application/json` | `{"error":"Authenticate: token validation error"}` |
| `e2b-sandboxes` | 401 | `application/json` | `{"code":401,"message":"authorization header is missing"}` |
| `ec2-hibernate` | 400 | `text/xml` | `<Error><Code>MissingParameter</Code>…AWSAccessKeyId…` |

A correction is recorded rather than hidden: the first fly probe, against the bare `/v1/apps`
collection with no `org_slug`, returned `404 text/plain`. That is a route mismatch, not an outage;
the documented routes then returned structured 401 JSON, including the per-app `machines`
collection the orphan scan will enumerate.

**Leg 3 — Windows parity for the committed option**
(`evidence/25-01-livetest-reach-windows.txt`). Run on `SeanDesktop` (PowerShell 5.1.26100.8875) via
base64-UTF-16LE `-EncodedCommand`. Identical result to Linux: **401, `application/json`, same body**.
The option's "behaves identically from macOS, Linux and Windows" claim is load-bearing for plan
25-04 and is now measured, not asserted. One environment finding recorded: Windows PowerShell 5.1
has no `-SkipHttpErrorCheck`, so the probe was re-taken through `curl.exe`.

## Why `fly-machines`

Unanimous on the panel, and the reasoning that survived scrutiny is narrow: it is the only option
that satisfies **every** pre-fixed integration constraint without a qualifier — bearer token over
plain HTTPS JSON straight into `wcore-egress` with no crate; `start` / `stop` / `suspend` /
`destroy` as literal REST verbs, which is exactly the lifecycle the contract must attest;
per-machine metadata labels, so 25-04's nonce orphan scan is one `GET /v1/apps/{app}/machines`;
JSON error bodies, where EC2's XML would need hand-rolled parsing or a forbidden new crate; and
measured cross-OS parity.

## Hibernation mechanism, and how the contract observes it

The reference transition is Fly's **`suspend`** (RAM-snapshot suspend/resume), NOT `stop`.
The contract observes the machine lifecycle as four attested transitions recorded in the receipt's
divergent section — `created → started → suspended → resumed`, plus `destroyed` on cleanup —
each read back from `GET /v1/apps/{app}/machines/{id}` rather than inferred from the request that
asked for it.

**Binding condition C1** (from the dissent, and from codex's conditional vote): an implementation
that can only observe `stop` MUST record that it did not observe hibernation and MUST NOT report
the hibernation property as attested. This is the single most likely place the decision silently
degrades, so it is written into the code as a distinct `HibernationObservation::NotObserved`
variant rather than left to reviewer vigilance.

## Cost

Fly Machines bill per second while running; a suspended machine bills only for its root volume. A
`shared-cpu-1x` / 256MB machine is fractions of a cent per equivalence run, and this phase's total
expected spend across 25-01 and 25-04 is **well under one dollar**. The throwaway org holds nothing
else, so the bill is also the audit trail.

## The throwaway org

One Fly organization (or one app inside it) created solely for this phase, holding ONLY this
phase's machines, so the orphan scan can assert emptiness against a single API call without
touching anything else Sean owns. The deploy token is scoped to that org only. A production token
must not be reused.

## What is reserved, and what is not

The **vendor choice was decided here**, on the panel, without waiting. The **funded account and the
minted token** are the only things no agent can produce, so they and only they are Sean's. They are
declared **NON-BLOCKING**: their absence drives termination state 2, not a halt.

**Measured 2026-07-26 on hetzner-dsm:** `FLY_API_TOKEN` absent, `WAYLAND_F25_CLOUD_TOKEN` absent,
`WAYLAND_F25_CLOUD_ORG` absent, `E2B_API_KEY` absent, `AWS_ACCESS_KEY_ID` absent,
`~/.aws/credentials` absent, `~/.fly/config.yml` absent. No vendor was closer to running than any
other — **binding condition C2**: the cloud leg is expected UNEXERCISED in this window and Success
Criterion 1 is to be graded NOT MET in those words unless a credential arrives.

Exact closing command and exactly what to mint: `evidence/25-01-cloud-credential-probe.txt`.

## Dissent

Preserved in full at `evidence/25-01-panel-dissent.txt`, naming all four options — including the
softest joint in the majority's reasoning, which is that `e2b-sandboxes` lost partly on the
UNMEASURED premise that SDK-first products move their raw HTTP surface.
