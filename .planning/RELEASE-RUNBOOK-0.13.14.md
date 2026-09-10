# 0.13.14 release runbook

Verified against the live repo 2026-09-10. Every fact here was read from the API
or the tree, not recalled.

## Preconditions, all checked

| fact | value | how |
|---|---|---|
| workspace version | `0.13.14` | `Cargo.toml` |
| npm `latest` and `next` | **0.13.12** | `npm view @ferroxlabs/wayland-core dist-tags` |
| `v0.13.14` tag | does not exist | `git/ref/tags/v0.13.14` -> 404 |
| `main` linear history | **required** | branch protection |
| `main` enforce_admins | **true** | branch protection |
| required reviews | none | branch protection |
| required checks | **13** | listed below |

**0.13.13 was never published to npm.** The public jump is 0.13.12 -> 0.13.14, and
the Desktop hand-off must say so rather than implying a 0.13.13 exists.

`enforce_admins: true` means the 13 checks are not bypassable. There is no
`--admin` path and none is to be attempted.

## The 13 required contexts on `main`

```
CI (linux-containerized)        Build (aarch64-apple-darwin)
CI (macos-latest)               Build (aarch64-pc-windows-msvc)
CI (Array)                      Build (aarch64-unknown-linux-gnu)
Eval acceptance gate            Build (x86_64-apple-darwin)
Bench regression (linux)        Build (x86_64-pc-windows-msvc)
report                          Build (x86_64-unknown-linux-gnu)
scan
```

**Three of these never run on `integ/**`** and only appear on the PR:
`Bench regression (linux)` (bench-regression.yml), `scan` (osv-scan.yml), and the
Supply Chain job — all three are `on: pull_request: branches: [main]`. Verified in
the workflow files. So a green integ run is NOT a prediction of a green PR, and
the PR must be opened early enough to surface them.

## Sequence

1. **Push** `integ/release-0.13.14` via the hetzner adapter (`just push` under
   `flock build.lock`, asserting HEAD). Read the `.status` receipt, NOT the ssh
   exit code — the ssh wrapper has reported 0 over a failed push.
2. **CI green** on the branch. Capture the run id: `release.yml` takes an
   `evidence_run_id` input described as "Successful current-SHA CI/collector run
   carrying `stabilization-raw-<SHA>`; **required for promotion**". Without it the
   promote step has no evidence to bind to.
3. **PR into `main`**, wait for all 13 including the three that only exist there.
4. **Squash-merge** (linear history is required). No `--admin`.
5. **Tag at the exact admitted source** — the merge commit on `main`, not the
   integ head, because a squash rewrites the SHA.
6. **Dispatch** `release.yml` with `--ref main` and `tag_name=v0.13.14`.
   **`--ref` stays `main`.** A tag ref breaks Azure OIDC — the subject claim is
   built from the ref and a tag ref produces a subject the federated credential
   does not match.
7. **Watch to `publish-npm`**, then confirm via the `release-complete` job, which
   fails on any non-success job, `draft != false`, asset `count < 14`, or a
   missing `release-manifest`.

## Deliver to Desktop

Published version; exact source SHA; authenticated release manifest; Windows x64
and Mac ARM64 binaries with SHA256; contract manifest. State absolute values per
binary, never "unchanged" — a relative statement is relative to a baseline the
reader may not share.
