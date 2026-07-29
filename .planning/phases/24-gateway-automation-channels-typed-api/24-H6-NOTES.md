# 24-H6 NOTES — running log (committed at T+0, before any measurement)

**Fixing:** F24-C3-H6 — matrix `/sync` cursor is process-local (`sync.rs:190`) and the
initial sync's timeline is discarded (`sync.rs:217`), so every message delivered while the
process is down is silently lost on restart.

**Known (inherited, NOT re-derived):** mechanism proven 3/3 with three controls in
`24-MATRIX-SIGNAL.md`. Reference implementation to follow is `imap.rs:120` +
`uid_store.rs` in `wcore-channel-email` — persist a resume position keyed per account.

**Need:** persist the `/sync` cursor across restarts, keyed (homeserver × user_id ×
channel name); prove four things — gap message arrives after restart, no duplicate on
restart, corrupt/missing cursor degrades safely and says so, steady state unaffected
(counted). Plus: prove the gate reddens on a real loss before trusting a pass. Use
`wcore_types::process_liveness`, never a hand-rolled liveness check.
