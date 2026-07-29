---
lane: 26-sc2-peers
criterion: "SC2 — peer coverage: importers for grok-build and gemini-cli"
was: "peer coverage 2 of 4 (26-SC2-SUMMARY.md §5, §6) — RE-VERIFIED TRUE at HEAD"
now: "peer coverage 4 of 4; both new peers driven end to end against REAL installed homes"
base: "5be910561f688c75d39492e7b982d6e100772a64 (gh/plan/f20-unified-audit-repair)"
branch: lane/26-sc2-peers
new-finding: "F26-SC2P-L1 (LOW, fixed in-lane) — peer_version probed VERSION/version/MANIFEST.json and returned None against the real ~/.grok, which declares \"version\": \"0.2.103\" in version.json. Found only by driving the real tree."
mitigation-carried: "YES, by construction and PROVEN on disk — write_tree is peer-agnostic; 3/3 grok and 4/4 real exec bits removed, 755 -> 644, measured with stat"
fence-exposure: "ZERO — 0 diff lines in crates/wcore-cli/src/lib.rs and main.rs, no .github/, no wcore-contract generate"
peer-trees-mutated: "NONE — 0 working-tree changes across all four; SOURCE-INTEGRITY digest PASS; nothing executed inside any of them"
status: complete
---

# 26-SC2-PEERS — the two missing importers, proven on real installs

Every number below came from an unproxied tool (`/usr/bin/git`, `/usr/bin/grep`,
`/usr/bin/find`, `/usr/bin/stat`, `/root/.cargo/bin/cargo` over a direct `ssh`)
or from a run of the real release binary.

---

## 0. The brief's premise, re-verified before building — "2 of 4" HELD

```
$ /usr/bin/grep -rniE "grok|gemini"     crates/wcore-cli/src/migrate/   → 0     (the claim)
$ /usr/bin/grep -rniE "hermes|openclaw" crates/wcore-cli/src/migrate/   → 150   (known-positive, same matcher)
$ crates/wcore-config/src/portability/mod.rs:45 enum PeerSource         → Hermes, OpenClaw
```

The known-positive licenses the zero: the same matcher on the same tree returns 150,
so the instrument was alive. **The brief was right and I did not manufacture work
around a false premise.**

One incidental drift, recorded so nobody re-derives it: `26-SC2-SUMMARY.md` §6 says
*"`migrate` still has no rollback (G3, untouched here)"*. At my base
`crates/wcore-cli/src/migrate/rollback.rs` exists (468 lines) — lane `26-sc3-rollback`
landed in between. Not my scope.

---

## 1. What the peers actually look like — grounded in each peer's OWN source

`openclaw.rs` set the discipline: read the peer's path constants, not its directory
listing. Following it mattered here, because **the `.gemini/` directory checked into
the gemini-cli repo is a PROJECT config, not a user home** — an importer written off
that listing would have targeted the wrong tree.

### grok-build → the `grok` binary (SpaceXAI, Rust)

| what | value | grounded at |
|---|---|---|
| home | `$GROK_HOME`, else `~/.grok` | `xai-grok-config/src/paths.rs:28-47` |
| config | `<home>/config.toml` | `xai-grok-config/src/lib.rs:4-6` |
| model | `[models] default` | `xai-grok-shell/src/agent/config.rs:975-977` |
| MCP | `[mcp_servers.<name>]` + `enabled` | `xai-grok-shell/src/util/config/mcp.rs:419,1185` |
| credential | `<home>/auth.json` (OIDC session) | `xai-grok-shell/src/auth/flow.rs:1951`, resolution order at `trace_classifier/mod.rs:1043-1057` |
| user skills | `<home>/skills/<n>/SKILL.md` | `xai-grok-shell/src/builtin.rs:75,134,150` |
| vendor skills | `<home>/bundled`, `<home>/server-skills` | `xai-grok-shell/src/inspect/mod.rs:1820,1828` |
| personas | `<home>/personas/*.toml` | `xai-grok-shell/src/config/mod.rs:279-315,385-392` |

### gemini-cli → the `gemini` binary (Google, TypeScript)

| what | value | grounded at |
|---|---|---|
| home | `<$GEMINI_CLI_HOME or $HOME>/.gemini` | `core/src/utils/paths.ts:13,22-28` + `core/src/config/storage.ts:54-60` |
| settings | `<home>/settings.json` | `core/src/config/storage.ts:78-80` |
| model | `model.name` | `cli/src/config/settingsSchema.ts:1062-1079` |
| MCP | root `mcpServers` | `cli/src/config/settingsSchema.ts:161-174`; shape at `core/src/config/config.ts:478-514` |
| credential | `<home>/oauth_creds.json` | `core/src/config/storage.ts:22` |
| skills | `<home>/skills/` | `core/src/config/storage.ts:101-103` |
| memory | `<home>/GEMINI.md` | `core/src/tools/memoryTool.ts:11` |

Two mapping choices are grounded, not invented: `xai` and `gemini` are both **builtin
wayland-core providers** (`crates/wcore-config/src/config.rs:2529,2545`). Where a peer
declares no model, **no provider is invented** — a profile pinned to `xai` with no model
is a claim the peer never made, and a unit test asserts the `None`.

---

## 2. What did NOT need building, checked rather than assumed

The expensive half was already there, and saying so is more useful than adding
redundant code that reads as work:

- **`peer_skill_roots()` needed no new root.** `home.join("skills")` is already its
  first entry, and it is the user skills root for *both* new peers. So both inherit
  the existing recursion, symlink refusal, `MAX_SKILL_ROOT_DEPTH` bound and dedupe.
  Verified by assertion, not by reading: `the_vendor_catalogs_are_counted_and_never_become_skills`
  drives the real walker and requires `skill:skills/mine` present and
  `bundled`/`server-skills` absent.
- **`write_tree()` needed no change.** It is peer-agnostic, so the F26-SC2-M1 exec-bit
  mitigation reaches a new peer by construction. **Asserted on disk anyway** (§4) —
  "it should carry" is exactly the claim this programme keeps finding false.
- **`scan_peer_memory()` already covered grok's `memory/`.** Two genuine gaps were
  additive: gemini's memory is a single root file (`GEMINI.md`), and grok's personas
  are `personas/*.toml` files rather than `SOUL.md`.

### Deliberate exclusions

`<grok home>/bundled/` (vendor catalog) and `server-skills/` (server-pushed) are **not**
skill roots — the same argument F26-GRADE-M1 made for Hermes's `hermes-agent/optional-skills/`.
They are counted in `deferred_other` and named to the operator. Importing them would
inflate an "imported" count without migrating anything the user authored.

---

## 3. Both peers are REALLY INSTALLED on this Mac — so the proof used real homes

The brief pointed at `~/dev/resources/{grok-build,gemini-cli}`, which are the products'
**source repositories**. Sean also has both products installed, and those are the trees
a migration actually faces. Facts measured before running anything:

| | `~/.grok` | `~/.gemini` |
|---|---|---|
| model declared | **NO** (`[cli] [marketplace] [ui]` only) | **NO** (no `model` block) |
| MCP servers | 0 | **14** |
| credential store | `auth.json` | `google_accounts.json`, `mcp-oauth-tokens.json`, `installation_id`; **no `oauth_creds.json`** |
| version | `version.json` → `0.2.103` | `package.json` = `{"type":"commonjs"}`, none |
| user skills | 5, **all 0644** | none |

**That produced a real finding.** `peer_version` probed `VERSION`/`version`/`MANIFEST.json`
and returned `None` against a home that plainly declares `"version": "0.2.103"` —
the honest-absence rule turning into a silently missing fact. **F26-SC2P-L1, fixed in
`facb3c7b`.** A synthetic fixture would never have shown it.

### Credential handling — disclosed in full

Neither peer's credential is a `ProfileConfig::api_key`: grok's is an OIDC session with
a refresh token, gemini's is a browser OAuth grant. Both are recorded **by name only**,
and `--include-credentials` deliberately does not promote either — the same call
`openclaw.rs` makes for a gateway token. A unit test asserts the *value* never reaches
the plan, checked against the value rather than the field name.

For the live proof, credential-store **contents were never copied**; same-named
placeholders were written so the by-reference path is exercised, and the staging script
prints every substitution. Every `mcpServers` entry of the real `settings.json` was
inspected key-by-key first (only `command`/`args`/`type`/`url`; the one `env` holds
`PATH`), so it shipped verbatim carrying nothing. Post-hoc sweep of everything this lane
committed, with a known-positive on the same regex:

```
token-shaped strings in the full lane diff : 0
same regex over the evidence logs          : 0, 0, 0
KNOWN-POSITIVE (two synthetic tokens)      : 2      ← the matcher fires
```

---

## 4. Live proof — the real binary, real homes, `wayland-core 0.12.25` on hetzner

`scripts/f26-sc2-peers-live-proof.sh`, `LIVE-RC=0`, 21 PASS / 0 FAIL, full transcript at
`evidence/26-sc2-peers/sc2-peers-live-proof.log`.

### What actually landed

```
grok    import rc=0   discovered=7  imported=7  quarantined=0  excluded=0
        1 profile (grok/root), 0 MCP, 0 credentials
        23 files written — 6 skills, 0 personas, 0 memory notes
        deferred + named: 4 bundled, 6 marketplace-cache, 26 sessions
        "3 imported files carried an execute bit; it was REMOVED."

gemini  import rc=0   discovered=29 imported=20 quarantined=9  excluded=0
        1 profile (gemini/root), 14 MCP servers found → 5 imported, 9 QUARANTINED
        33 files written — 13 skills, 0 personas, 1 memory note (GEMINI.md)
        deferred + named: 1 commands, 3 credential_files, 1 extensions
        warned: gemini authenticates with "gemini-api-key"; wayland-core does not
                consume that grant, so authentication must be re-established here
        "4 imported files carried an execute bit; it was REMOVED."
```

The gemini result is the stronger one and I did not engineer it: **the existing
containment machinery fires on a real peer.** 9 of 14 real MCP servers carry a launch
command and were quarantined inert; the 5 URL-only ones imported. Both accountings
balance (`imported + quarantined + excluded == discovered`).

### The hostile case, per peer, measured ON DISK

| | grok | gemini |
|---|---|---|
| payload | `skills/brand/scripts/*.cjs` | `skills/async-pr-review/scripts/*.sh`, `ci/scripts/ci.mjs`, `pr-address-comments/scripts/fetch-pr-info.js` |
| provenance of payload | real marketplace skill from `~/.grok/marketplace-cache`, placed where an install puts it | the gemini-cli project's own `.gemini/skills/`, the identical layout `getUserSkillsDir()` returns |
| source mode | **755** | **755** |
| landed mode | **644** | **644** |
| sha256[0:16] source → landed | `36d21f39…` → `36d21f39…` | `bdfc4bb7…` → `bdfc4bb7…` |
| exec bits surviving under the live root | **0 of 3** | **0 of 4** |

Every one of those is a `stat` on the imported file, not an assertion from the code path.
`X1` proves the **bytes crossed unchanged** — this is a migration, not a filter — and
`X4` widens the claim from the one file the test names to **every** file under the live
root, because a mitigation that holds on the named path and leaks on the other twelve is
not a mitigation.

Each absence carries a control taken in the same invocation: `P0` proves the filesystem
honours the exec bit and `stat` reports it **both** ways (755 *and* 644) before any "not
executable" is claimed; `H0-CONTROL` proves the source was 755, so `X2` measures a
change; `S1` proves the source is byte- and mode-identical afterwards.

**`grok`'s `~/.grok/skills/` carries no exec-bit helper today (0 of 5).** Its hostile
payload is therefore a real marketplace skill's real bytes at its real install
destination, not a naturally-occurring one. Stated because the difference matters.

### F26-SC2-M1 re-measured on the two new peers — the earlier reasoning HOLDS

```
skills carrying Wayland's ```! directive — grok: 0 of 6    gemini: 0 of 13
```

`26-SC2-SUMMARY.md` §3 argued that zero real peer skills use Wayland's directive, so
essentially every real peer skill classifies `Data` and imports live — and that the
proportionate control is exec-bit removal plus operator disclosure, **not** quarantine,
because the 512-item ceiling would convert a completeness win back into mass refusal.
**Both new peers reproduce that exactly, and neither defeats the mitigation.** I did not
overturn the reasoning and found no reason to.

---

## 5. Three-assertion self-test — the known-negative genuinely fails

`scripts/f26-sc2-peers-known-negative.sh`, `KN-RC=0`, transcript at
`evidence/26-sc2-peers/sc2-peers-known-negative.log`.

```
A  known-positive     A-RC=0   21 PASS / 0 FAIL
B  known-negative     B-RC=1   17 PASS / 4 FAIL      (mutation applied: 11 changed lines; COMPILED: yes)
     FAIL: grok X2 the imported helper is STILL EXECUTABLE (755)
     FAIL: grok X4 3 execute bits survived
     FAIL: gemini X2 the imported helper is STILL EXECUTABLE (755)
     FAIL: gemini X4 4 execute bits survived
C  old shape misses it
     base 5be91056: migrate grok   -> rc=2  error: unrecognized subcommand 'grok'
     base 5be91056: migrate gemini -> rc=2  error: unrecognized subcommand 'gemini'
     C-CONTROL     : migrate quarantined -> rc=0     (so C measured ABSENCE, not breakage)
D  restored         D-RC=0   21 PASS / 0 FAIL
KNOWN-NEGATIVE SELF-TEST: PASS
```

The mutation is a **mode-preserving writer**, not a deleted guard, and that choice is
load-bearing: `fs::write` yields 0644 on a new path regardless, so deleting
`strip_execute_bits` would leave the proof green and the guard would read as
load-bearing while doing nothing. The realistic regression is a copy-based `write_tree`,
which is what B simulates. **The 4 assertions that go red are exactly the 4 that measure
the mitigation, and no others.** `C-CONTROL` is what stops C being a dead instrument —
the same base binary accepts a subcommand it *does* have. `D` is what makes B's red
attributable rather than ambient.

---

## 6. Gates, all read back with their full counts

```
cargo fmt --all -- --check                                 : FMT-RC=0, 0 lines of diff
cargo check --workspace --all-targets                      : CHECK-RC=0, 0 errors
cargo clippy -p wcore-cli -p wcore-config --all-targets -D warnings : CLIPPY-RC=0, 0 errors
cargo test -p wcore-cli --test migrate_quarantine
  test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
cargo test -p wcore-cli --lib migrate::
  test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 1835 filtered out
cargo test -p wcore-config --lib portability::
  test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 557 filtered out
```

12 of the 58 are the new peers' own tests, listed by name in the run — the executed
count was read back rather than inferred, because a `--lib migrate::` filter that
matched nothing would exit 0 having run zero tests.

**`cargo check --workspace --all-targets` was run deliberately**, against LANE-BRIEF §2's
targeted-build guidance, and it earned its keep: this change adds two variants to
`PeerSource`, and a per-crate check cannot see a downstream exhaustive `match`. It is a
*check*, not a build, so it is the cheap instrument for exactly this risk. The first run
found one real error (a `super::quarantine` path from inside a test module); the second
was clean.

### Pre-existing, out of scope, named not fixed

- `warning: function record_overwrites_so_the_surface_describes_the_LATEST_turn should
  have a snake case name` — `crates/wcore-memory/src/activation.rs:198`. A file this lane
  never touched.
- `imap-proto v0.10.2` future-incompat — a dependency, already named as pre-existing at
  base by `26-SC2-SUMMARY.md`.

---

## 7. Honest verdict

**Peer coverage is now 4 of 4, and both new importers are proven against real installed
homes rather than fixtures.** The exec-bit mitigation carried cleanly to both peers, was
not weakened, and its claim goes red under a mutation. The provenance surface answers for
both peers with a discriminating known-negative.

**What I did NOT do, and would be wrong to imply:**

- **Personas and memory are DISCOVERED but the grok persona path is not live-proven.**
  `scan_peer_personas` now finds `personas/*.toml`, but the real `~/.grok` has no
  `personas/` directory, so the live run exercised 0 of them. Unit-covered only.
  gemini's `GEMINI.md` **was** live-proven (1 memory note staged).
- **`skill classification still does not inspect the file payload`** (F26-SC2-M1).
  Carried forward unchanged and deliberately — the ceiling design that would fix it
  properly is still not done, and I did not want to reopen a decision `26-SC2-SUMMARY.md`
  argued well.
- **grok has no naturally-occurring hostile case in its real skills root.** Its payload
  was assembled from real marketplace bytes at the real install destination. Real, but
  not found in situ.
- **Neither peer's `--json` plan path was live-exercised**, only the prose path. The
  redaction boundary it crosses is type-level and unit-asserted, but I did not run it.
- **grok's project-scoped `.grok/config.toml`** (per-repo) is out of scope; only the
  user-global home is imported. Same for gemini's per-project `.gemini/`.
- gemini's `~/.agents/skills` root (`getUserAgentSkillsDir`) sits **outside** the gemini
  home and is not scanned. Named, not fixed.
- **No settings/hooks/statusLine import for gemini, no `[ui]`/`[cli]` import for grok** —
  refused by the same argument the other two peers use, and counted.

---

## 8. Fence exposure vs the merge-base (`5be91056`)

```
$ /usr/bin/git diff 5be910561f688c75d39492e7b982d6e100772a64 HEAD \
    -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs | wc -l
0
$ /usr/bin/git diff --name-status 5be910561f688c75d39492e7b982d6e100772a64 HEAD
A  .planning/phases/26-migration-export-backup-restore/evidence/26-sc2-peers/26-SC2-PEERS-NOTES.md
A  crates/wcore-cli/src/migrate/gemini.rs
A  crates/wcore-cli/src/migrate/grok.rs
M  crates/wcore-cli/src/migrate/mod.rs
M  crates/wcore-cli/src/migrate/quarantine.rs
M  crates/wcore-config/src/portability/mod.rs
A  scripts/f26-sc2-peers-known-negative.sh
A  scripts/f26-sc2-peers-live-proof.sh
A  scripts/f26-sc2-peers-stage.sh
$ .github/ files changed: 0
```

`MigrateCmd` is declared inside `migrate/mod.rs`, so two new subcommands cost **zero**
shared-fence lines. No `wcore-contract generate`, no merge to
`plan/f20-unified-audit-repair`, no PR, no tag, no release, no issue closed.

## 9. The peer trees

**Nothing inside `/Users/seandonahoe/dev/resources/{hermes-agent,openclaw,grok-build,gemini-cli}`
was mutated or executed**, and the same care was extended to Sean's live `~/.grok` and
`~/.gemini`. Only `find`, `grep`, `stat`, `head` and `cp` (as a source) were used. The
staging script digests every source path it touches before and after and asserts equality:

```
SOURCE-INTEGRITY: PASS — every source path is byte-, mode- and mtime-identical
```

Independently, after all work completed:

```
$ git -C /Users/seandonahoe/dev/resources/<peer> status --porcelain | wc -l
grok-build: 0    gemini-cli: 0    hermes-agent: 0    openclaw: 0
```
