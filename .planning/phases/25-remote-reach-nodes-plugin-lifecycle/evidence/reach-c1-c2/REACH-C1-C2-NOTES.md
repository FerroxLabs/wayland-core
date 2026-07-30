# lane/reach-c1-c2 — NOTES (append-only, committed continuously)

Lane: `reach-c1-c2`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-reach-c1-c2`,
base `0d48b5515b816e8930456fcda7c91c0ec9a46ebd`.

Brief: close `REACH-*` C1 (cloud), C2 (second physical host), C4 (SSH + cloud orphan counts),
which the COMPETITIVE-LEDGER row records as blocked on Sean.

---

## T+0 — the brief's premise is ALREADY REFUTED ON DISK, in a direction the brief did not anticipate

The brief says C1/C2/C4 are open and asks me to run the legs. **Two prior lanes already ran
them, on 2026-07-28**, and `25-PHASE-STATUS.md` in this very tree records all four criteria MET:

| Criterion | 25-PHASE-STATUS.md grade | Closed by |
|---|---|---|
| 1 — local/container/ssh/cloud | **MET** | `lane/25-cloud`, 2026-07-28 |
| 2 — nodes on a second host | **MET**, one limitation | `lane/25-hosts`, 2026-07-28 |
| 3 — twelve plugin verbs | MET Linux / PARTIAL Windows | `lane/25`, 2026-07-27 |
| 4 — fail closed, no orphans | **MET** | `lane/25-hosts`, 2026-07-28 |

> "**Nothing in Phase 25 is now waiting on Sean.**" — `25-PHASE-STATUS.md`, head of file.

So the **COMPETITIVE-LEDGER `REACH-*` cell is STALE**, exactly as the `PORT-*` cell was found
stale by `lane/port-import` on 2026-07-30. Its cited source is `Phase 25, 2026-07-28` and it was
never refreshed after `lane/25-cloud` and `lane/25-hosts` landed the same day.

**This changes my deliverable.** Re-running legs that already ran adds nothing. The valuable
work is **independent re-verification** — the ledger's own admission rule requires evidence not
taken from the building lane's own summary — plus a ledger correction.

## T+0 — the brief's C1 credential guess is WRONG, and precisely so

The brief speculated the cloud leg needs `~/.config/gcloud`, `~/.azure/login/` or `GOOGLE_API_KEY`.

**The cloud backend is Fly.io Machines.** `crates/wcore-exec-backend/src/backends/cloud.rs:1`
— *"The HIBERNATING CLOUD reference backend — Fly.io Machines."* `API_BASE =
"https://api.machines.dev/v1"` (`:56`). The credential is a Fly API token read from
**`WAYLAND_F25_CLOUD_TOKEN`** (`:43`), scoped to a Fly app slug in `WAYLAND_F25_CLOUD_ORG` (`:52`).

None of gcloud, Azure or `GOOGLE_API_KEY` can satisfy this backend. Vendor choice was fixed by a
recorded four-way panel (`25-01-CLOUD-BACKEND-DECISION.md`, all four returned `fly-machines`).

## Open questions at T+0

1. Is `WAYLAND_F25_CLOUD_TOKEN` present anywhere reachable, or did Sean's minted token expire /
   live only in `lane/25-cloud`'s session?
2. Does hetzner→seandesktop SSH trust still hold today (brief says yes; 25-PHASE-STATUS says yes
   at 2026-07-28; **re-measure**)?
3. Are the C4 SSH + cloud orphan numbers reproducible, and do their gates run in BOTH directions?
4. Are `lane/25-cloud` / `lane/25-hosts` **code** fixes present at my base, or only their docs?
