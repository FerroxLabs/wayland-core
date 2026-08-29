---
issue: 113
repo: FerroxLabs/wayland-core
kind: defect
title: "Browser tool non-functional by default - Camoufox sidecar never spawned + policy-disabled (web automation/screenshots broken)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A browser op launches the configured Camoufox sidecar through the real production adapter path"
    state: met
    evidence: "test:crates/wcore-agent/tests/browser_sidecar_launch_wiring_test.rs::a_browser_op_launches_the_configured_sidecar_through_the_real_adapter"
    owner: core
    note: "asserts on the LAUNCHED PROCESS (a stub sidecar writes a file), not on a mock; the path is tool.rs ensure_session -> supervisor ensure_ready -> launch_camoufox_program"
  - id: c2
    text: "A machine with no sidecar on PATH provisions one by default instead of dead-ending on the download placeholder"
    state: met
    evidence: "symbol:crates/wcore-browser/src/binary.rs::provision_sidecar_via_npm"
    owner: core
    note: "reached from resolve_sidecar_program when the program is absent and the pinned-digest download path is off; SidecarAutoInstall defaults enabled for the operator-facing config"
  - id: c3
    text: "The deny-by-default refusal hands the operator a remedy that provably enables the tool"
    state: met
    evidence: "test:crates/wcore-agent/tests/browser_config_hint_roundtrip.rs::allowlist_snippet_actually_enables_the_tool"
    owner: core
    note: "the printed snippet is round-tripped through the real ConfigFile serde types to an actual Allow decision, so it cannot drift into prose that does not parse"
  - id: c4
    text: "No shipped doc still advertises the chromiumoxide fallback backend, which no longer exists in the tree"
    state: met
    evidence: "test:crates/wcore-browser/tests/no_phantom_backend_test.rs::no_shipped_source_advertises_a_chromiumoxide_backend"
    owner: core
    note: "1092e0c1 (red arm) then f83fcd2e (fix). The scanner walks shipped source with a positive control that fails if it reads nothing, plus the_two_real_backends_are_present so the prose cannot be fixed by deleting the real backends. registry-default.json - the description wayland plugin list prints - was fixed too and is guarded in plugin_install_smoke.rs."
  - id: c5
    text: "The deny-by-default browsing posture is recorded as a decision on the issue and the issue is dispositioned"
    state: blocked
    owner: maintainer
    note: "The issue is still OPEN and the deny-by-default posture is not recorded ON IT: the latest comment is a verification write-up, not a disposition. Q-113 in .planning/DECISIONS.md takes the decision (close as refuted, recording deny-by-default), and only the maintainer closes issues in this repo, so the remaining act is the maintainer's."
---

The issue reports that the browser tool is advertised to the model but cannot
work: the Camoufox sidecar is never spawned, no sidecar ships, the policy denies
everything, and the CDP fallback is stubbed.

Three of those four claims are refuted or superseded at v0.13.10. There is a
complete production launch path with an end-to-end regression test that watches
the real process; a fresh machine now provisions the sidecar over npm by
default; and the stubbed chromium backend was deleted outright rather than left
half-built. The fourth claim - deny-by-default policy - is still accurate and
still deliberate, but the opaque dead end it caused is gone: the refusal is
routed by reason and carries a config snippet that is proven to work.

What remains is a stale doc comment (c4) and a maintainer disposition (c5).
Criteria are transcribed from the cluster F verification note of 2026-08-29 and
each evidence token was re-checked against this tree before being cited.
