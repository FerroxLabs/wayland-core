---
issue: 1356
repo: FerroxLabs/wayland
kind: defect
title: "ACP serve detaches a keeping-up reader at stage 2 when large events are followed by small ones"
status: open
last_verified_commit: 7222f23be
criteria:
  - id: c1
    text: "The mechanism is NAMED with the instrument that named it (which limit fired, at what reader position), with a deterministic red test that does not depend on load."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10 from the wayland#1352 review-round A/B (.planning/evidence/w15-1352/README.md, Review round section). OBSERVED on hetzner-dsm through the real acp serve, release builds, 'churn' arm (512 KiB events then 4 KiB events, aggregate retained history over the 64 MiB cap, sessions closing mid-batch), 112 fast rows at concurrency 32 per binary: stage-2 detaches `live delivery overloaded; resume from retained event cursor` of FAST readers were base-release 9b2e24f2 1/112, #1352 first repair d73cb623 1/112, #1352 review-round repair b08ee6d7 4/112 (Fisher p ~0.4, so not shown different and not shown equal). Every detached row had received only its 512 KiB events (16 or 11), so the detach lands at or just before the switch to small events. Stage-1 relay cancellations 0 on all three; every batch quiesced. Not seen with the uniform chunk sizes the wayland#1349 soak uses. HYPOTHESIS ONLY: a count-based limit (LIVE_EVENTS 256 behind, or the per-log 1,024-event cap evicting positions the reader still needs) reported as a stage-2 overload. Trackers searched for 'live delivery overloaded', 'resume from retained event cursor', 'stage-2 overload' and 'acp detach' in both repos: no matching ticket (control 'relay overloaded' returned #1352 in the same session)."
  - id: c2
    text: "A keeping-up reader is not detached by a large-then-small event pattern: the churn arm at n>=112 fast c32 rows shows 0 stage-2 detaches of keeping-up readers; a reader that genuinely falls behind is still detached (test); no bound raised without a measured justification; peak and quiescent RSS recorded on both arms."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
  - id: c3
    text: "If the limit is shown to be correct behaviour (the reader was not keeping up by the protocol's own definition), that is shown with numbers AND the host-facing resume path is demonstrated working for exactly this case, so the detach costs the user nothing."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. c2 and c3 are alternatives for the repair half; the row that does not apply is to be superseded with the reason when the mechanism is known."
---

Created 2026-09-10 at filing time, not retroactively. Pre-existing on the
0.13.14 base; surfaced by the wayland#1352 review-round churn arm.
