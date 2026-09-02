---
issue: 411
repo: FerroxLabs/wayland-core
kind: feature
title: "A planning-only lane push buys a full cold hosted Windows compile+test it cannot use"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "A `lane/**` push that changes only `.planning/` and `*.md` does not pay a full cold hosted Windows compile+test, PROVEN by a live run showing the leg short-circuiting, not by reading the YAML."
    state: not-met
    owner: core
    note: "FILED 2026-08-31 by lane/f13-s3-ci-routing while discharging wayland-core#409 c6, whose fix (ci.yml:965) is what creates this cost. Not-met because nothing has been built against it. kind is FEATURE, not defect, and the classification is deliberate: nothing is broken. The pipeline now produces a correct Windows verdict for every lane push; it is merely more expensive than it needs to be for the majority of them, so this must not block a release the way a defect does."
  - id: c2
    text: "A `lane/**` push that touches any file outside that set still gets the full Windows test leg, proven in the SAME pair of runs as c1 -- a mitigation that suppresses the leg for a code push re-opens #409 and is worse than the cost it saves."
    state: not-met
    owner: core
    note: "The wrong-refusal control for c1, and the reason c1 may not be graded from a single run. A cost fix that saves 100% of the spend by never running the leg satisfies c1 alone and re-creates exactly the invisibility wayland-core#409 measured."
  - id: c3
    text: "The skip predicate is graded by a gate that can FAIL, driven red in both directions (suppressing a code push; admitting nothing at all), so it cannot widen silently the way the `if:`-only R4 rule would allow."
    state: not-met
    owner: core
    note: "MEASURED SCOPE LIMIT, recorded so this is not discovered later: `scripts/check-windows-attribution.py` R4 evaluates the `ci-windows-hosted` `if:` expression and nothing else. Verified on 2026-08-31 by driving R4 red twice against the real ci.yml -- once with the marker-only `if:` restored, once with the `if:` over-relaxed to `github.event_name == 'push'`. Both fired; both left YAML_PARSE_EXIT=0, so each red was a routing verdict rather than a syntax break. A step-level skip inside the job is outside everything R4 reads."
  - id: c4
    text: "A short-circuited leg reports as ABSENT Windows coverage in `report`, never as a pass -- measured against `annotate-windows-coverage.sh` output, not inferred."
    state: not-met
    owner: core
    note: "gh#1146 is the precedent: a skipped leg contributes no red and therefore silently contributes to a green. `.github/scripts/annotate-windows-coverage.sh` counts `<testcase>` elements rather than job conclusions, which is why a short-circuited leg CAN be reported honestly -- but that interaction has to be measured, because the script has never been exercised against a leg that ran, uploaded nothing, and concluded success."
---

Filed 2026-08-31 by `lane/f13-s3-ci-routing` as the carrier for a cost measured while
discharging `wayland-core#409 c6`. It exists so that measurement has an owner and a
trigger instead of decaying inside a criterion note that closes with #409.

THE MEASUREMENT, so it is not re-derived. The 58 most recent `lane/**` CI runs were
deduplicated to unique head commits and the 40 most recent classified by changed path:
15 touch code / `Cargo.*` / `.github/`, and 25 -- 62.5% -- touch `.planning/` and
`*.md` ONLY. A `.planning`-only push leaves the compiled tree byte-identical to its
parent, so the hosted Windows leg re-derives an unchanged verdict from a cold image.

PRICE PER LEG, from `ci.yml`'s own recorded numbers rather than an estimate: the
hosted leg is recorded at 39+ min and still running on this image against
`Build (x86_64-pc-windows-msvc)` at 32 min cold on the same image, it carries
`timeout-minutes: 180`, hosted Windows bills at the 2x rate, and this workflow's
`concurrency:` deliberately does not cancel branch pushes so successive pushes to one
lane stack rather than supersede.

NOT A REGRESSION AND NOT AN ARGUMENT AGAINST #409 c6. Before that fix the same lane
pushes paid nothing for Windows AND got no Windows verdict, which is the defect #409
was filed for. This ticket asks for the same verdict at a lower price, never for less
verdict.
