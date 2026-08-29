---
issue: 559
repo: FerroxLabs/wayland
title: "Team leader token burn: 77.7M input tok/session, cache_read=0 — enable prompt caching + trim re-billed context (Core/Flux)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The turn-1 transient that poisoned the cache prefix is removed from messages[1]"
    state: met
    evidence: "commit:6762f218"
    owner: core
    note: "tracked in full as #1168"
  - id: c2
    text: "The OpenAI adapter no longer drops accompanying text on tool-result turns"
    state: met
    evidence: "commit:d6f64be2"
    owner: core
    note: "d6f64be2 is the red arm; the second root cause found in stage-2"
  - id: c3
    text: "Ask 1 — enable prompt caching where it was off"
    state: not-met
    owner: core
    note: "REFUTED, not unfixed: measured live against FluxRouter on 2026-08-25, caching was already on. Recorded so nobody re-opens it as work"
  - id: c4
    text: "This ticket's own close condition: ONE real 26-turn Desktop team run showing non-zero cache_read"
    state: not-met
    owner: core
    note: "the 0.0358 -> 0.6526 measurement is a 7-round-trip synthetic rig on flux-router. Closing on that proxy is exactly the substitution this ticket exists to catch"
  - id: c5
    text: "Ask 2's second half — the sub-call count is reduced, or shown not to need reducing"
    state: not-met
    owner: core
    note: "no trace of this half in any release"
  - id: c6
    text: "The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1"
    state: not-met
    owner: core
    note: "the residual #1168 closed with. Same shape as the 26-byte date that dominated the measured collapse, much smaller, and a live contributor to the real-run number c4 is waiting on"
---

Both root causes are fixed and the effect is measured: hit ratio 0.0358 ->
0.6526, monotonic, with no late collapse.

It stays open because that measurement is a synthetic 7-round-trip rig, and
this ticket measured a real 26-turn Desktop team leader. Its written close
condition is one real team run showing non-zero `cache_read`. That run has
not happened. c3 is recorded as an explicitly REFUTED ask so that a future
reader does not resurrect it as outstanding work — an ask that was wrong is
not the same as an ask that is pending.

c6 arrives here from #1168, which closed with that residual stated. This is
where it lives now: #1168 hands it over through a `superseded` criterion, and
the gate refuses that handover if this ticket ever closes with c6 unmet.
