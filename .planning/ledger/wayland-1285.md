---
issue: 1285
repo: FerroxLabs/wayland
kind: defect
title: "Two more macOS-only retry flakes: harness_tui_flow resume_repaints and wcore-mcp f016_real_spawn"
status: open
last_verified_commit: e16de82cd
criteria:
  - id: c1
    text: "Both tests are either made deterministic under parallel nextest on macOS, or their allowlist entries are deleted at expiry because a normal-duration run stopped reproducing them."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "STILL NOT MET. Neither test was touched: `harness_tui_flow.rs` and `wcore-mcp/src/transport/stdio.rs` are outside this lane's ownership, so only the deletion branch was available -- and the census below closes it. CI-HISTORY CENSUS, 2026-09-10, and it REFUTES the deletion branch of this criterion for all six lines this lane was asked to grade. Method: every `ci.yml` run `gh run list` returns at limit 200 (2026-08-31T04:13 .. 2026-09-09T12:32); for each, every unexpired `nextest-junit-<leg>` artifact downloaded and every `<testcase>` carrying `<flakyFailure>`/`<flakyError>` counted. 126 of the 200 runs had JUnit artifacts: macos-latest 103, linux-containerized 112, Array 110, windows-latest-hosted 8. This is a per-RUN flake incidence at the CI profile's `retries = 2`, NOT a per-execution failure rate at `--retries 0` -- a run counts once if the test failed at least one attempt and then passed. It cannot substitute for the `--retries 0` population job any of these tickets asks for, and is not offered as one. BOTH LINES ARE STILL REPRODUCING AND MUST NOT BE DELETED. `f016_real_spawn_uses_sanitized_launch_context`: 7 of 103 macos-latest runs (6.8 percent), 8 flakyFailures, runs 33437649161, 33481688035, 33629541563, 33804424314, 33804772883, 33866807735, 34212497448 (x2) -- the last on 2026-09-08, two days before this grading. ZERO on linux-containerized and ZERO on Array across the same window, so its `macOS ONLY` claim SURVIVES the census. `resume_repaints_prior_conversation_into_the_transcript`: 20 flake-runs, of which only 4 are macOS (33437649161, 33726557626, 33842165634, 34173358674) and 16 are linux-containerized (33425600515 x2, 33574149322, 33637957153, 33705709380, 33707789181, 33708958434, 33711272322, 33719833206, 33726557626, 33783325082, 33841983575, 33843515833, 33898018654, 34173361738, 34207142616, 34318072596, last on 2026-09-09). At 16/112 Linux against 4/103 macOS this is a LINUX flake that also appears on macOS, not the reverse; the 2026-09-03 correction already on line 66 of the allowlist understated it as a single occurrence. WHAT IS OWED: (1) a bounded native macOS job, `--retries 0`, n>=20 per test, for the rate neither line has -- the same job #1284 and #1286 need, and the reason all three expire 2026-10-01 rather than the usual two months; (2) for resume_repaints a LINUX arm of equal n, because the platform that decides it is Linux and no `--retries 0` Linux measurement exists either. EXPIRY IS 2026-10-01 FOR BOTH LINES, not 2026-09-20."
---

# Two macOS retry flakes

Ledgered for coverage. Repair or expiry is 0.13.13 work.
