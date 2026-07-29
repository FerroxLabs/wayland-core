# 26-SC2-PEERS — running notes

Lane `lane/26-sc2-peers`. Base `5be910561f688c75d39492e7b982d6e100772a64`
(`gh/plan/f20-unified-audit-repair`), SHA asserted against `git ls-remote gh` before
any work. Every number here from an unproxied tool (`/usr/bin/grep`, `/usr/bin/find`,
`/usr/bin/git`).

## T0 — the brief's premise re-verified at HEAD

Brief claim: peer coverage is **2 of 4**; no importer for `grok-build` or `gemini-cli`.

```
$ /usr/bin/grep -rniE "grok|gemini" crates/wcore-cli/src/migrate/     → 0      (known-negative)
$ /usr/bin/grep -rniE "hermes|openclaw" crates/wcore-cli/src/migrate/ → 150    (known-positive, same matcher)
$ crates/wcore-config/src/portability/mod.rs:45 enum PeerSource       → Hermes, OpenClaw  (2 variants)
$ /usr/bin/find crates/wcore-cli/src/migrate -type f
    content.rs hermes.rs mod.rs openclaw.rs provenance.rs quarantine.rs rollback.rs select.rs
```

**"2 of 4" HOLDS at HEAD.** The known-positive licenses the zero: the same matcher on the
same tree returns 150 for the two peers that do exist, so the instrument is alive.

One incidental drift vs 26-SC2-SUMMARY §6, which said *"`migrate` still has no rollback
(G3, untouched here)"*: `crates/wcore-cli/src/migrate/rollback.rs` (468 lines) EXISTS at my
base. Lane `26-sc3-rollback` landed since that summary was written. Not my scope; recorded
so no one re-derives it.

## T1 — peer tree layouts, measured read-only

**NOTHING was executed or mutated inside `/Users/seandonahoe/dev/resources/`.** `find`,
`grep`, `head` only.

### grok-build — SpaceXAI's `grok` terminal coding agent (Rust)

Source repo, not an install. No `.grok/` dir in the tree. 6 `SKILL.md`, all under
`crates/codegen/xai-grok-shell/skills/` — those are the agent's **built-in bundled**
skills, not user content. Need to find the product's real user-config path from its source.

### gemini-cli — Google's `gemini` CLI (TypeScript)

Has a real, canonical `.gemini/` config directory checked into the repo root:
```
.gemini/settings.json
.gemini/config.yaml
.gemini/commands/*.toml          (13, incl. nested github/ and oncall/)
.gemini/skills/<name>/SKILL.md   (13 at repo root)
```
24 `SKILL.md` total tree-wide; the other 11 are vendored test-data / builtin package
content, not user skills. `.gemini/skills/` carries helper scripts:
`async-pr-review/scripts/*.sh`, `ci/scripts/ci.mjs`, `pr-address-comments/scripts/*.js`
— i.e. the F26-SC2-M1 helper-carrying class is present here and must be exec-bit stripped.

## Open at this point

- Read `hermes.rs` + `openclaw.rs` in full; match structure exactly.
- Determine each new peer's REAL user-config root (not the source repo layout).
- Decide honestly whether `grok-build` has an importable user surface at all.
