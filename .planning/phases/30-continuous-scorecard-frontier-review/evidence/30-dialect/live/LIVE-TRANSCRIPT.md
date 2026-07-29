# 30-dialect — live evidence, `hetzner-dsm`, 2026-07-29

Everything below is a real run of a real binary. Nothing is a unit test.
Worktree `/root/wayland-30-dialect` at `lane/30-dialect`; binaries built with
`cargo build -p wcore-cli -p wcore-eval-scenarios --bin wayland-core --bin wayland-scorecard`
(targeted, per LANE-BRIEF §2). `df -h /root` before: 720G avail; after: 711G avail.

---

## 1. Unit suite — the executed count read back, not the exit status

```
cargo test -p wcore-eval-scenarios --lib dialect
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 222 filtered out
RC=0
```

**28 executed**, not 0. Asserted deliberately: LANE-BRIEF §3.2 flavour (c) is a filter that
matches no test name and exits 0 having run nothing. `dialect` matched 28 of 250.

## 2. `wayland-core` refuses to start on a headless host — independently reproduced

30-02 recorded this. It reproduces exactly, and it is why the first discovery attempt captured
**zero requests**:

```
DIALECT_DISCOVER label=wayland declared_tools=0 requests=0 model=- names=
```

Run directly with a cleared environment (`env -i` + PATH/HOME/LANG/BASE_URL/API_KEY):

```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked. On a headless host
set WAYLAND_VAULT_PASSPHRASE_FD ... or turn durable sessions off with [session] enabled = false
```

`WAYLAND_VAULT=plaintext` alone is **not** sufficient — measured, rc=1. What works is a seeded
`.config/wayland-core/config.toml` carrying `[credentials] backend = "encrypted-file"` plus
`WAYLAND_VAULT_PASSPHRASE`. Both are carried as **data** in `inv-wayland.json`
(`workspace_seed_files` / `extra_env`), never hidden in the driver, so the extra setup our own
tool requires stays visible to a reader — exactly as 30-02 insisted.

The passphrase is the synthetic literal `frontier-trial-not-a-secret`. It unlocks a throwaway
vault in a per-run workspace and authenticates nothing.

## 3. Live dialect discovery against the real binary

```
wayland-scorecard dialect discover --invocation inv-wayland.json \
  --workspace-root ws --tool-version "0.12.25 lane/30-dialect@cb38864d" \
  --timeout-s 150 --out-prefix wayland

DIALECT_DISCOVER label=wayland declared_tools=8 requests=1 \
  corpus_sha256=10c85fbd29ba89bdd08539c63d73744efb5616f584ed84c7f96e4c4a9e8f1323 \
  model=fixture-chat-v1 names=Bash,Edit,Forge,Glob,Grep,Read,ToolSearch,Write
```

**Eight tools, declared by the harness itself, on the wire.** Not read from source, not written
by me: `Bash, Edit, Forge, Glob, Grep, Read, ToolSearch, Write`.

Corpus: `wayland-corpus.json`. Manifest: `wayland-manifest.json`.

## 4. The confound, confirmed on real captured data

```
=== does the REAL corpus declare the frozen v1 name `write_file`? ===
ABSENT -> v1 emitted a call this harness cannot dispatch. This IS the measured 0/30.
```

This is the third assertion of the instrument self-test, run against a real corpus rather than a
synthetic one. The frozen v1 script names `write_file`; this harness declares `Write` and has no
tool of that name. **The 0/30 was the script, not the product.** Had `write_file` been present,
the whole repair would have been unnecessary and the self-test would have been vacuous — which is
why the test asserts its absence rather than assuming it.

## 5. Compilation and verification, all four dimensions

```
DIALECT_VOCABULARY=OK version=F30-DIALECT-VOCAB-V1 product_tokens_found=0

correctness  COMPILE=OK tool_names=Write  translation_sha256=3ad52c367219ff4278abe86d66401be1983a0b145b83ddc657c1e463a778b4dd   VERIFY=OK
recovery     COMPILE=OK tool_names=Write  translation_sha256=1da202f3a0b1b06183c06b08bf2f3deeb4dec5c962f3049cc8313854fc5ccb5b   VERIFY=OK
cost         COMPILE=OK tool_names=Write  translation_sha256=3ad52c367219ff4278abe86d66401be1983a0b145b83ddc657c1e463a778b4dd   VERIFY=OK
security     COMPILE=OK tool_names=Read   translation_sha256=9e8cd0a8d7507f60fc1541be6d0a64239bee356592bf6dbaffb4d2b3dcc95777   VERIFY=OK
```

All against `corpus_sha256=10c85fbd…`. `correctness` and `cost` share a digest because their
canonical scripts are identical — that is expected and is a property of the frozen v1 script too.

Note what the filter did with the other seven declared tools, without being told anything about
them: `Edit` and `Forge` and `Glob` and `Grep` and `Bash` and `ToolSearch` were all excluded, and
`Read` was selected for the read intent and not for the write intent.

## 6. Red-then-green — the verifier can FAIL

| Case | Expected | Measured |
|---|---|---|
| hand-tune the oracle content inside a compiled translation | DETECT | **rc=1**, `translation_sha256 mismatch` |
| verify a translation against a *different* corpus (`Write`→`Writer`) | DETECT | **rc=1**, `corpus_sha256 mismatch` |
| the untampered pair | PASS | **rc=0**, `DIALECT_VERIFY=OK` |

The two failures are what make the third row worth anything. A verifier that only ever returned
OK would have produced the identical third row.

## 7. The cohort gate, live — including the case that costs us

```
GATE 1 — the real cohort as it stands today (peers not provisioned):
  DIALECT_COHORT=INELIGIBLE dimension=correctness members=1 refused_by=COHORT_TOO_SMALL:1
    consequence=NO_HARNESS_IS_RUN_OR_PUBLISHED_FOR_THIS_DIMENSION
    member=wayland declared_tools=8 resolved=Write refusal=-
  rc=1

GATE 2 — two members, the peer cannot resolve (synthetic patch-only surface):
  DIALECT_COHORT=INELIGIBLE dimension=correctness members=2 refused_by=peer
    consequence=NO_HARNESS_IS_RUN_OR_PUBLISHED_FOR_THIS_DIMENSION
    member=wayland declared_tools=8 resolved=Write   refusal=-
    member=peer    declared_tools=2 resolved=-       refusal=DIALECT_NO_CANDIDATE intent=write_file
  rc=1

GATE 3 — green control, two members that both resolve:
  DIALECT_COHORT=ELIGIBLE dimension=correctness members=2 all_resolved=true
    member=wayland declared_tools=8 resolved=Write
    member=peer    declared_tools=2 resolved=write_file
  rc=0
```

**GATE 2 is the load-bearing row.** Wayland resolved perfectly — `Write`, cleanly, no ambiguity —
and the dimension is *still* ineligible, and Wayland's number is *still* unpublishable, purely
because the peer refused. That is the panel's amendment doing the thing it was added to do:
selective measurability now costs the vendor exactly what it costs a peer.

**GATE 1 is today's real state.** The lane can compile Wayland's dialect and cannot publish
anything, because a cohort of one is not a cohort.

## 8. Credential discipline

- **No provider credential was used anywhere in this lane.** Every leg is loopback. The key at
  `~/.wayland-secrets/flux.env` was never opened, so lane provider spend is **$0.00**.
- Child processes spawned with `env_clear()` plus the same non-secret allowlist `trials run` uses.
- Sweep of every file under `evidence/30-dialect/` against the live `flux.env` values:
  **0 occurrences**.
- The two literals that appear (`wayland-frontier-trial-not-a-secret`,
  `frontier-trial-not-a-secret`) are synthetic, authenticate nothing, and never leave loopback.
