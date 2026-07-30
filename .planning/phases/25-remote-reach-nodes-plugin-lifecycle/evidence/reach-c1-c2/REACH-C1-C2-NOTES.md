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

---

## T+25 — all four questions answered. Every one refutes the brief.

### Q2 — C2's SSH trust is LIVE, measured today, both directions

```
=== MAC -> hetzner ===            Ubuntu-2404-noble-amd64-base   rc=0
=== MAC -> seandesktop ===        SeanDesktop                    rc=0
=== hetzner -> seandesktop ===    SeanDesktop                    INNER_RC=0     <-- C2's blocker
```

Known-negative controls, same transport, same invocation shape — the instrument can return
failure, so the three zeros above are not free:

```
hetzner -> seandesktop-does-not-exist   Could not resolve hostname       NEG_RC=255
hetzner -> seandonahoe@seandesktop      Permission denied (publickey…)   NEG2_RC=255
```

So the ledger's *"no SSH trust relationship exists between `hetzner-dsm` and
`SeanD@seandesktop`, and creating one is reserved to Sean"* is **FALSE, and was already known
false**: `lane/25-hosts` measured the identical result on 2026-07-28 and ran the whole node
corpus across the two physical hosts.

### Q1 — the C1 credential is NOT missing and was never Sean-blocked after 2026-07-28

`/root/.wayland-f25-cloud.env` on `hetzner-dsm`: `-rw------- root root 716 Jul 28 03:36`.
Names present (values never read): `WAYLAND_F25_CLOUD_TOKEN` → 1, `WAYLAND_F25_CLOUD_ORG` → 1.
Known-negative in the same sweep: `ZZZ_NOT_A_REAL_VAR` → 0, so the counter can return zero.

Neither `~/.config/gcloud`, nor `~/.azure/login/`, nor `GOOGLE_API_KEY` is relevant to this
backend — it is Fly.io Machines. `GOOGLE_API_KEY` **is** set on this Mac; it satisfies nothing here.

### Q3/Q4 — prior lanes closed the gaps AND their code is at my base

The verdict `25-PHASE-VERDICT.md` (2026-07-29, lane `grade-25`) graded C1 PARTIAL / C4 PARTIAL
and costed the gaps as **G1–G6, every one marked "Credential? NO"**. Three later lanes closed them:

| gap | closed by | where |
|---|---|---|
| G1 cloud cancellation, G2 ssh cleanup leak, G3 four surfaces one commit | `lane/25-c1-cleanup` @ `05a493a2` | `25-C1-SUMMARY.md` |
| G4 egress DENY (Linux), G5 false Windows ledger line, G6 key identity | `lane/25-c4-egress` | `25-C4-SUMMARY.md` |
| G4 egress DENY (Windows) | `lane/25-c4-windows` @ `fa16cb53` | `25-C4-WINDOWS-SUMMARY.md` |

**Code presence at my base `0d48b551`** (`/usr/bin/grep`, counts redirected to a file and read
with the Read tool per §3b), each with a known-positive and a known-negative in the same sweep:

| marker | file | count |
|---|---|---|
| `wait "$child" \|\| status=$?` (G2 fix) | `backends/ssh.rs` | **1** |
| `posix_quote` (hosts injection fix) | `backends/ssh.rs` | **13** |
| `cancel_marker_taken` (G1 receipt arm) | `backends/cloud.rs` | **3** |
| `arm_egress_policy` (G4 fix) | `wcore-cli/src/backend.rs` | **2** |
| `MissingApiKey` (C4-win HIGH regression fix) | `wcore-cli/src/backend.rs` | **2** |
| `against-backend` (G6) | `wcore-cli/src/backend.rs` | **9** |
| KNOWN-POSITIVE `REMOTE_RUNNER` | `backends/ssh.rs` | 9 |
| KNOWN-NEGATIVE `ZZZ_NOT_REAL` | `backends/ssh.rs` | **0** |
| tests `ssh_far_end_quoting.rs`, `ssh_remote_runner_cleanup.rs` | present | HAVE / HAVE |

**Every fix is on integration.** So the work is done and merged; only the ledger disagrees.

## Revised plan

Re-running the legs from scratch would add nothing. What is genuinely missing is that **no
lane has re-measured any of this at the current integration head** — each proved its own tip
(`05a493a2`, `fa16cb53`, `2da46485`) and the merge train has moved repeatedly since. So:

1. Build `wayland-core` at `0d48b551` on hetzner (own `CARGO_TARGET_DIR`, `CARGO_BUILD_JOBS=10`).
2. C1 — drive all four surfaces at `0d48b551` in one invocation; assert `EQUIVALENT`.
3. C4 — cloud + ssh orphan counts at `0d48b551`, **both directions** each.
4. Correct the `REACH-*` ledger row; report unrun cells rather than skipping them silently.

