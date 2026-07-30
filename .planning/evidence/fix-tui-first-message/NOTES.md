# NOTES — lane/fix-tui-first-message

Base: `bc90ee1c1f08b76e6682b4beab2386fc7216a52e` (integration `plan/f20-unified-audit-repair`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-tui-first-message`.

## T+0 — brief premise verification

Brief claims two defects behind UAT-TUI-UNIX F1. Both located in source at base SHA.

### (a) State detection — the first-run gate keys on a FILE, not on credentials

`crates/wcore-cli/src/main.rs:2447`

```rust
let first_run = !config::global_config_path().exists();
```

Passed into `TuiSession.first_run`; consumed at `crates/wcore-cli/src/tui/mod.rs:475`:

```rust
let initial_surface = match session.as_ref() {
    Some(s) if s.force_onboarding => SurfaceId::Onboarding,
    Some(s) if !s.first_run       => SurfaceId::Workspace,
    _                             => SurfaceId::Onboarding,
};
```

Line 2447 sits on the **success** path of `Config::resolve_with_provenance` — i.e. a provider
and a credential already resolved. The UAT ran with a pristine `--home`, so
`~/.wayland/config.toml` did not exist, so `first_run == true` and Onboarding opened **despite**
`FLUX_API_KEY` being exported and `-p flux-router -m flux-auto` being on argv. Brief claim (a)
CONFIRMED at base.

Note there is a *second*, richer gate at `main.rs:1854-1860` on the resolve-FAILURE path which
does consider `missing_credentials`. That one is correct; it is not the one that fires here.

### (b) Input loss — the Connect step's bare-letter accelerators eat prose

`crates/wcore-cli/src/tui/surfaces/onboarding.rs:538 handle_connect_key`, not editing a field:

- `1`-`9` → connect env key N
- `a`     → connect all env keys (when >= 2)
- `k`/`j` → move cursor
- `o`     → Ollama path
- **`s`   → `finish_non_key(Path::Skip)` → `self.step = Step::Ready`** (line 572-575, 468-479)
- Enter   → activate selected path
- `_`     → `SurfaceAction::None` — **character silently discarded**

`handle_ready_key` (line 803), no config conflict:

- **`Enter` | `Char(' ')` → `SurfaceAction::Switch(SurfaceId::Workspace)`** (line 839)
- `_` → discarded

So the sequence that dismisses the modal from prose is: **first lowercase `s`, then the next
space.** Everything up to and including that space is destroyed; the remainder lands in the
composer.

This predicts the UAT's loss counts EXACTLY — all five rows, including the two 100% losses:

| UAT sent | predicted lost prefix | predicted len | UAT observed |
|---|---|---|---|
| `Use the bash tool to run echo SLOWTYPE_TOKEN` | `Use ` | 4 | 4 |
| `What is 17 times 23? Reply with just the number.` | `What is ` | 8 | 8 |
| `MARKERSTART_what is two plus two_MARKEREND` | `MARKERSTART_what is ` | 20 | 20 |
| `Use the bash tool to run exactly: echo HELLO_FROM_UAT` | `Use ` | 4 | 4 |
| `/quit` | no lowercase `s` → never dismisses | 100% | 100% |
| `ABCDEFGH_THIS_IS_THE_START_OF_MY_TYPED_LINE_1234567890` | uppercase `S` only, `Char('s')` is case-sensitive → never dismisses | 100% | 100% |

6/6 exact. The `MARKERSTART_` row is the discriminating one: its uppercase `S` at index 6 does
NOT fire, the lowercase `s` of `is` at index 18 does, and 18+1(space)+1 = 20. A theory that
keyed on any `s` would have predicted 7, not 20.

This is a **derivation, not yet a measurement.** It must be reproduced against a running binary
before any fix is claimed.

## Plan

1. Repro at human speed (7.1 c/s) on hetzner using `.planning/evidence/uat-tui-unix/slow-type.sh`,
   at base SHA, BEFORE any fix. Both credential quadrants.
2. Fix (a): gate onboarding on resolved credentials, not on file existence.
3. Fix (b): type-ahead buffer on the onboarding surface, flushed into the composer on handoff.
4. Re-measure all four quadrants.

## Open questions

- Does workspace `handle_paste` route a flushed buffer through the paste-detect modal? Must check.
- Quadrant 2 (credentials absent → modal appears) is served by the `main.rs:1858` MissingApiKey
  path, not by line 2447. Must prove that path still fires after fix (a).
