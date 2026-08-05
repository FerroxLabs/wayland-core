# Per-hunk ablation — is each hunk actually load-bearing?

Run at `6db3ed7f` on `hetzner-dsm`, worktree `/root/wayland-authprod`. Each
hunk reverted **individually, with the tests left untouched**, by exact-string
replacement that **asserts exactly one occurrence and aborts otherwise** — a
silently-missed revert cannot masquerade as a green. Script:
`ablate.py`. Raw results: `ablation.json`.

## Result

| run | wcore-channel-discord | wcore-channel-slack | test that reddened |
|---|---|---|---|
| **baseline** | 66 passed / 0 failed | 54 passed / 0 failed | — |
| revert **H1** discord close-frame capture | **65 / 1 FAILED** | 54 / 0 | `a_4004_close_publishes_auth_expired_and_stops_the_gateway` |
| revert **H2** discord `AuthExpired` publish | **65 / 1 FAILED** | 54 / 0 | `a_4004_close_publishes_auth_expired_and_stops_the_gateway` |
| revert **H3** slack `auth.test` at `start()` | 66 / 0 | **50 / 4 FAILED** | `a_rejected_bot_token_publishes_auth_expired_and_never_connects`, `an_accepted_bot_token_connects_and_publishes_no_auth_expired`, `an_unreachable_auth_test_is_not_treated_as_a_rejection`, `a_token_revoked_after_start_publishes_auth_expired_on_send` |
| revert **H4** slack send-path `AuthExpired` | 66 / 0 | **53 / 1 FAILED** | `a_token_revoked_after_start_publishes_auth_expired_on_send` |
| **restore control** | 66 / 0 | 54 / 0 | — (matches baseline exactly) |

**All four hunks are load-bearing.** No hunk reverted to a green suite.

The **control held in every run**: the crate that was *not* ablated always
matched its baseline exactly. That is what makes the reds meaningful rather
than a suite that simply collapses under any edit.

## Honest qualification on H1 vs H2

H1 and H2 redden **the same single test**, because they are two halves of one
path — read the close code, then publish the reason before exiting. The suite
cannot distinguish them, and I am not going to claim it can. What the table
does establish is that **each half is necessary**: removing either one loses
the `AuthExpired`.

Worth noting for the record: reverting H1 leaves the classifier unit test
`close_4004_is_classified_as_a_credential_rejection` **still passing**, because
H1 reverts the *call site*, not the classifier. A suite consisting only of the
`gateway.rs` unit tests would have gone fully green with the wiring severed.
That is precisely why `tests/gateway_auth_close.rs` was added, and this
ablation is the evidence it was needed rather than decorative.

## Instrument defect found and REPAIRED in this harness

The first ablation run produced a **false result**: a `wcore-channel-discord`
test failed under `H3`, a **Slack-only** ablation.

Cause: backups were taken with `shutil.copy`, which does not preserve mtime, so
restoring one set the source mtime **backwards**. Cargo fingerprints on mtime,
concluded the artifact was current, and skipped the rebuild — so the next run
executed the **previous ablation's binary** while `git status --porcelain`
reported a completely clean tree.

Measured directly:

```
gateway.rs mtime:   2026-07-30 14:46:34
test binary mtime:  2026-07-30 14:46:35     <- newer than its own source
grep for the fix in gateway.rs: 1           <- the fix IS present on disk
cargo test  -> FAILED (got [ConnectionStateChanged { Connected }], no AuthExpired)
touch gateway.rs && cargo test -> ok. 2 passed
```

The failing assertion was *exactly* the H2-ablated symptom, on a tree that git
called clean. Two repairs, both in this lane rather than written up and left:

1. `os.utime(path, None)` after **every** write and **every** restore, so the
   source is always newer than any artifact built from it.
2. A **restore control** — the baseline suites re-run at the end, asserted to
   match the opening baseline exactly. That check is what would have caught
   this immediately, and it now guards every future run of this script.

The table above is from the run **after** both repairs, and its restore control
passes.
