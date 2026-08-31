---
issue: 361
repo: FerroxLabs/wayland-core
title: "Shared-process lib suite: active_approval_token_split_by_truncation_leaves_no_fragment fails its anti-vacuity control under load"
status: open
kind: defect
last_verified_commit: d9f7e0a0
criteria:
  - id: c1
    text: "The mechanism is named: what makes output lack truncated under load is identified in code, not inferred"
    state: met
    owner: core
    evidence: "symbol:crates/wcore-agent/src/output_redaction.rs::redact_tool_output"
    note: "Not load. `redact_tool_output` runs before `truncate_result` (orchestration/mod.rs:2594 then :2600) and under the old ordering ran `PIIScrubber` FIRST. FIRECRAWL_API_KEY is `fc-[A-Za-z0-9]{20,}` and 1 `apr-<uuid v4>` in 256 has a fourth group ending `fc`, so the greedy match started INSIDE the token and ran through its last group and on through the 400 alphanumeric filler bytes. The 490-char payload collapsed to 133, `truncate_result` no-opped at its 200-char cap, and the anti-vacuity control found no 'truncated' marker. The token is redrawn every run, which is what made a fixed 1-in-256 input look scheduling-dependent."
  - id: c2
    text: "The failure is reproduced deliberately at least once, with the command and environment recorded, before any fix is written"
    state: met
    owner: core
    evidence: "commit:aa524efd"
    note: "Reproduced deliberately by inverting the one expression aa524efd introduced, on hetzner-dsm in /root/w-f13/fin-flake-584 at 5fe7f9fa: 1000/1000 red with the pinned token, and 6/2000 red with the original minted token (1-in-256 predicts 7.8; Poisson-consistent). The NEGATIVE is recorded too: on integ/f13 unmodified, the original minted-token fixture ran 1000 times with 0 failures. See the body for the commands."
  - id: c3
    text: "The fixture reaches the truncation boundary deterministically, independent of scheduling"
    state: met
    owner: core
    evidence: "test:crates/wcore-agent/src/orchestration/mod.rs::active_approval_token_split_by_truncation_leaves_no_fragment"
    note: "The token is pinned to `apr-00000000-0000-4000-80fc-000000000000` — the 1-in-256 adversarial uuid — instead of minted, so the payload is a fixed 490 bytes against a 200-byte cap on every execution and the boundary is reached with no dependence on scheduling, thread count or token draw. A control asserts the adversarial shape (`token.contains('fc-')`) so the fixture cannot be quietly de-fanged back to a benign token."
  - id: c4
    text: "Both assertions survive: the anti-vacuity control at mod.rs:5744 and the fragment assertion at :5749"
    state: met
    owner: core
    evidence: "file:crates/wcore-agent/src/orchestration/mod.rs:5782:a truncation-split token left the fragment {fragment} on the wire"
    note: "Both are present and unmodified, now at :5775 (control) and :5780 (fragment); only the token binding above them changed. Nothing was relaxed, removed or `#[serial]`-isolated, and the test still runs in the shared-process lib leg. The red arm proves both are live: the control fires first at 5775:9, and the stranded head it prints (`apr-00000000-0000-4000-80`) contains the 16-char fragment the second assertion looks for."
  - id: c5
    text: "A red arm is quoted verbatim: the fixture failing before the change, from a real run"
    state: met
    owner: core
    evidence: "file:.planning/ledger/wayland-core-361.md"
    note: "Quoted verbatim in the body below, from `cargo test -p wcore-agent --lib orchestration::tests::active_approval_token_split_by_truncation_leaves_no_fragment -- --exact --nocapture` on hetzner-dsm with the ordering inverted."
  - id: c6
    text: "After the fix, cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on the build host, and the run count is recorded"
    state: superseded
    successor: FerroxLabs/wayland-core#373
    owner: core
    handoff: "FerroxLabs/wayland-core#373"
    evidence: "commit:18e59e85"
    note: "NOT met, and deliberately handed over rather than left partial. The command was red on integ/f13 for FOUR independent shared-process races, none of them #361's. Three are closed in this lane: wcore-config's serial-group split (2c8efb46), wcore-observability's unserialized env-gate readers (812e26075), and wcore-cli emptying the process PATH under every other test (18e59e85f). The fourth is wcore-tools osv_check::tests::ssrf_refusal_is_visible_at_default_log_levels, measured at 5 failures in 100 runs on unmodified integ/f13, with two hypotheses tested and refuted -- it is filed with its full evidence and a written contract as #373, which carries this criterion verbatim as its own c5. Re-measured after merging current integ/f13 (d9f7e0a0): 12 failures in 100 runs, so it did not go away and got no better. Best observed streak across three attempts: 1 consecutive pass. Ten consecutive passes were NOT attempted again on purpose: against a live 5%-per-run flake a clean ten is a 60% coin flip, and recording that as evidence would be a lucky green, not a green."
---

## What this actually was

The reported symptom — a shared-process `--lib` leg going red where nextest is
green — was a coincidence of the single observed failure, not the mechanism.

`redact_tool_output` runs on the ORIGINAL tool result, before `truncate_result`
and before compaction (`orchestration/mod.rs:2594`, then `:2600`). Until
`aa524efd` it ran `PIIScrubber` first and the exact-match active-token scrub
second. `PIIScrubber` is itself a cutter and none of its patterns know where an
`apr-<uuid>` begins or ends. FIRECRAWL_API_KEY is `fc-[A-Za-z0-9]{20,}`; a v4
uuid whose FOURTH group ends in `fc` puts `fc-` inside the token, and the match
then runs from there through the final 12-char group and straight on into the
400 bytes of `[A-Za-z0-9]` filler the fixture deliberately parks behind the
token.

That is what broke the control. The payload the fixture builds is
`80*'A' + token(40) + 400*'B'` = 490 bytes against `EchoTool(200)`. The greedy
match replaced 415 of those bytes with a 28-byte placeholder, leaving 133 —
under the cap — so `truncate_result` returned the content unchanged, no
`"truncated"` marker appeared, and the anti-vacuity control refused to run the
fragment assertion against a case that was not the boundary case.

The token is minted fresh on every execution, so the input was redrawn each run
at a fixed 1-in-256 rate. That is why it presented as load- or
scheduling-dependent and why it could not be pinned to a tree.

## The mechanism is already closed; the fixture was not

`aa524efd` ("scrub approval tokens before the PII pass, not after") inverted the
order, so the token is now removed while it is still one contiguous string and
nothing greedy can reach out of it. It is on `integ/f13` and on two lanes; it is
in NO tag and not on `main`, so it ships with 0.13.12 — the brief's "shipped in
0.13.11" is wrong on the release number, not on the substance.

With that in, the failure is unreachable for ANY token: measured, the original
minted-token fixture ran **1000 times with 0 failures** on unmodified
`integ/f13`. But 1000 green runs of a fixture that only reaches its boundary for
255 uuids in 256 is a weak statement, and 10 green runs of it would be worth
almost nothing (a 1-in-256 input has only a 3.8% chance of appearing at all in
10 draws). So the fixture was pinned to the adversarial uuid. It now exercises
the worst case on every single execution rather than 0.39% of them, which is
what makes c6's run count mean something.

## The command did not pass, for a SECOND reason — now fixed

`cargo test --workspace --lib --no-fail-fast` was red on `integ/f13` for a
defect that has nothing to do with #361's redaction ordering, and c6 cannot be
met while it stands. Run 1 of the ten died on
`wcore-config`'s `command_floor::tests::the_yield_recognises_the_workspace_under_a_second_spelling`
— another anti-vacuity control, in another crate:

```
thread 'command_floor::tests::the_yield_recognises_the_workspace_under_a_second_spelling' (1039503) panicked at crates/wcore-config/src/command_floor.rs:1309:9:
control: the same path is refused when the session is NOT inside the profile home, or the assertion above proves nothing
```

Root cause: the eleven WAYLAND_HOME-mutating tests in `command_floor.rs` carried
a BARE `#[serial_test::serial]`, which takes serial_test's DEFAULT group. That
group is an independent lock from `#[serial(wayland_home_env)]`, where all 21
other WAYLAND_HOME mutators in the same lib binary live (`config.rs` ×20,
`env_file.rs` ×1). The eleven ran CONCURRENTLY with every one of them while
looking protected. A neighbour repointed `WAYLAND_HOME` between the subject's
`set_var` and its `floor_refusal` calls, the "not inside the profile home" case
stopped being refused, and the control did exactly what it exists to do.

This is not inference: `cargo test -p wcore-config --lib` on unmodified
`integ/f13`, run once, standalone, failed two tests — one from each side of the
split, which is the signature of a bidirectional env clobber:

```
---- command_floor::tests::rule_2b_yields_where_the_workspace_is_inside_the_authority_directory stdout ----
thread '...' (1433627) panicked at crates/wcore-config/src/command_floor.rs:1690:9:
a cd + relative `..` must not reach the entry list

---- config::tests::explicit_xai_api_key_outranks_ambient_oauth_credentials stdout ----
thread '...' (1433801) panicked at crates/wcore-config/src/config.rs:8575:14:
the OAuth credential must still resolve on its own: No API key found. ...

test result: FAILED. 772 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 159.94s
```

Fixed in `2c8efb46` by naming the group. The same diagnosis and the same fix
were already applied to `env_file::tests::load_wayland_env_file_applies_without_overriding`,
whose in-tree comment describes this exact trap — the `command_floor.rs` family
was missed. No serialisation was added and nothing was isolated: eleven tests
that were already `#[serial]` simply now hold the lock that matches the resource
they mutate.

## And a THIRD, in wcore-observability

With the `wcore-config` split closed, the next `--workspace --lib` run died in
`wcore-observability`:

```
---- trace::tests::with_result_snippet_truncates_at_utf8_boundary stdout ----
thread '...' (2216074) panicked at crates/wcore-observability/src/trace.rs:767:37:
snippet present

---- trace::tests::with_result_snippet_truncates_at_512_bytes stdout ----
thread '...' (2216073) panicked at crates/wcore-observability/src/trace.rs:755:37:
snippet present

test result: FAILED. 53 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

Same shape, one rung lower. `with_result_snippet` is gated on
`WAYLAND_TRACE_RESULT_SNIPPETS` via `env_gate::enabled_unless_disabled`, which
does a fresh `std::env::var` read on EVERY call ("so tests can flip the var at
runtime"). The tests that WRITE that var are `#[serial(env)]`. The three that
READ it through `with_result_snippet` carried no serial attribute at all — and
a group excludes its own members from each other, never an unmarked reader. A
writer held the var at `off` while a reader was inside `with_result_snippet`,
the snippet came back `None`, and `.expect("snippet present")` panicked.

Measured, 100 tight repetitions of the lib-test binary per arm:

| build | runs | failures |
|---|---|---|
| pre-edit (`trace.rs:755:37`) | 100 | 28 |
| post-edit (`812e2607`) | 100 | 0 |

An instrument note, because it nearly produced a false result: the first
attempt at that measurement selected the test binary with
`ls -t target/debug/deps/wcore_observability-*`, which picked a stale artifact
from an earlier build. It reported 21/50 failures on a tree that was already
fixed. The panic's line number (`755`, where the fixed file has `769`) is what
gave it away. The arms above are the two binaries cargo actually named on
stdout.

## Red arm, verbatim

Ordering inverted in `redact_tool_output` (the one expression `aa524efd`
introduced), pinned fixture, hetzner-dsm, `/root/w-f13/fin-flake-584`:

```
$ cargo test -p wcore-agent --lib \
    orchestration::tests::active_approval_token_split_by_truncation_leaves_no_fragment \
    -- --exact --nocapture

running 1 test

thread 'orchestration::tests::active_approval_token_split_by_truncation_leaves_no_fragment' (751657) panicked at crates/wcore-agent/src/orchestration/mod.rs:5775:9:
control failed: the fixture did not truncate: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAapr-00000000-0000-4000-80[REDACTED:FIRECRAWL_API_KEY]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test orchestration::tests::active_approval_token_split_by_truncation_leaves_no_fragment ... FAILED

failures:
    orchestration::tests::active_approval_token_split_by_truncation_leaves_no_fragment

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2616 filtered out; finished in 0.07s
```

The panic message IS the mechanism: 80 A's, then 25 plaintext characters of a
live approval token stranded on the wire, then the placeholder that swallowed
the token's tail and all 400 filler bytes. 80 + 25 + 28 = 133 < 200.

## Measurements

All on hetzner-dsm, `/root/w-f13/fin-flake-584`, tree at `5fe7f9fa`, driving the
compiled lib-test binary directly
(`./target/debug/deps/wcore_agent-<hash> <test> --exact --quiet`) in a loop:

| arm | ordering | token | runs | failures |
|---|---|---|---|---|
| negative (the finding) | fixed (`aa524efd`) | minted `apr-<uuid v4>` | 1000 | 0 |
| the fix, worst case | fixed (`aa524efd`) | pinned adversarial | 1000 | 0 |
| positive control | inverted | pinned adversarial | 1000 | 1000 |
| rate measurement | inverted | minted `apr-<uuid v4>` | 2000 | 6 |

The positive control matters: without it, `FAILURES=0` is indistinguishable from
a loop that cannot detect a failure at all. 6/2000 = 0.30% against a predicted
1/256 = 0.39% (Poisson mean 7.8, P(X<=6) = 0.34) — the arithmetic and the
measurement agree, which is what pins the mechanism to the uuid draw rather than
to load.

## Rejected instruments (do not repeat)

* `taskset` core-constraining: produces a DIFFERENT failure (stack overflow in
  `concurrent_near_cap_admits_exactly_one_retained_workspace`). Measures the
  instrument, not the subject. Carried over from the previous investigation.
* Anything that serialises, `#[serial]`-isolates, relaxes or deletes the control
  at :5775. That leg exists to catch what nextest's process-per-test cannot see;
  retiring the control retires the instrument. Refused by c4.
* Swapping the pinned token back to a minted one. That restores the 1-in-256
  flake and de-fangs the boundary test. The `token.contains("fc-")` control
  exists to make that change fail loudly.

## Two instrument failures worth not repeating

Both produced confident numbers that were entirely artefact, and both were
caught only by a sanity check rather than by the number looking wrong.

* **A stale test binary.** `ls -t target/debug/deps/wcore_observability-*` picked
  an artifact from an earlier build and reported 21/50 failures on a tree that
  was already fixed. Take the binary name from cargo's own `Executable ...` line
  at measurement time, never from a glob.
* **A binary that was not there at all.** A later loop scored 86/100 and then
  100/100 against a path that had stopped existing; every "failure" was exit
  127. The measurement script now resolves the path, checks `-x`, and runs one
  positive-control execution that must print a `test result:` line before the
  loop starts.

A third instrument was rejected before it could mislead: a filtered
high-collision A/B for the `wcore-cli` PATH fix scored 0/100 on BOTH the fixed
and the unfixed binary. It could not have failed, so it proves nothing, and the
fix is justified at the mechanism instead.

## One failure that was NOT ours, proven

`wcore-cli::harness_owns_spawned_trees::dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child`
failed the lane gate. It is the surviving-process-tree defect from
FerroxLabs/wayland#1156: "the grandchild N outlived the guard - killing the
direct child does not reach a backgrounded descendant".

Attribution, measured rather than argued: 3/3 FAIL at this lane's merge-base
`ab6b602f`, 3/3 PASS on `origin/integ/f13` as it stood afterwards. Pre-existing,
and fixed upstream between the two. The first attempt at that attribution was
invalid and nearly went the other way -- it checked out `origin/integ/f13`,
which had ADVANCED since this worktree was created, so it compared against a
newer tree rather than the base. The merge-base is the arm.

A further instrument note: reproducing it with `--no-capture` is wrong. That
flag makes nextest inherit stdio, and the subject is a test about detached
descendants holding a process tree open.
