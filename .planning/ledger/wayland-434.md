---
issue: 434
repo: FerroxLabs/wayland
title: "Flux tier-alias -> strict-reasoner: #417 replay gap (engine keys off request.model, alias resolves server-side)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The replay socket is populated, so the engine has somewhere to learn the served model from"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "The gap is closed for the turn on which the alias first resolves, not only for turn N+1"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::the_turn_the_alias_first_resolves_recovers_from_the_refusal"
    owner: core
    note: "The old note is right that the socket cannot cover turn N - a router answers on the way back - so the turn is closed from the OTHER side: the refusal itself is the route signal. symbol:crates/wcore-agent/src/engine.rs::is_missing_reasoning_content_rejection is a narrow 400 classifier (the wire field named AND described as absent/required, with not-supported winning over absence wording, because a model that rejects the field cannot be appeased by sending more of it). On a match the engine sets symbol:wcore_types::llm::LlmRequest::replay_reasoning_content and re-issues the SAME turn once - bounded like the sibling orphaned-tool-pair repair - and the flag is sticky for the conversation so a later turn is shaped correctly from its first send (test:crates/wcore-agent/src/engine.rs::the_learned_contract_outlives_the_turn_that_paid_for_it). The provider honours it at symbol:crates/wcore-providers/src/openai.rs::message_compat, force-only-ON (test:crates/wcore-providers/src/openai.rs::message_compat_replays_when_the_engine_forces_it_on_a_bare_alias). Journaled with the prepared request so a recovered turn is not rebuilt without it. Narrowness is graded, not assumed: an auth 400 and a not-supported refusal are each billed exactly once (test:crates/wcore-agent/src/engine.rs::an_unrelated_400_is_not_re_sent, test:crates/wcore-agent/src/engine.rs::a_not_supported_refusal_is_not_re_sent). RED ARM, run: short-circuiting the retry gate with `if false && ...` reddened it - `the FIRST turn must complete, not only the one after it: Provider(Api { status: 400, message: \"messages[1]: missing field `reasoning_content`; the last assistant message must contain reasoning_content\" })`. Restored + touched, 7/7 green."
  - id: c3
    text: "The alias-resolves-server-side path is closed end to end"
    state: blocked
    owner: flux
    note: "requires the router to declare the resolved model on the turn it resolves it; core cannot close this alone. Ticket carries needs:flux"
---

Partially fixed in v0.13.10. The engine keys reasoner replay off
`request.model`, but a Flux tier alias resolves to a concrete strict-reasoner
server-side, so the engine is deciding on a name that is not the model.

The replay socket now exists and is populated, which is core's half. It
covers turn N+1 by construction — there is nothing to learn from until the
first response comes back — so a single-turn run still gets the alias
behaviour. Closing that requires the router to say what it resolved, which is
the flux lane's change, not core's.
