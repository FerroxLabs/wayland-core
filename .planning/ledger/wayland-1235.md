---
issue: 1235
repo: FerroxLabs/wayland
kind: defect
title: "One mock turn with a 480 KB tool result costs 63.6 s of CPU; the spill/read-back test is killed by the default nextest budget 3/3"
status: open
last_verified_commit: 6bcf1b503
criteria:
  - id: c1
    text: "Ask 1 -- where the 63.6 s goes. Profile one turn; name the function."
    state: met
    evidence: "test:crates/wcore-safety/tests/scrub_cost_probe.rs::scrub_cost_probe"
    owner: core
    note: "ANSWERED, and answered the way the ask is worded - the function is NAMED, not inferred. It is wcore_safety::PIIScrubber::scrub, reached through wcore_agent::output_redaction::redact_tool_output, which runs TWICE on every successful tool result (crates/wcore-agent/src/orchestration/mod.rs:2594 before truncation and :2607 after compaction). hetzner has neither perf nor gdb, so the naming is by microbenchmark plus a fix arm. Baseline debug, 480,000 bytes: scrub alone costs 22.833 s (47,569 ns/byte) against the whole turn's 44.063 s. Baseline release: 1.546 s (3,220 ns/byte); two calls is 6,441 ns/byte against the whole turn's measured 7,377 ns/byte, i.e. 87% of the term, in one function. THE BRANCH, by control: the same byte count shaped so it forms no base64 candidate ('.' every 8 characters) costs 96 ns/byte against solid's 12,242 - 128x - so the whole term is the encoded-secret branch (decoded_contains_secret, reached from base64_candidates / wrapped_base64_candidates). THE FIX ARM: changing only that function, fixture byte-identical, takes engine.run() at 480,000 from 44.063 s to 12.196 s in debug at a HIGHER host load. The full attribution, both profiles, both payloads, host load quoted with every figure, is on FerroxLabs/wayland-core#395 c1/c2, which is the same finding measured through its own probe; this ticket is the CI-visible face of it. The ticket's second observation - 'the cost is not proportional to the payload' - is now explained rather than left standing: the 60,000/120,000/240,000/480,000 table it quotes mixes arms in which the shed fired with arms in which it did not, and 120,000 FAILED outright because the fixture was under the shed trigger (see c3). On a controlled sweep the term is clean and linear in the payload - 2.00x the bytes costs 2.03x the seconds - in BOTH profiles."
  - id: c2
    text: "Ask 2 -- either that cost is justified on a 480 KB shed and is documented, or it is fixed."
    state: met
    evidence: "symbol:crates/wcore-safety/src/pii.rs::decoded_contains_secret"
    owner: core
    note: "FIXED, not documented-as-justified. The cost was not justified: it was redundant work, in two provably-equivalent places, and a third that was pure allocation. (1) decoded_contains_secret asked matches!(scrub_direct(..), Cow::Owned(_)). scrub_direct returns Cow::Borrowed if and only if !fast_set().is_match(input) - its first statement - and Cow::Owned on every other path, so the discriminant being read IS the pre-filter's answer, and the whole redacted string it built to produce that discriminant was thrown away. Asking fast_set() directly drops 25 Regex::replace_all passes and 25 full-length allocations per decode attempt. (2) The four base64 alphabets disagree only on '+/' vs '-_' and on padding, so an ordinary long alphanumeric run decodes to identical bytes four times and the expensive half was paid four times. Deduplicating on the decoded bytes is exact - a byte string already checked cannot answer differently on a second look - and decode order, and therefore the answer, is unchanged. (3) scrub_direct now takes replace_all's buffer only when it actually replaced (Cow::Borrowed means unchanged) instead of into_owned()ing a full copy once per pattern that did not match. Decode scans per scrub of one candidate run: 8 -> 2. DETECTION IS UNCHANGED and that is graded, not asserted: the whole crate is green (85 unit + 22 integration, including every_secret_the_scrubber_used_to_remove_is_still_removed), and a new control, an_encoded_secret_is_still_found_under_the_scan_ceiling, holds a real base64-encoded AWS key against the new scan ceiling so the ceiling cannot be met by not looking. MEASURED EFFECT, all on hetzner-dsm with host load quoted: the timing probe at a byte-identical 480,000 payload goes 44.063 s (load 46) -> 12.196 s (load 67), 3.61x and understated by the load difference. On the untuned second instance this ticket's own comments name, wcore-agent::engine_compact_test::tc_2_6_context_overflow_sheds_tool_output_and_continues: 44.4 s alone / FLAKY 2/2 at 54.589 s under load -> PASS 11.779 s at load 53. That test is not touched by c3's fixture change, so only the pii.rs fix reaches it. Full wcore-agent suite after the change: 3948 tests run, 3948 passed, 0 failed, 0 timed out, 4 slow - and neither spill_readback nor tc_2_6 is among the four any more. RESIDUAL, stated: the term is reduced, not removed - scrub still makes two candidate passes over the same span and each pays a full decode, a lossy allocation and a RegexSet sweep. That remaining ~2x is recorded on wayland-core#395 c3 and deliberately not improvised inside a perf fix to security code."
  - id: c3
    text: "Ask 3 -- the fixture constant is tied to the thresholds it must exceed, so the test cannot silently stop spilling."
    state: met
    evidence: "test:crates/wcore-agent/tests/spill_readback_engine_wiring.rs::the_engine_spills_where_this_session_can_read_it_back"
    owner: core
    note: "MET, and it closes the exact failure the ask names. The fixture was a literal 480,000-character payload written next to a hand-set window, and the ticket measured that halving it to 120,000 made the test FAIL - because the subject of the assertion (a spill happened) simply stopped occurring. It is now DERIVED from the two thresholds it has to sit between: config.compact.input_ceiling_for_window(WINDOW) * CHARS_PER_TOKEN is the shed trigger, max_result_size is the cap truncate_result applies BEFORE the estimate is taken, and the payload is twice the trigger. Both bounds are ASSERTED in the fixture rather than trusted, each with the arithmetic in its message, so a change to the reserves, to MAX_RESERVE_FRACTION or to the estimator reds here with the numbers printed instead of silently un-spilling. At today's config (window 60,000, output_reserve 10,000, emergency_buffer 10,000) that is 320,000 chars against a 160,000-char trigger and a 600,000-char cap. The value moved (480,000 -> 320,000) and that is reported rather than hidden: it is why the runtime figures on c4 are a larger improvement than the 3.61x the pii fix alone delivers on a byte-identical payload."
  - id: c4
    text: "Ask 4 -- only once 1-3 are answered: a [[profile.ci.overrides]] entry with the measured number, in the style of the existing ones, if it is still needed."
    state: met
    evidence: "test:crates/wcore-agent/tests/spill_readback_engine_wiring.rs::the_engine_spills_where_this_session_can_read_it_back"
    owner: core
    note: "MET as written - the ask is conditional ('if it is still needed') and it is NOT still needed, so no [[profile.ci.overrides]] entry was added. Adding one would have been the suppression the ticket explicitly refused ('Deliberately NOT allowlisted'). MEASURED, in the ticket's own two forms so the before/after is comparable line for line. /usr/bin/time -v on the compiled test binary, hetzner-dsm, debug (which is what CI runs): User time 63.60 s -> 7.56 s; System time 0.10 s -> 0.01 s; Elapsed 1:16.27 -> 0:07.58; Maximum RSS 38,060 KB -> 40,856 KB. Under nextest with the default profile (slow-timeout 30 s, terminate-after 2), which killed it 3 of 3 times at TIMEOUT 60.011 s: PASS 7.905 s at host load 59, no SLOW line. 7.9 s against a 30 s slow-timeout is not a budget problem, so there is no measured number to write into an override. The second and third bullets of the ticket's follow-up comment are addressed by the same fact rather than by an allowlist: the verdict tracked machine load because the runtime sat at 74% of the kill budget, and at 13% of it there is no margin left to lose. NOT CLAIMED: the wcore-cli instance - see c5."
  - id: c5
    text: "The other test named in this ticket's comments -- wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed -- is diagnosed, or is explicitly recorded as not covered here"
    state: not-met
    owner: core
    note: "OPEN and NOT CLAIMED, recorded so it is not read as covered by c1-c4. This ticket's comments carry three sightings of wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed (TRY 1 and TRY 2 both TMT at 60.005 s in a batch; PASS 58.986 s alone at load 46; PASS 58.778 s at load 44), and the comment that filed them says so itself: 'No diagnosis of packaged_f04 here - I did not profile it and it may be a different cause from the 480 KB tool result.' This lane did not profile it either. It is in wcore-cli, not wcore-agent, and nothing in its name or in the sightings ties it to a large tool result, so the c1 attribution does not transfer to it by argument and was not measured against it. Its margin is the worse of the two problems in this ticket - at 58.986 s against a 60 s budget, retries do not rescue it - and it is untouched here."
---

The 63.6 seconds is `PIIScrubber::scrub`, and it is not a test-profile
artifact: the same term survives `--release` at 7.4 seconds per megabyte of
tool output through one turn. The full debug-versus-release measurement is on
FerroxLabs/wayland-core#395, which is the product half of this finding; this
ticket is its CI-visible face and is graded on the four asks it wrote down.

Every tool result on the turn loop is scrubbed twice, and a whitespace-free
run of 24 characters or more -- an ordinary `Read` of a minified file, a
base64 blob, a long log line -- is one candidate that reaches the
encoded-secret branch at full length. That branch was doing eight
full-length decode scans where two suffice: a predicate that rebuilt an
entire redacted string to read a `Cow` discriminant the pre-filter already
had, and four base64 alphabets that decode an alphanumeric run to identical
bytes each paying for it separately. Both are fixed by equivalence, not by
sampling, and detection is held by the crate's own suite plus a new control
that redacts a real encoded AWS key under the new ceiling.

The ticket's second finding -- "the cost is not proportional to the payload"
-- turns out to be two different things stacked. The non-monotonic table
mixes runs where the shed fired with runs where it did not, and the 120,000
row FAILED because the fixture had dropped below the shed's own trigger. On
a controlled sweep the term is clean and linear in both profiles. The
fixture is now derived from that trigger and from `max_result_size`, with
both bounds asserted, so it cannot drift under its own subject again.

No `[[profile.ci.overrides]]` entry was added, because ask 4 is conditional
and the condition is gone: 63.60 s of user CPU became 7.56 s, and a test at
7.9 s against a 30-second slow-timeout has no measured number worth writing
into an override. That was the ticket's stated preference -- it refused the
allowlist entry on the grounds that it would suppress a product cost nobody
had measured. The cost is now measured, and mostly removed.

One thing here is NOT closed and is c5: the `wcore-cli` test in the
follow-up comments. It was never profiled, by the reporter or by this lane,
it is in a different crate, and nothing ties it to a large tool result. Its
margin is the worse of the two -- 58.986 s against a 60 s budget, where
retries do not help -- and it is untouched.
