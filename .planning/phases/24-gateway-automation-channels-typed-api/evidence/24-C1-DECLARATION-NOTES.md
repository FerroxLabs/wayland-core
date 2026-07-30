# 24-C1 declaration — working NOTES

Lane `lane/24c1-declaration`. Base `0d48b551`. Started 2026-07-30.

Goal: write the per-adapter delivery-semantics declaration in `docs/`, from measurement,
and make it enforceable with a drift test.

## M1 — `supports_outbound_idempotency` overrides at base (`/usr/bin/grep -rn crates/`)

Capture: 22 hits, rc=0. Overrides of the trait method (`fn supports_outbound_idempotency(&self) -> bool`
NOT on the trait itself):

| Crate | file:line |
|---|---|
| `wcore-channel-slack` | `src/lib.rs:249` |
| `wcore-channel-matrix` | `src/lib.rs:294` |
| `wcore-channel-discord` | `src/lib.rs:344` |

Trait default `false` at `crates/wcore-channels/src/lib.rs:139`.

**Brief's "exactly-once is 3 of 10" — HOLDS at base.** Matrix `:294` and Discord `:344` are
byte-exact against the brief. Instrument alive: the same grep returned 22 hits including known
consumers (`gateway.rs:956`, `manager.rs:716`, `cron.rs:177`).

## M2 — the adapter population is 10

`ls crates/ | grep channel` → 10 adapter crates + `wcore-channels` (trait) + `wcore-channels-registry`
(factory). iMessage is `#[cfg(target_os = "macos")]`-gated in the registry (`lib.rs:60-61`), so on
Linux/Windows it is not constructible at all — that is a *row in the table*, not an omission.

## M3 — construction path for a drift test

`wcore-channels-registry` is the only crate depending on all ten (`Cargo.toml`). Its
`channel_factory_for(platform) -> Option<ChannelFactory>` (`lib.rs:46`) is the production
construction path and every `make_*` is pure (`parse_options` + `::new`) — no network, no
credential read at construction. So a drift test can build all ten from hermetic fixture configs.

## Still to establish

- [ ] What the gateway actually DOES on `supports == false` (`gateway.rs:956` consumer) — abandon vs retry.
- [ ] Whether Matrix/Discord's overrides are restart-stable (brief says both were fixed).
- [ ] Per-platform primitive for the other 7 — cite code, not the ledger.
- [ ] F24-GWP-H1: Windows duplicate burst at the Task Scheduler `PT1M` boundary. Bears on every row.
- [ ] Is any `false` adapter actually fixable? (Brief's judgement to test.)
- [ ] Drift test, both directions.
