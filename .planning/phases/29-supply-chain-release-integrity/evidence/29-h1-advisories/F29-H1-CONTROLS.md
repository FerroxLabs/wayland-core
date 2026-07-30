# F29-H1 — every control, run in BOTH directions

Host `hetzner-dsm`, worktree `/root/wayland-f29h1`, commit
`ead1bfab8c347b64db7888d3e8bf83078f7bbe0b` (SHA asserted after checkout:
WANT == GOT). `Cargo.lock` sha256 `200e0d8d…` — identical on the Mac and on
hetzner, and identical before and after every command below.

`SUPPRESSIONS_EXAMINED` is printed on every run, so a run that checked nothing is
distinguishable from a run that checked everything. **No cell below is a skip.**

## Baseline verdicts at `ead1bfab` (the "can it pass" direction)

| Control | Verdict | rc |
|---|---|---|
| `cargo deny --manifest-path Cargo.toml check` | `advisories ok, bans ok, licenses ok, sources ok` (+76 `warning[duplicate]`, non-fatal) | 0 |
| `cargo audit` | 1029 crates scanned, **0 vulnerabilities**, 6 warning-class advisories left VISIBLE, `ignore = []` | 0 |
| `cargo metadata --locked` | lock is valid, no drift | 0 |
| `verify-suppression-traces.py` | `SUPPRESSIONS_EXAMINED=12  SUPPRESSIONS_FAILED=0` | 0 |
| `verify-suppression-traces.py --self-test` | 11/11 assertions | 0 |

`cargo deny` exercising the edited `deny.toml` is also the TOML validity proof —
the Mac's python has no `tomllib`, so the parse was verified by the real tool.

## The "can it fail" direction — five planted known-positives

| # | Plant | Result | rc |
|---|---|---|---|
| **A** | `cargo audit --file <pre-fix Cargo.lock>` (sha `60f35e73…`, 1017 crates) | **All four quick-xml findings reported**: RUSTSEC-2026-0194 and -0195, each at 0.31.0 **and** 0.39.4 | 1 |
| **B** | remove the `ttf-parser` ignore from `deny.toml` | `error[unmaintained]: 'ttf-parser' is unmaintained` → `advisories FAILED` | 1 |
| **C** | tamper the real `osv-scanner.toml` trace to `parents=bollard` (**the actual historic defect**) | `FAIL [C2-PARENTS] RUSTSEC-2025-0134 … UNDOCUMENTED parents ['rustls-native-certs']`, 1 of 12 | 1 |
| **D** | point a tag at `ttf-parser@0.99.0` (stale mute) | `FAIL [C1-STALE] … not in Cargo.lock — delete this suppression or correct the version` | 1 |
| **E** | run with `--now 2027-01-01` | `SUPPRESSIONS_FAILED=12`, all `C3-EXPIRED` | 1 |

After every plant the file was restored and re-verified: `tree-clean=YES`,
`Cargo.lock` sha unchanged, and a final gate run returned `RC_FINAL_GREEN=0`.

### Plant A discharges the trap named in my brief

> *"A suppression file makes a gate pass. Prove your gate can still detect a real
> advisory by planting a known-positive."*

`cargo audit` at this commit is green. Plant A shows that green is a *measurement*
and not a mute: given a lockfile that genuinely contains the vulnerable crate, the
same scanner at the same commit reports all four findings. It also independently
reproduces the prior lane's "**four**, not two" count — two advisories x two
resolved versions — from a lockfile I extracted myself.

## Self-test assertions (11/11), including the third assertion §6b-ii requires

Direction "can pass": complete/accurate/unexpired trace; a crate with no parents
declaring `NONE`; the 1-parent, 2-parent and 10-parent real shapes.

Direction "can fail": the three **real historical defects** — quick-xml "sole path"
naming 1 of 3, `paste` naming phantom `ratatui` while omitting `tokenizers`,
`rustls-pemfile` naming 1 of 2 — plus stale mute, expired mute, and a prose-only
reason (prose is not a bypass).

Controls: (1) a **prose-containment matcher goes GREEN on the real F29-02-H1 text**
(it does mention `plist`) while this gate reds it — the §6b-ii "the old broken
matcher would have missed it" assertion; (2) an empty lockfile is detected as a
dead instrument rather than passing everything.

### A defect this self-test caught in itself

The first self-test run was **9/11**, and both failures were in the *can-it-pass*
direction. Cause: the fixture wrote `dependencies = [...]` inline while the parser
required a line-leading `]`, so **every parent set was empty**. The failure cases
had been "passing" only because `declared != {}` — i.e. the suite was green on a
fixture with no edges at all. Fixed in both places (parser now accepts either
layout; the self-test asserts its own fixture's edges before using it). Recorded
because §6b-ii's point is that a noted instrument defect left unrepaired recurs.

## The two secondary brief items

**`grep -rn 'environment:' .github/workflows/` → 0. VERIFIED TRUE.**
Instrument alive in the same capture: 27 `runs-on:` across 11 workflow files.

**I did not "fix" it, deliberately.** `gh api repos/FerroxLabs/wayland-core/environments`
returns `{"total_count":0,"environments":[]}`. A GitHub environment referenced by a
workflow is **auto-created on first use with no protection rules**, so adding
`environment: release` to `release.yml` would produce a job that *looks* gated and
approves itself — a permanently-green gate, the §3b-iii failure mode in its purest
form. Required-reviewer configuration is a repo-settings action reserved to Sean.
The honest state is: the finding stands, and closing it needs one repo-settings
change plus a one-line YAML edit, in that order. Doing the YAML alone would convert
a true finding into a false green.

**The dependency-policy verdict is no longer RED, and is already chained.**
The brief describes `advisories FAILED, bans ok, licenses FAILED, exit 5`, deliberately
unchained. At `b2ddf113` that is stale: `justfile:174` reads
`check-all: fmt-check lint test-ci hakari-verify audit deny` (chained 2026-07-29 by
lane 29-deny) and the verdict measured above is `advisories ok, bans ok, licenses ok,
sources ok`, rc=0. My disposition therefore is **keep it chained and green, and close
the gap neither `audit` nor `deny` covers** — both answer "is this advisory muted?",
neither checks whether the stated reason for muting it is true. That is the gap
F29-02-H1 came through, and it is now `verify-suppressions`, also chained into
`check-all` and running unconditionally in `supply-chain.yml`.

One scope caveat carried forward, not resolved: `cargo deny`'s `[graph] all-features`
is now `true`, but a green `cargo deny` still certifies the resolved graph, not every
platform. My gate reads `Cargo.lock` directly, which is the union of all features and
targets — deliberately wider, so it can demand you document a parent edge no default
build activates. For a suppression justification that is the correct bias.
