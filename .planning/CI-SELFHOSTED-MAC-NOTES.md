# NOTES — lane `ci-selfhosted-mac` (working log, appended as measured)

Branch `lane/ci-selfhosted-mac`, merge-base `75babf32`. Committed inside the first 15 minutes
per LANE-BRIEF §6b-i, and re-committed after every measurement.

---

## 0. Plan (written before measuring, so it can be graded against what I actually found)

Task: decide which of the three macOS jobs move to the new `sean-mac-arm64` self-hosted runner,
restore lane-branch macOS coverage to the degree the new capacity supports, prove it live with a
counterfactual, and state the hermeticity cost.

Working hypothesis to test, NOT to assume:
1. A single self-hosted runner has **lower throughput** than the 5-wide hosted pool. If true,
   "move everything" is wrong and the win is *latency at low demand*, not capacity.
2. The right split routes **lane** macOS demand to self-hosted and leaves **main / integration /
   PR** on the hosted pool.
3. The `[ci-darwin]` rationing must stay for whatever remains hosted.

Steps: (a) measure real job durations from the API; (b) compute serial arithmetic both ways;
(c) check fork-PR exposure; (d) implement; (e) live-dispatch and prove `runner_name`; (f) prove
the artifact is genuinely arm64; (g) counterfactual; (h) prove Windows untouched.

---

## 1. Facts established at start

### 1.1 The runner (`gh api repos/FerroxLabs/wayland-core/actions/runners`)

```
id 34  sean-mac-arm64  macOS  status=online  busy=false  v2.336.0
labels: self-hosted:read-only  macOS:read-only  ARM64:read-only
```

All three labels are `read-only` = **self-detected by the runner**, not hand-written. Per the
brief this is the distinction that was earned the hard way (the first install carried `ARM64` as
a `custom` label while the runtime self-reported `X64` — an x64 package under Rosetta). This
install is clean. Contrast, same call, the two Windows runners:

```
id 22  ferrox-win-msvc  Windows  online  busy=true   labels: self-hosted:ro Windows:ro X64:ro msvc:CUSTOM
id 21  SEANDESKTOP      Windows  online  busy=true   labels: self-hosted:ro Windows:ro X64:ro msvc:CUSTOM
```

`msvc` is `custom` on both — that one is a *tag*, not an architecture claim, so it is benign. The
rule I am carrying forward: **a `custom` architecture label is an unproven claim.** Neither
Windows runner asserts its arch via a custom label, so neither is suspect.

### 1.2 FINDING (NEW, HIGH) — the repository is PUBLIC, and self-hosted runners already serve `pull_request`

```
$ gh api repos/FerroxLabs/wayland-core --jq '{private,visibility,allow_forking}'
{"allow_forking":true,"private":false,"visibility":"public"}
```

`ci.yml` triggers on `pull_request: branches: [main]`, and the `ci` job's matrix already contains
`["self-hosted","Windows","X64","msvc"]`. So **fork-PR code paths already reach Sean's self-hosted
Windows boxes**, and naively adding `[self-hosted, macOS, ARM64]` to that same matrix would extend
the same exposure to his personal Mac — the machine holding his login keychain, SSH keys and every
worktree in this program.

This is a pre-existing exposure I did **not** create and will **not** silently widen. Design
consequence, adopted before writing any YAML: **the self-hosted macOS runner is used on `lane/**`
pushes only — never on `pull_request`, never on `main`.** Verifying the fork-PR approval policy
next; even with approval-gating on, "one careless Approve" is not a boundary I want in front of a
personal machine.

### 1.3 The runner is on THIS Mac, inside the forbidden checkout

```
$ pgrep -lf Runner.Listener
2263 /Users/seandonahoe/dev/waylandcore/actions-runner/bin/Runner.Listener run --startuptype service
$ uname -m -> arm64 ;  macOS 26.3 (25D125)
```

Two consequences:

- The runner's work tree lives under `/Users/seandonahoe/dev/waylandcore` — the heavily-dirty
  checkout LANE-BRIEF §0 forbids me to touch. I will not touch it; noting only that runner jobs
  land on the same volume.
- **LANE-BRIEF §0's "never run cargo on the Mac" applies in spirit here.** The rule exists because
  Mac builds are slow and were causing real problems. Routing CI to this runner does not violate
  the letter (I am not invoking cargo), but it hands the same cost to the same machine via a
  different door. That is exactly the capacity question I was asked to answer honestly, and it
  argues against moving the heavy job.

### 1.4 What the three jobs are FOR (read from ci.yml, not assumed)

| job | what it actually does | why it exists |
|---|---|---|
| `CI (macos-latest)` | fmt, clippy, **full 12.8k-test nextest**, contract check, release smoke, eval gate, `cargo audit` | the Darwin **correctness verdict** |
| `Build (aarch64-apple-darwin)` | `cargo build --release -p wcore-cli --target aarch64…` + **uploads the arm64 artifact** | the **artifact producer** a lane live-tests with; uploads regardless of test outcome (ci.yml:646-654 records the 2-1 cross-audit that kept it for exactly this reason) |
| `Build (x86_64-apple-darwin)` | same, x86_64 target, uploads Intel artifact | Intel **compile-regression** check + Intel release binary |

---

## 2. Open / next

- [ ] real durations for the three jobs from the API → serial arithmetic
- [ ] current hosted macOS queue wait (post-rationing; the 4-5h figures are pre-fix and must not
      be reused as the comparison baseline)
- [ ] fork-PR approval policy
- [ ] implement, dispatch, prove `runner_name`, prove arm64, counterfactual, Windows unaffected
