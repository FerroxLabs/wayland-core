# F19 unified spawner seal

Date: 2026-07-17

Integrated commit: `135809a50f088fbbe676c919e39f929b318091ae`

Candidate commit: `34819751cc84f77d04894721056f0fe3d3c5795b`

F19 routes production child launches through the F18 durable lifecycle while
preserving each launch surface's explicit foreground/background behavior.
Spawn, Delegate, skills, workflows, swarm, mesh, fleet, and host-created work
now share durable identity, admission, cancellation, result delivery, and
restart-visible state rather than maintaining independent child semantics.

## Acceptance evidence

- The deterministic packaged child-lifecycle loop passed 13 of 13 cases.
- Full `wcore-agent --all-targets` and `wcore-cli --all-targets` test matrices
  passed on the candidate.
- Scoped Clippy with warnings denied and formatting checks passed on the
  candidate.
- Independent review of exact range `034554b...34819751` found no BLOCKER or
  HIGH findings.
- Candidate-to-integration source comparison differed only in the locked build
  plan document; no candidate source file changed during integration.

## Exact integrated-HEAD proof

Executed through the Hetzner remote Cargo harness against
`135809a50f088fbbe676c919e39f929b318091ae`:

```text
cargo test -p wcore-agent -p wcore-cli --all-targets
```

The command completed with exit code 0. The `wcore-agent` unit target ran
2,012 cases with 2,009 passed and three ignored, and the `wcore-cli` unit
target passed 1,683 tests with one ignored test; all executed integration-test
and benchmark targets in the command completed without failure.

F19 is sealed. F20 owns transactional delegated mutation; F21-F23 remain
outside this seal.
