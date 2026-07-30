# Lane `f29-h1-advisories` — summary

Branch `lane/f29-h1-advisories`, based at integration `b2ddf113`.
Build/verify host `hetzner-dsm`, worktree `/root/wayland-f29h1` (SHA asserted).

**Verdict: the HIGH is closed, and it was already closed before I started. My
contribution is the part that was missing — proof of *why* it mattered, and an
executable gate so the defect shape cannot return.**

---

## 1. Two of my brief's three premises were stale. The third verified.

| Brief claim | Status at `b2ddf113` |
|---|---|
| `F29-02-H1` is "OPEN and UNFIXED" | **STALE.** Fixed at source by `0ea36bcc` (an ancestor of HEAD) on 2026-07-29; `.cargo/audit.toml` has `ignore = []` |
| deny verdict is RED and "deliberately not chained into `check-all`" | **STALE.** Chained at `justfile:174` since 2026-07-29; measured `advisories ok, bans ok, licenses ok, sources ok`, rc=0 |
| `grep -rn 'environment:' .github/workflows/` returns zero | **TRUE.** Re-verified, instrument alive (27 `runs-on:`, 11 files) |

I did not manufacture work to match the stale parts. What I did instead:
re-verify the fix at *my* base, establish the facts the finding rested on, and
build the control nobody had built.

## 2. The true path count: **3 direct parent edges** — the finding was correct

Derived from the pre-fix `Cargo.lock` (`git show 8c5eaa8f^:Cargo.lock`) by a
reverse-dependency parser I wrote — deliberately a **different instrument** from
the `cargo tree -i` the prior lane used, so this corroborates rather than repeats.
Parser self-test in the same capture: 1017 stanzas, `serde` found.

```
quick-xml 0.31.0 <- calamine 0.26.1
quick-xml 0.39.4 <- plist 1.9.0
quick-xml 0.39.4 <- wcore-tools 0.12.25      direct parent edge count: 3
```

Three, two through `wcore-tools`. `audit.toml`'s predecessor documented **one** and
called it "sole". (Counting whole transitive paths to the workspace root instead
gives 67; I report the direct-edge count because that is what a parent trace
enumerates.)

## 3. `0194` was reachable. `0195` was not. And the suppression argued the wrong path.

Reachability traced to the **call site**, not inferred from the dependency graph.
The advisories' trigger is specific: `BytesStart::attributes()` / `try_get_attribute`
with the default duplicate check, or `NsReader` (0194); `NsReader` only (0195).

```
user-supplied .xlsx
  -> DocExtractTool::execute        crates/wcore-tools/src/doc_tool.rs:220
  -> xlsx_to_markdown               doc_tool.rs:531-549 (Xlsx::new, sheet_names,
                                    worksheet_cells_reader, next_cell)
  -> calamine-0.26.1 xlsx/cells_reader.rs::next_cell   (line 95)
       .attributes() at 102, 113, 161, 172, 190, 197, 218
  -> quick_xml BytesStart::attributes(), checks ENABLED  ==  RUSTSEC-2026-0194
```

21 trigger sites across calamine 0.26.1's xlsx path, **zero** `with_checks(false)`
in that crate, and `doc-extract` is default-on. **0194 REACHABLE.**

**0195 NOT reachable** anywhere: zero `NsReader`/`NamespaceResolver` uses in the
workspace, in calamine 0.26.1, or in plist 1.9.0.

**The inversion.** Of the three paths, the one `audit.toml` documented and argued
unreachable — `plist <- syntect <- wcore-cli` — is the only one with **zero**
trigger sites. The reachability argument was *correct*, about the one safe path,
presented as covering the graph. That is the precise defect shape, and it is why
the repair had to be an enumeration gate rather than better prose.

**This refines the prior lane.** `29-H1-SUMMARY.md` hedged that "neither advisory's
specific trigger was demonstrably reachable", having checked `doc_tool.rs`'s own
calls but not calamine's internals. The fix was not merely prudent; it was necessary.

## 4. What I built: `scripts/verify-suppression-traces.py`

The prior lane's rotation policy is a **comment**, and a comment cannot fail. Nor
was this a one-off: two other traces had already drifted the same way and were
caught by hand — `paste` named `ratatui` as a puller (false) and omitted the
`tokenizers` root; `rustls-pemfile` claimed "only via bollard", naming one of two
edges. Three instances, three human re-derivations.

The gate re-derives the direct-parent set for every suppression in `deny.toml`,
`.github/osv-scanner.toml` and `.cargo/audit.toml` straight out of `Cargo.lock`
and enforces **exact set equality** — catching an omitted parent *and* a phantom
one — plus a stale mute (crate gone from the lock), an expired mute, and any entry
with no checkable trace at all (prose is not a bypass). All 12 live suppressions
now carry a `[trace crate=…@… parents=… expires=…]` tag whose parents were
**generated from the lockfile, not transcribed**.

Wired into `just check-all` and into `supply-chain.yml` **with no `if:` guard** —
that job's path-relevance filter can skip on a dependency change that invalidates
a trace, and a skipped gate reports the same green as a passing one.

## 5. Both directions, on every control — no skips

Full table in `evidence/29-h1-advisories/F29-H1-CONTROLS.md`.

Can-pass: `cargo deny` rc=0 `advisories ok, bans ok, licenses ok, sources ok`;
`cargo audit` 1029 crates / 0 vulnerabilities / `ignore = []`; `cargo metadata
--locked` rc=0; gate `SUPPRESSIONS_EXAMINED=12 SUPPRESSIONS_FAILED=0`; self-test 11/11.

Can-fail, five planted known-positives, each red for the right reason:

- **A** — `cargo audit --file <pre-fix lock>` reports **all four** quick-xml findings
  (0194+0195 x 0.31.0+0.39.4). This is the trap my brief named: it proves the green
  above is a *measurement*, not a mute, and reproduces the "four, not two" count.
- **B** — remove one `deny.toml` ignore → `error[unmaintained]`, `advisories FAILED`.
- **C** — tamper the **real** osv trace into the actual historic `rustls-pemfile`
  defect → `C2-PARENTS … UNDOCUMENTED parents ['rustls-native-certs']`.
- **D** — tag a version absent from the lock → `C1-STALE`.
- **E** — `--now 2027-01-01` → 12/12 `C3-EXPIRED`.

Tree restored clean after every plant; `Cargo.lock` sha256 `200e0d8d…` unchanged
throughout and identical on both hosts; final gate run green.

**The self-test caught a defect in itself.** First run was 9/11, both failures in
the *can-pass* direction: the fixture used inline dependency arrays the parser
could not read, so every parent set was empty and the fail-cases were passing for
the wrong reason. Repaired in the parser and guarded by a fixture assertion —
per §6b-ii, repaired in-lane rather than written up.

## 6. What I did NOT do

- **Did not add `environment:` to any workflow.** The claim is true, but
  `gh api …/environments` returns `total_count: 0`, and GitHub auto-creates a
  named environment **with no protection rules**. Adding the YAML alone would
  produce a job that looks gated and approves itself — converting a true finding
  into a false green. Closing it needs a repo-settings change (required reviewers)
  **first**, which is Sean's. Left open and documented.
- **Did not re-run `osv-scanner`** — still not installed on `hetzner-dsm`. The
  same gap the prior lane recorded. I edited `.github/osv-scanner.toml`, and my
  gate parses it, but no osv-scanner count in this lane is mine.
- Did not build the workspace, run the full suite, merge, open a PR, tag, release,
  close an issue, or run `wcore-contract generate`.
- Did not touch `crates/wcore-cli/src/{lib,main}.rs`.
- Did not change any dependency, so `Cargo.lock` is byte-identical to base
  (`cargo metadata --locked` rc=0 confirms no drift).

## 7. Still open in this family

- Manual-approval gate: needs Sean's repo-settings change, then one YAML line.
- `osv-scanner` never executed by anyone; its `.toml` is maintained blind.
- The five `deny.toml` suppressions now carry `expires=2026-09-02`. **The gate will
  go red on 2026-09-03** unless each is re-derived and re-accepted. That is
  intended — a mute nobody revisits is the thing that produced this finding — but
  it is a dated commitment someone must service.
- Three of the SUPPLY-* family's five named members (update identity, revocation/
  rotation, rollback rehearsal) remain unbuilt; untouched by this lane.
