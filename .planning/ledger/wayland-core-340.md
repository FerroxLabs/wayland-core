---
issue: 340
repo: FerroxLabs/wayland-core
title: "The MCP malware gate does not cover every launch, and its doc claims it does"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The report calls this the cheap fix regardless of the rest - an overstated security guarantee is worse than an understated one because it stops the next person looking"
  - id: c2
    text: "An indirect runner shape such as a shell command wrapping a registry package does not reach exec unchecked"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The report verifies that a NotApplicable classification is permitted, and reports but does NOT verify end to end that sh -c npx evil-pkg classifies that way"
  - id: c3
    text: "The fail-open on an unreachable OSV endpoint is an explicit operator choice, and where it stays open it is visible to the user"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The report accepts the fail-open as deliberate and tested, but notes that blocking the OSV host is enough to run a known-malicious package, and that a warn-level log reaches nobody with RUST_LOG unset"
  - id: c4
    text: "The wayland-ijfw npx reachability probe is confirmed to run after the gate, or is moved behind it"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The reporter confirmed the file constructs an npx command but explicitly did not confirm the ordering. If the probe runs first it is a production pre-exec bypass and package install code runs before OSV is queried"
  - id: c5
    text: "Each runner form has a test pinning which token is queried - uvx, npx, pipx run, pipx install, --from and --with"
    state: not-met
    owner: core
    note: "seeded from the issue body and NOT graded against the tree. The claim that pipx run queries the literal run is marked likely partially wrong by the reporter, since the parser already reasons about entry-point-versus-package for some runners. It needs a test per form rather than an assumption either way"
---

The OSV malware gate refuses an MCP stdio launch whose command names a package
with known malware advisories. It is a real improvement - before it, config.toml
MCP servers launched arbitrary npx and uvx packages entirely ungated. The
complaint is that its doc comment claims every stdio launch is checked before
execution, and that claim is broader than the code.

READ THIS BEFORE USING THIS FILE. Unlike the other wayland-core entries seeded
in this pass, #340 was not covered by the v0.13.10 verification sweep. Every
criterion above is transcribed from the issue body alone and every one is
recorded not-met because nothing here has been graded against the shipped tree.
A not-met here means unverified, not measured-absent.

That distinction matters more than usual for this issue, because the body is
itself a cross-audit that labels its own claims: two are marked verified by the
reporter, two are marked reported-but-unverified, and one is marked likely
partially wrong. Anyone picking this up should re-read
crates/wcore-mcp/src/malware_gate.rs and the osv_check parser at the shipped
commit before treating any of the five as established.
