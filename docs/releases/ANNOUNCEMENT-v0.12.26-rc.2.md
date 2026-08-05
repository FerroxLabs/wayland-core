# Wayland Core 0.12.26-rc.2 — the honesty release

**2026-08-05.** Release candidate for 0.12.26. 2,918 commits since 0.12.25:
250 features, 629 fixes, 519 new tests, 740 documentation changes. The largest
release we have shipped, and not mostly because of new surface.

```bash
npm install @ferroxlabs/wayland-core@next
```

This is a **pre-release**. The stable 0.12.25 line is untouched — a plain
`npm install @ferroxlabs/wayland-core` still gets you stable.

> **rc.1 was withdrawn before it reached npm.** It shipped a Linux x86_64
> binary that could not start on a host without ALSA installed, because we put
> voice in the default feature set and that adds a hard `libasound.so.2`
> dependency on Linux. Our own release smoke — which runs the published
> artifact on a clean Rocky Linux 9 — caught it and refused to publish. The tag
> and pre-release were deleted; nothing reached the registry. rc.2 fixes it and
> adds the build-time gate that would have caught it on both Linux targets
> instead of one. We are telling you this rather than quietly renumbering,
> because that is the entire point of this release.

---

## The short version

We spent this cycle holding the product to one rule: **nothing lies to the
user, and nothing loses their data.**

That turned out to be a much bigger job than adding features. Most of what
follows is the product being made to tell the truth about what it can actually
do.

**Some of this release is us withdrawing claims.** Discord and Slack are now
documented and enforced as *at-most-once* delivery, because that is what those
platforms actually do — our exactly-once claim was wrong. Matrix's exactly-once
guarantee now ships with the precondition it always had. An advertised egress
safety interlock that did not exist has been removed rather than implemented in
name only. We think a release note that only lists wins is a release note that
lies by omission.

## The headlines

**Your credentials.** The plaintext credential fallback is gone, replaced by a
fail-closed ladder. A host with no secure store now refuses to save a secret
instead of quietly writing it in the clear — and tells you so. We also found
and fixed a defect where two concurrent writers could splice two different
secrets together, with no crash to warn you. Three surfaces that reported a
*locked* token store as "signed out" now tell the truth.

**Your privacy.** `memory.enabled = false` did not stop memory from recording.
It does now. A bare `[memory]` block in a repo you cloned could silently undo
your global opt-out. It cannot now. You can see and switch off exactly what
memory puts in your prompt.

**Your data survives a crash.** Sessions are crash-complete and durable. A
clean exit used to write a journal the product could not read back — fixed, and
older journals recover without loosening the checksum. Backups capture live
SQLite consistently instead of archiving three files that disagree with each
other. Skill rollback is atomic instead of rewriting your directory file by
file.

**Windows is a first-class platform now, not a caveat.** It got the largest
share of this release. The last release blocker was a Windows defect where a
read-only accounting scan demanded delete rights, so a live `git` process in
your checkout would kill a perfectly healthy worker and report the wrong
reason. Path handling, log rotation, worktree reclaim, the sandbox probe, shell
quoting, daemon survival — all repaired, and the AppContainer probe went from
140 ms/op to 68 ms/op under 24-way contention.

**Durable Goals across five engines.** One canonical Goal taxonomy and one
terminal transition, driven from the shipped binary over Anvil, Council,
Crucible, ForgeFlows and Direct — controllable from the host protocol, the CLI
and the TUI, and observable over the JSON stream.

**A gateway you can actually operate.** Exactly-once delivery ledger,
observable drain, a single-owner inbound lease, and abandoned deliveries you
can name, acknowledge and re-send instead of losing. Plus `gateway
support-bundle`, redacted and proved by canary.

**Every MCP server was unusable.** The model was never told a hydrated tool was
callable. That is fixed.

**Releases you can verify.** A signed release manifest with a role-scoped trust
root, a deterministic CycloneDX SBOM, keyless Sigstore build provenance on every
archive, and an updater that binds the download to the signed digest and fails
closed.

## What we changed about how we work

Several gates in this repo could not fail. One could not pass. Both are equally
worthless, and we found both.

A deleted failing test used to satisfy the merge gate. A bare `cargo test`
could report vacuous green. A leg with zero tests read as a pass. The
anti-vacuity linter we wrote to catch this had never actually run — and found
four real problems the first time it did. Meanwhile the Windows packaged gate
had *no reachable pass state at all*, because it demanded kernel resource
samples that Windows Job Objects never produce.

Where a harness was wrong rather than the product, this release says so. A
significant number of commits are recorded as instrument repairs, not as
product fixes, because pretending otherwise would inflate the changelog with
bugs that never existed.

## Breaking changes

- `--dangerously-skip-permissions` is split into two named tiers, so the flag
  name shows you when sandbox bypass is included.
- The plaintext credential fallback is removed.
- `[default] read_only` now actually enforces.
- An untrusted project config can no longer raise `max_tokens` / `max_turns`.
- The egress master switch is operator-owned, not project-negotiable.
- Discord delivery is at-most-once.
- OAuth tokens move into the credential ladder.
- Voice is default-on for macOS and Windows and opt-in on Linux, because
  linking ALSA means the binary cannot start without it.

## Known open items

We ship the list rather than the impression. Two sibling code paths still
request delete rights on a read-only walk on Windows — same class as the
blocker we just closed, deliberately left out of this RC because they are
unmeasured and measure-then-fix is what worked. A delete-disposition errno is
still stringified where the new retry cannot read it. Startup log residuals,
log rotation on the new headless log, two named retry-masked flakes, and a
dated dependency-suppression expiry are all tracked.

**Full detail:** [`docs/releases/v0.12.26-rc.2.md`](./v0.12.26-rc.2.md)

## Verification

All three platform CI legs green — Windows, Linux, macOS — plus all six release
target builds, the eval acceptance gate, browser e2e, lint, supply chain, OSV,
bench regression and the release rehearsal.

The Windows blocker was proven closed with 100/100 live runs on the committed
tree against a ~33% baseline failure rate, 277/277 on the Windows swarm and
sandbox suites, and 263/263 on Linux.

Please file anything you hit. An RC is the point at which we most want to be
told we are wrong.
