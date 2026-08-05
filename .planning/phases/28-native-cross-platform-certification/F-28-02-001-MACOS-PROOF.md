# F-28-02-001 — macOS activeness, proved by execution

The claim under test: **a black-box caller driving only the shipped macOS binary can obtain
positive evidence that the sandbox was ACTIVE for a given execution.** 28-02 measured this as
impossible and recorded 24 RED cells. It is now obtainable.

Script: `scratchpad/macos-proof.sh`. Everything below is transcript, not summary.

## 1. The binary

Downloaded from CI, not built here. No cargo ran on this Mac beyond `cargo fmt`.

| | |
|---|---|
| artifact | `wayland-core-aarch64-apple-darwin` |
| CI run | `30314578073` (`FerroxLabs/wayland-core`), job `Build (aarch64-apple-darwin)` = **success** |
| commit | `8a09297bf64f9d90293b8d482bd75d8533e40bdc` (`lane/macos-activeness`) |
| sha256 | `9432b7bf7bd222f70da46f63bb6c8298e9edcf65859982b80ccaaafd6a91d436` |
| `file` | `Mach-O 64-bit executable arm64` |
| `--version` | `wayland-core 0.12.25` |
| host | this Mac — macOS **26.3** (25D125), arm64 |

**Why this commit and not the lane tip.** The only `src/` change after `8a09297b` is inside
`#[cfg(test)]` (`git diff 8a09297b..HEAD -- crates/wcore-cli/src/` is 9 insertions, all within
`mod tests`), which is not compiled into a release binary. The shipped surface in this artifact
is therefore identical to the lane tip's. Stated rather than assumed, because "the artifact is
close enough to the source" is exactly the shortcut that makes a certification worthless.

The CI run as a whole concludes `failure` on this branch — contract-corpus drift fails before
the test step, identically at the untouched base. The `build` job is separate, succeeded, and
its artifact downloads normally. Clearing that drift is forbidden and was not attempted.

## 2. The surface exists in this binary

```
$ wayland-core sandbox status --json
{"available":true,"backend":"sandbox_exec","binds_cwd_authority":false,
 "binds_workspace_authority":false,"bypasses_containment":false,
 "enforces_read_deny":true,"owns_descendants_hard":false}
```

`owns_descendants_hard: false` is the root cause of the original finding, now visible from
outside the product: it is precisely why the delegated Swarm path refuses on this platform.

## 3. The differential

Identical probe script, run once outside the product and once through
`wayland-core sandbox exec --workspace <ws> "sh probe.sh"`.

```
===== OUTSIDE — uncontained baseline =====
F28RAN
F28_DNS=RESOLVES
F28_ESCAPE=WROTE
F28_ETC=READ
F28_ROOTLS=Applications,bin,cores,dev,etc,home,Library,opt,private,sbin,System,tmp,Users,usr,var,Volumes,
host escape marker OUTSIDE: PRESENT

===== INSIDE — through the product's own containment path =====
Exit code: 0
F28RAN
F28_DNS=NO_DNS
F28_ESCAPE=DENIED
F28_ETC=DENIED
F28_ROOTLS=Applications,bin,cores,dev,etc,home,Library,opt,private,sbin,System,tmp,Users,usr,var,Volumes,
host escape marker INSIDE: ABSENT
```

**`ACTIVENESS: observed=true`** — three independent differences:

1. **DNS** resolves outside, does not inside — the SBPL profile emits no network rule under
   `NetworkPolicy::Deny`, so deny-default denies egress.
2. **A write outside the workspace** lands on the host uncontained (`marker-OUTSIDE` PRESENT)
   and is refused inside (`F28_ESCAPE=DENIED`, `marker-INSIDE` ABSENT) — confirmed on the host,
   not taken from the child's self-report.
3. **`/etc/hosts`** is readable outside and denied inside — `/etc` is granted by neither the
   `contained` workspace policy nor the macOS profile allowlist.

**The child is proven to have run** (`F28RAN` in its own stdout). Without that, "no violation"
would be indistinguishable from "no child", which is the exact confusion the activeness rule
exists to forbid.

**`F28_ROOTLS` is identical on both sides, and that is correct.** macOS `sandbox-exec` has no
mount namespace and no PID namespace, so the two Linux signals 28-02 relied on (`NSpid`, root
listing) cannot fire here. This is why an `/etc` read signal was added to the harness. It also
demonstrates the detector is signal-specific rather than firing on noise: three signals fire,
one correctly does not.

## 4. Negative control — the detector discriminates

```
NEGATIVE CONTROL activeness observed=false — detector discriminates
```

The identical detector, fed the **outside** reading on both sides of the comparison, reports
activeness ABSENT. A detector that fired here would be firing unconditionally and every green
it produced would be worthless. This mirrors the two negative controls 28-02 ran on the
Windows activeness detector.

## 5. It is not a bypass — measured on this binary

```
$ WAYLAND_SANDBOX=none wayland-core sandbox exec --workspace <ws> "echo F28RAN"
wayland-core sandbox: sandbox selection: sandbox bypass cannot be activated by
configuration or environment; use an explicit local Dangerous launch
EXIT=1

$ WAYLAND_SANDBOX=none WAYLAND_ALLOW_NO_SANDBOX=1 wayland-core sandbox exec ... "echo F28RAN"
   ... same refusal, EXIT=1
```

The verb cannot be used to run a command uncontained, with or without the opt-out variable.

## 6. The delegated gate is UNCHANGED — measured, not asserted

```
$ wayland-core swarm --workers 1 --worker-command "/bin/sh -c true" --repo <git repo> \
    --base-branch main --timeout 30s
"Failed": "sandbox backend sandbox_exec cannot own descendants that escape a process
 group; select Docker for delegated Swarm execution on this host; qualified Docker
 fallback is unavailable on this macOS host: docker backend disabled (feature
 `live-docker` off)"
```

Byte-for-byte the refusal 28-02 recorded. **The fix did not admit `sandbox_exec` to the
delegated path** — the tempting route that would have turned 24 cells green by weakening a
fail-closed security control.

## 7. Verdict

macOS sandbox activeness **is obtainable** through a black-box surface of the shipped binary,
by the same differential method already used on Linux and Windows, with a negative control
proving the detector discriminates.

**The 24 macOS `sandbox-probes` cells can now be re-run and graded honestly.**
They were **not** re-run here — re-resolution against the tip belongs to plan 28-03, and this
lane must not re-grade the matrix it is supplying evidence to.
