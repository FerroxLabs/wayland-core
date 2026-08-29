---
issue: 354
repo: FerroxLabs/wayland-core
title: "MCP malware gate: make the OSV fail-open an explicit operator choice (strict/permissive)"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A config key selects the malware-gate mode, permissive or strict, defaulting to today's permissive behaviour"
    state: not-met
    owner: core
    note: "No malware_gate key exists anywhere under crates/wcore-config/src in the graded tree. This is the policy surface #340 explicitly did not ship."
  - id: c2
    text: "Under strict, a backend error from check_package_for_malware refuses with McpError::MalwareBlocked instead of returning Allowed"
    state: not-met
    owner: core
    note: "malware_gate.rs raises MalwareBlocked only for Unidentified (line 209) and Blocked (line 218). A backend error still falls through to Allowed in both directions because there is only one direction."
  - id: c3
    text: "The SSRF short-circuit, where is_safe_url fails, follows the same mode as the backend-error path"
    state: not-met
    owner: core
    note: "Cannot follow a mode that does not exist. Graded not-met rather than n/a so it is not lost when c1 lands."
  - id: c4
    text: "A test per mode drives StdioTransport::spawn against an unreachable backend: permissive launches and logs at ERROR, strict refuses and never reaches exec"
    state: not-met
    owner: core
    note: "The permissive half is graded today by osv_check fail-open visibility; the strict half has no test because it has no behaviour."
  - id: c5
    text: "A negative control shows a clean package launches in BOTH modes"
    state: not-met
    owner: core
    note: "Required by the ticket so a strict mode that refuses everything cannot pass as a fix."
  - id: c6
    text: "The mode is documented in docs/mcp.md and surfaced in /doctor"
    state: not-met
    owner: core
    note: "docs/mcp.md carries no malware_gate section in the graded tree, and /doctor reports no gate mode."
---

Split out of `#340`. That ticket asked for two things on the OSV fail-open: at
minimum make it VISIBLE, and better, make it an explicit operator choice. The
minimum shipped on `lane/swarm-mcp` — a network, HTTP, timeout or parse failure
now logs at ERROR, the only level that reaches a user with `RUST_LOG` unset. The
operator choice did not, and it is deliberately not a log-level change: it is a
config surface with a docs and `/doctor` face and its own review.

The default must stay `permissive`. Refusing every MCP launch when the machine is
offline is a real product regression for anyone working on a plane, so this knob
is only worth adding with the mode plumbed all the way through.

Note the related standing decision Q3 in `.planning/DECISIONS.md`: the visibility
question was answered "a typed protocol frame, not a log level", and that frame
lands with Q4 (`FerroxLabs/wayland#1099`), not here.
