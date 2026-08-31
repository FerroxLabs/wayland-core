---
issue: 1212
repo: FerroxLabs/wayland
kind: defect
title: "The #1173 keyless self-hosted exemption is applied at one credential gate and not at resolve_council_provider"
status: closed
last_verified_commit: 65b95a87
criteria:
  - id: c1
    text: "A keyless self-hosted council member is not classified CouncilProviderError::Keyless"
    state: met
    evidence: "test:crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs::a_keyless_self_hosted_council_member_is_not_skipped"
    owner: core
    note: "Graded HERE rather than in the config.rs inline module because this file strips every credential variable (NoCredentialEnv) and redirects WAYLAND_HOME, so the row cannot pass on a developer's ambient OPENAI_API_KEY. Its negative control is in the same file: a_keyless_public_council_member_is_still_skipped drives the same council gate at https://api.openai.com AND at the #1211 spelling https://api.openai.com?x=@127.0.0.1 and asserts CouncilProviderError::Keyless for both, so the exemption cannot be read as 'the council stopped checking credentials'. RED ARM 2026-08-30 (mutation M2): the `None if declared_keyless_self_hosted_endpoint(...)` arm deleted from resolve_council_provider -- diff-verified to remove the executable match arm, printed before and after, the surviving text being only the comment above it -- gives verbatim: `thread 'a_keyless_self_hosted_council_member_is_not_skipped' panicked at crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs:326:13: a council member at a keyless self-hosted endpoint must be built, not skipped -- the same config starts fine on the main CLI path. Got: Keyless(\"openai\")`. Every #1211 test in the same run stayed GREEN (`10 tests run: 8 passed, 2 failed`), so M2 is discriminating. Restored, touched, re-run green."
  - id: c2
    text: "The two gates consult ONE predicate rather than two re-implemented chains, or a test asserts they agree on identical config"
    state: met
    evidence: "symbol:crates/wcore-config/src/self_hosted.rs::declared_keyless_self_hosted_endpoint"
    owner: core
    note: "BOTH branches of the criterion, not the cheaper one. ONE PREDICATE: the three conditions of the #1173 exemption were written inline at config.rs:2683-2686; they now live only in this function and both gates call it -- Config::resolve at config.rs:2683 and resolve_council_provider at config.rs:3600 (the council's compat is resolved before the credential chain now, because the exemption is a function of it). AND A TEST: both_credential_gates_agree_on_identical_config (same file as c1) drives both gates over http://127.0.0.1:11434, https://api.openai.com and https://api.openai.com?x=@127.0.0.1 through the same [providers.openai] base_url declaration and asserts one verdict, then asserts that verdict is the expected one -- so it cannot pass by both gates being wrong together. RED ARM 2026-08-30, TWO mutations in OPPOSITE directions so this row does not ride on c1's: under M2 (council arm deleted) it fails `assertion left == right failed: http://127.0.0.1:11434: the startup gate and the council gate must reach the same verdict on identical config (startup exempt=true, council exempt=false)` -- the defect as filed; under M3 (the council call site alone passing a hardcoded \"http://127.0.0.1\" for the URL argument, diff-verified on the executable argument list) it fails the other way, `... https://api.openai.com: ... (startup exempt=false, council exempt=true)`, while a_keyless_self_hosted_council_member_is_not_skipped stays GREEN. M3 also demonstrates the first branch: with one shared predicate the only way to make the gates disagree is to lie to it at a call site."
  - id: c3
    text: "A test drives a council member at a keyless self-hosted endpoint and asserts it is not skipped; shown RED against today's code"
    state: met
    evidence: "test:crates/wcore-agent/src/orchestration/council/resolver.rs::a_keyless_self_hosted_member_stays_in_the_runnable_pool"
    owner: core
    note: "Graded at the layer where a member is actually SKIPPED, not at the classifier -- resolvable_specs is the auto Assembler's runnable pool, downstream of the CouncilProviderError -> ResolveError::Keyless mapping at resolver.rs:111 that run.rs:183-190 turns into a SkippedProposer. The assertion is `runnable == [\"openai\"]` over candidates [\"openai\" (declared http://127.0.0.1:11434, no key), \"cohere\" (no key anywhere)], so the cohere row is a wrong-refusal control inside the same assertion: a pool that simply stopped filtering fails it. Non-vacuity of the environment was checked rather than assumed -- OPENAI_API_KEY, COHERE_API_KEY, API_KEY and ANTHROPIC_API_KEY are all unset on the gate host and ci.yml sets none of them, and the RED ARM proves it directly. RED ARM 2026-08-30 under mutation M2 (see c1 for the diff verification): `thread 'orchestration::council::resolver::tests::a_keyless_self_hosted_member_stays_in_the_runnable_pool' panicked ... assertion left == right failed: the keyless self-hosted member must stay runnable, and the genuinely keyless BYO member must still be dropped, left: [], right: [\"openai\"]`, with resolvable_specs_keeps_keyed_drops_keyless_and_dedups and resolve_skips_genuinely_keyless_provider both still GREEN. Restored, touched, re-run green."
---

`resolve_council_provider` re-implements the credential chain for council
members and did not consult the #1173 exemption, so a member pointed at a
keyless local endpoint was classified `Keyless` and dropped before spawn --
the same configuration the main CLI path runs happily.

The three conditions of the exemption now live in exactly one function,
`self_hosted::declared_keyless_self_hosted_endpoint`, and both gates call it.
Closed AFTER wayland#1211 on purpose: sharing a predicate that misread
`?x=@127.0.0.1` would have spread that misclassification to a second gate,
which is strictly worse than the divergence it fixes.

Fixed in 059291c0e and 65b95a875 on lane/f13-sec-credgate.
