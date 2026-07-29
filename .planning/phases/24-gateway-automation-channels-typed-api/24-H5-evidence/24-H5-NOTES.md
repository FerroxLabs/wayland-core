# 24-H5-NOTES — running log (committed at T+~12 min, appended after every measurement)

Lane `F24-C3-H5`. Branch `lane/24-h5`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-h5`, base
`d34b2fe119916b7e35ad47d28783634955d75664` (`plan/f20-unified-audit-repair` at fetch time).
Hetzner worktree: reusing `/root/wayland-24-c3-finish` with its warm `target/` as instructed.

## T+0 — the brief, restated so a resume does not have to re-read it

`channel reload` registers a new adapter; `channel health` says `healthy`; the webhook returns
`200`; every inbound message to it is **silently denied**. The identical config from the
identical generator is **admitted at startup** and **denied via reload** — one variable, already
controlled by lane `24-c3-finish` (`24-C3-FINISH.md` §3).

Fail-closed is the correct posture and is NOT the defect. The defect is that three surfaces
simultaneously tell the operator the channel works.

**Two facets, and fixing only the first is worse than the bug** (it stops being fail-closed):

1. the access **policy** map, and
2. the tool **postures**.

## T+10 — call sites, read at base, not re-derived from the finding

Confirmed every one of the six the finding lane handed over, plus the two extra call sites the
repair has to keep in step:

| file:line | what is there |
|---|---|
| `crates/wcore-cli/src/gateway.rs:1101-1157` | the reload block. Rebuilds the **adapter** set into the shared manager (`guard.reload(desired)`), recounts `registered_n`, clears `registration_error`. **Never touches `inbound_host`.** |
| `crates/wcore-cli/src/gateway.rs:888-941` | `channel_inbound_host::spawn` — called **once**, at startup. Result bound to `inbound_host`, used again only at `:1208` (shutdown). |
| `crates/wcore-agent/src/channel_inbound_host.rs:151-152` | `load_channel_policy_configs()` read **once**; `policies_loaded` fixed for the process lifetime. |
| `crates/wcore-agent/src/channel_inbound_host.rs:154-171` | **facet 2** — `postures` built here, moved into `ChannelTurnDispatcher::new` at `:198-204`. |
| `crates/wcore-agent/src/channel_inbound.rs:130` | `policies: HashMap<String, InboundPolicy>` — a plain owned map, no shared handle. |
| `crates/wcore-agent/src/channel_inbound.rs:214` | `let policies = self.policies;` — **moved into the spawned task**. Nothing outside can reach it after this line. |
| `crates/wcore-agent/src/channel_inbound.rs:249-252` | `policies.get(&name).cloned().unwrap_or_default()`. |
| `crates/wcore-channels/src/dispatch/access.rs` (`InboundPolicy::default`) | fail-closed: `dm: Allowlist` over an EMPTY allowlist ⇒ permits nothing. |
| `crates/wcore-agent/src/channel_dispatch.rs:75` + `:138-146` | facet 2's consumer — `postures` map, and `scope_for` falling back to `Conversational` rooted at `cwd`. |
| `crates/wcore-agent/src/bootstrap.rs:3189-3260` | the **other** construction site of the same two maps (`AgentBootstrap`). Must be migrated in the same change or the two hosts diverge. |

`load_channel_policy_configs` (`bootstrap.rs:311-315`) reads
`wcore_channels_registry::channels_dir()` — the SAME directory `auto_register_from_dir` is given
in the gateway reload block. So a reload-time re-read is guaranteed to see the same channel set
the adapters were rebuilt from. That is F24-C3-H1's invariant and the repair must not break it.

## T+12 — design chosen, and WHY it structurally forecloses the half-fix

The trap named in the brief is that facet 1 alone passes a naive re-run. The design answer is to
make the two facets **one object**, so there is no code path that can refresh one without the
other:

`crates/wcore-agent/src/channel_policy.rs` — `ChannelPolicyRegistry`, a single
`std::sync::RwLock<Snapshot>` where `Snapshot { policies, postures, generation }`. Both the
subscriber and the dispatcher hold the same `Arc`. `replace()` swaps **both maps under one write
lock and bumps one generation counter**. There is no `replace_policies_only`.

`std::sync::RwLock`, not `tokio`'s: both read sites (`channel_inbound.rs:249`,
`channel_dispatch.rs:138`) are bounded map lookups that clone out and are never held across an
`await` — the same rule the file already applies to `Arc<StdMutex<AutoReplyRateLimiter>>`
(`channel_inbound.rs:134-141`).

`InboundHost` then exposes the `Arc` plus `reload_policies()` (re-reads from disk, replaces,
returns the new count), and `gateway.rs`'s reload block calls it.

## T+12 — acceptance shape (what I must NOT let myself get away with)

- **Arrivals must be counted positively.** This defect IS universal denial, so any leg that only
  asserts "denied == expected" passes on the broken build. Every zero gets a live positive
  control in the same run.
- **The posture must be asserted, not just arrival.** Reloaded posture must EQUAL startup posture
  for the same config. Arrival alone is the half-fix's green.
- **The instrument must be shown able to see a denial** before any "no denials" claim.
- Byte-count every capture; `${PIPESTATUS[0]}` returns empty here.

## Status ledger (append-only)

- [x] T+0 read `LANE-BRIEF.md`, `24-C3-FINISH.md`
- [x] T+10 all call sites verified at base
- [x] T+12 design fixed, NOTES committed
- [ ] registry + both hosts migrated
- [ ] gateway reload calls it
- [ ] unit tests (in-crate) incl. posture-equality and the old-shape-would-have-missed-it assertion
- [ ] hetzner build + targeted tests
- [ ] live driver run: reload leg green, posture asserted, denial still denied
- [ ] SUMMARY
