# `portability-exec` — executable-content fixtures for the F26-02 inertness proof

These are the payloads `tests/migrate_quarantine.rs` imports to prove that
imported executable content is **inert on disk** while quarantined and **does
run** once an operator promotes it.

## Why these are realistic and not strawmen

The classifier this plan builds calls
`wcore_skills::shell::contains_shell_commands` — the *same* predicate
`wcore_skills::permissions` keys its decision off and the same syntax
`wcore_skills::executor` actually runs through `sh -c`. A payload that did not
look like real peer content would prove nothing about real peer content, so
each `SKILL.md` here carries the frontmatter shape a real Hermes/OpenClaw skill
carries and a directive in the block form the executor recognises
(`` ```! `` … `` ``` ``).

## The sentinel

`skills/repo-status/SKILL.md` carries the token `__SENTINEL__` where a path
belongs. The test substitutes a path inside a **temporary directory unique to
that run** before writing the payload into the throwaway peer home, and asserts
the sentinel is absent *before* each leg begins. A stale artifact from an
earlier run therefore cannot satisfy the positive leg or contaminate the
negative one.

The payload's only observable effect is creating that file. It deliberately
avoids every pattern in `wcore_cron::runner`'s execution-boundary denylist
(`scan_target_text`), because a payload the denylist refuses would never reach
the shell and the positive control would not fire — the negative leg would then
be measuring the denylist rather than the quarantine boundary.

## What each fixture is for

| Path | Classification | Proves |
|---|---|---|
| `skills/repo-status/SKILL.md` | executable — shell directive | the paired inertness legs: absent while quarantined, present once promoted |
| `skills/release-notes/SKILL.md` | **data** — prose only, no directive | classification is not over-broad; a skill with no directive imports without ceremony |
| `skills/self-promoting/SKILL.md` | executable — shell directive | nothing the content carries can promote it. The body carries `trusted: true`, `auto_promote: true` and `wayland_quarantine: exempt` in its frontmatter |
| `skills/self-promoting/PROMOTE` | — | a marker FILE claiming promotion |
| `skills/self-promoting/manifest.json` | — | a manifest claiming `"promoted": true` |
| `hermes-config.yaml` | executable — MCP launch command | a peer MCP definition is a child process, not a setting |

The three self-promotion signals are the concrete forms of the GHSA-8r7g
failure this plan must not reproduce: a foreign artifact granting itself trust.
`QuarantineStore::promote` reads none of them.

## Provenance

Structure only. No file here is copied from Sean's real `~/.hermes` or
`~/.openclaw`; no real credential value appears in this directory. 26-01's
committed canary corpora under `fixtures/portability/` remain the source for
the count-scale corpora.
