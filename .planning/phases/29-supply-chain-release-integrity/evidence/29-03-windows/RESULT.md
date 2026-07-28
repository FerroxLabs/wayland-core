# 29-03 Windows downgrade-refusal leg — RUN. MET.

29-03 recorded `F29-LIMIT-06` as *"the Windows leg — `seandesktop` refusing SSH auth"* and
graded **Windows: NOT ACHIEVED, blocked on a Sean-reserved credential**. The host is reachable
as `SeanD` and no credential was needed (see
`.planning/phases/28-native-cross-platform-certification/evidence/28-03-windows-requeue/HOST-ACCESS.md`).

## Same construction as Linux

Measured **through the shipped binary**, against the **real public GitHub API**, with **no
update-source redirect of any kind** and **no credential**, by rebuilding the package at
`0.99.0` so the newest real release (`v0.12.25`) is a **downgrade**.

Host `SeanD@seandesktop`, account `seand`, lane HEAD `eecfc331`, scheduled task with an explicit
exit marker. Built binary sha256 `9e1586a2890ffc7cc6e28fddeb78b7201a7be03977e1332529b5694d20e3d155`.

```
D_BASELINE_VERSION 0.12.25
D_BUMPED_VERSION   0.99.0
D_BUILD_RC=0

--- wayland-core.exe --version ---
wayland-core 0.99.0

--- self-update --check-only against the REAL api.github.com, no credential, no redirect ---
current: v0.99.0
latest:  v0.12.25
manifest: UNAVAILABLE — the bundled release trust root is a placeholder and is refused: it
holds no keys. Replace RELEASE_TRUST_ROOT_JSON in crates/wcore-cli/src/update_trust.rs with
the real FerroxLabs release trust root before release-manifest verification can work.
REFUSED: the offered release v0.12.25 is OLDER than the running v0.99.0. An update must move
forward; installing a downgrade would reintroduce every defect fixed since v0.12.25. Nothing
was installed.
D_CHECK_RC=0

--- self-update (INSTALL path, same conditions) ---
current: v0.99.0
latest:  v0.12.25
wayland-core self-update: REFUSED: the offered release v0.12.25 is OLDER than the running
v0.99.0. ... Nothing was installed.
D_INSTALL_RC=1

--- did the binary swap itself? ---
wayland-core 0.99.0
```

## Windows against Linux, clause by clause

| Clause | Linux (`evidence/29-03/live-downgrade.txt`) | Windows (this leg) |
|---|---|---|
| running version | `wayland-core 0.99.0` | `wayland-core 0.99.0` |
| latest offered by the real API | `v0.12.25` | `v0.12.25` |
| trust root | placeholder, refused, holds no keys | placeholder, refused, holds no keys |
| `--check-only` | REFUSED, **exit 0** | REFUSED, **rc=0** |
| INSTALL path | REFUSED, **exit 1** | REFUSED, **rc=1** |
| version after the refused install | `0.99.0` — did not swap itself | `0.99.0` — did not swap itself |

**Identical on every clause.** Linux's machine line was
`F29-02-LIVE-DOWNGRADE::check-only-refusals=1::check-rc=0::install-rc=1`; Windows reproduces
`check-rc=0` / `install-rc=1`.

## The trap this leg walked into first

Attempt 1 rewrote the first `version = "..."` line in `crates/wcore-cli/Cargo.toml`. That crate
declares **`version.workspace = true`** and carries no version line at all, so the edit matched
nothing. It would have built a **0.12.25** binary — for which the newest real release is the
**same** version, not a downgrade, so the refusal under test would never have been exercised
and the leg would have reported on a scenario it never created.

It was caught because `D_BASELINE_VERSION_LINE` came back **empty**: a field that cannot
legitimately be empty was treated as a failure rather than rendered as blank. Attempt 2 rewrites
the version inside `[workspace.package]` in the **root** manifest — the same line the Linux run
edited — and asserts the bump took (`D_BUMPED_VERSION 0.99.0`) before building. The manifest is
restored afterwards and re-read to prove it (`D_RESTORED_VERSION 0.12.25`).

`self_update.rs:55` reads `env!("CARGO_PKG_VERSION")`, which resolves through that workspace key.

## Verdict

**29-03's Windows leg: MET.** `F29-LIMIT-06` is closed — it was never a real-credential limit.
The other five `F29-LIMIT-*` rows are untouched by this lane and remain open: they need Sean's
real release keys or a real published signed release, which this lane may not supply.
