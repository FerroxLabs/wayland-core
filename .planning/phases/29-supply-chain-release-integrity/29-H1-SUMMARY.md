# F29-02-H1 — quick-xml DoS pair: disposition and evidence

**Disposition: FIXED.** Both advisories are eliminated at source. The
`.cargo/audit.toml` ignore list is back to `ignore = []` and `cargo audit`
passes with **no suppression of any kind**.

- Lane branch: `lane/29-h1`
- HEAD: `0ea36bcc06a6fa9b10b90223f2057bec2f8ca02d`
- Base: `12fc794f` (`plan/f20-unified-audit-repair`)
- Build/test host: `hetzner-dsm`, worktree `/root/wayland-29-h1`
- Files changed: `Cargo.toml`, `Cargo.lock`, `.cargo/audit.toml`,
  `.github/osv-scanner.toml`,
  `crates/wcore-tools/tests/doc_extract_quickxml_migration.rs` (new)
- Shared-file fence: `crates/wcore-cli/src/{lib,main}.rs` **untouched**
  (`git diff $BASE -- …` empty, `$BASE` captured once as `12fc794f`)

---

## 1. The finding was correct, and understated by one whole path

The brief's grounded facts all verified at `12fc794f`:

| Claim | Verified |
|---|---|
| `crates/wcore-tools/Cargo.toml:109` — `quick-xml` is a **direct** dep | yes |
| root `Cargo.toml:275` — `quick-xml = "0.39"` → resolves 0.39.4 | yes |
| `doc-extract` is **default-on** (`Cargo.toml:135`) | yes |
| `doc_tool.rs:644/775` parse docx/pptx from **user-supplied files** | yes |
| nothing calls `with_checks(false)` | yes |
| quick-xml 0.41.0 published, un-yanked | yes (crates.io sparse index, 103 versions) |

So the `.cargo/audit.toml` "Parent trace (sole path)" claim was false. But the
brief also instructed me not to resurrect the calamine leg, on the grounds that
quick-xml 0.31.0 "neither advisory names" and that leg "was already withdrawn as
wrong."

**That withdrawal was itself wrong, and I am reporting it rather than complying.**
Both advisories declare `patched = [">= 0.41.0"]` with **no `unaffected`
range** (RustSec advisory-db source, fetched raw), so every version below
0.41.0 is in scope — 0.31.0 included. This is not an argument; it is what the
gating tool does. At the base commit, with the repo's ignore list bypassed:

```
error: 4 vulnerabilities found!        # not 2
quick-xml 0.31.0  RUSTSEC-2026-0195    (severity 7.5 high)
quick-xml 0.31.0  RUSTSEC-2026-0194
quick-xml 0.39.4  RUSTSEC-2026-0195
quick-xml 0.39.4  RUSTSEC-2026-0194
```

There were **three** parent paths, of which `audit.toml` documented one and
called it "sole":

```
quick-xml v0.31.0 <- calamine v0.26.1 <- wcore-tools          # undocumented
quick-xml v0.39.4 <- wcore-tools (direct)                     # undocumented
quick-xml v0.39.4 <- plist v1.9.0 <- syntect v5.3.0 <- wcore-cli   # the only documented one
```

The two undocumented paths are both `DocExtractTool`, which parses docx/pptx
(quick-xml directly) and xlsx/ods (through calamine) from user-supplied files —
exactly the untrusted-input condition both advisories describe.

**One nuance I will not overstate.** The *documented* path's reachability
argument was correct: syntect is used only via `SyntaxSet::load_defaults_newlines()`
and `ThemeSet::load_defaults()` (`crates/wcore-cli/src/tui/widgets/diff.rs:332-333`),
which read embedded binary dumps; there is no `from_folder` / `load_from_reader`
anywhere in the workspace (grepped). And on the omitted paths, `doc_tool.rs`
happens never to call `BytesStart::attributes()` and uses plain `Reader`, not
`NsReader` — so a strict reading says neither advisory's *specific* trigger was
demonstrably reachable even there. That is a fragile defence one refactor from
being false, and it is emphatically **not** what the file claimed. The right
answer was to take the source fix, which is what I did.

---

## 2. The fix

`plist 1.10.0` (requires quick-xml `^0.41.0`) and `calamine 0.36.1` (requires
quick-xml `^0.41`, MSRV 1.88 vs our 1.95) were both already published — so
`audit.toml`'s "No fix to take at source" was also false. Pins moved to:

```toml
calamine  = "0.36"     # was "0.26"
quick-xml = "0.41"     # was "0.39"
```

`cargo update -p plist` then resolved:

```
Updating calamine v0.26.1 -> v0.36.1
Updating plist    v1.9.0  -> v1.10.0
Removing quick-xml v0.31.0
Removing quick-xml v0.39.4
  Adding quick-xml v0.41.0
```

**No source changes were required.** `doc_tool.rs` uses only `Reader::from_str`,
`read_event`, `Event::{Start,End,Text,Eof}`, `QName::as_ref`, `BytesText::decode`
and calamine's `Xlsx::new` / `sheet_names` / `worksheet_cells_reader` /
`next_cell` / `DataRef` — all unchanged across 0.39→0.41 and 0.26→0.36. The
0.40 changelog's breaking items (`read_text` returning `BytesText`, removed
deprecated `NsReader::resolve*`, `xml_content` taking `XmlVersion`, attribute
unescape deprecations) touch nothing this crate calls. `cargo check -p wcore-tools
--all-targets`: **0 errors, 0 warnings**.

---

## 3. Red before green

| Gate | Base `12fc794f`/`e5f40ac1` | After `0ea36bcc` |
|---|---|---|
| `cargo audit`, neutral CWD, **no ignore list** | **rc=1, 4 vulnerabilities** | **rc=0, 0 vulnerabilities**, 7 warning-class |
| `cargo audit` at repo root (CI condition) | rc=0 *only because 2 ids were ignored* | **rc=0 with `ignore = []`** |
| `cargo tree -i quick-xml@0.39.4` | 2 parents (plist, wcore-tools) | `did not match any packages` |
| `cargo tree -i quick-xml@0.31.0` | 1 parent (calamine) | `did not match any packages` |
| `cargo tree -i quick-xml@0.41.0` | absent | calamine 0.36.1 + wcore-tools + plist 1.10.0←syntect |

The base run is the honest red: the repo-root run was green *only* because the
two ids were suppressed. Removing the suppression at base yields rc=1.

---

## 4. Extraction proof — real files, real tool, byte-identical

New: `crates/wcore-tools/tests/doc_extract_quickxml_migration.rs`. Each case
builds a **real zip archive** with the OOXML parts inside, writes it to a real
path, and drives `DocExtractTool` through the public `Tool::execute`. Not a
compile check.

```
BEFORE (e5f40ac1): test result: ok. 5 passed; 0 failed; 0 ignored; 0 filtered out
AFTER  (19ec58f8): test result: ok. 5 passed; 0 failed; 0 ignored; 0 filtered out
md5 of extracted blocks, both runs: 8eb73ef66355200c38e5147c79a2bc11
diff BEFORE AFTER -> rc=0, 0 bytes            # byte-identical
```

Extracted docx (includes the required table case), unchanged across the bump:

```
Wayland Core quarterly report
Prepared for the supply-chain review.
| Crate | Version | Status |
| --- | --- | --- |
| quick-xml | 0.41.0 | patched |
| calamine | 0.36.1 | patched |

End of report.
```

xlsx (the calamine leg) and pptx likewise identical; a csv case is included as a
control that does not touch either parser.

Wider suites at `0ea36bcc`, run per-crate in isolation:

- `cargo test -p wcore-tools` — **1195 passed, 0 failed, 5 ignored** (25 targets)
- `cargo test -p wcore-cli --lib` — **1830 passed, 0 failed, 1 ignored**
- `cargo clippy -p wcore-tools -p wcore-cli --all-targets -- -D warnings` — rc=0
- `cargo fmt --all -- --check` — rc=0

> The wcore-cli run also prints `test result: FAILED. 0 passed; 1 failed` at
> line 1584. That is **not** wcore-cli: it is a nested `failing_fixture` crate
> (`panicked … "deliberate"`) that a wcore-cli test deliberately shells out to.
> Pre-existing, and the parent run still exits 0. Flagging it because counting
> "test result" lines without reading them would misreport this run.

---

## 5. The instrument was tested against a known-positive and a known-negative

Per the standing rule that a checker tends to carry the defect class it hunts:

1. **It failed for real, unprompted.** The first run was `1 failed` — my pptx
   fixture lacked `ppt/presentation.xml`, which the format sniffer keys on. The
   test caught a genuinely broken input rather than passing it.
2. **Known-positive (targeted).** I mutated the docx parser only
   (`b"t" => in_text = true` → `false` in `docx_xml_to_text`) and re-ran:
   `3 passed; 2 failed` — exactly `docx_paragraphs_and_table_extract` and
   `feature_gate_reports_honestly`, the two docx-dependent cases.
3. **Known-negative (same run).** `pptx`, `xlsx` and `csv` still passed under
   that mutation, so the suite is not failing wholesale — the assertions bind to
   the specific parser path they name. Source restored (`grep -c MUTANT` → 0).

**Zero-test hazards actively avoided.** No file-level `#![cfg]` — the extraction
cases carry per-function `#[cfg(feature = "doc-extract")]` and
`feature_gate_reports_honestly` is ungated and asserts a real outcome in both
configurations. Measured executed counts: **5** with default features, **1**
with `--no-default-features` — never 0. All targets were run **by file**
(`--test doc_extract_quickxml_migration`), never by name filter. Every capture
was redirected to a file, then `echo $?`, then `wc -c`.

`--no-default-features` still compiles and degrades honestly:

```
Cannot extract text from …/gate.docx: this build of wcore-tools was compiled
without the `doc-extract` feature. Rebuild with the default features (or
`--features doc-extract`) to enable office-document extraction.
```

---

## 6. Live evidence on the real artifact

`cargo build -p wcore-cli` succeeded (rc=0) and the **shipped 328 MB
`target/debug/wayland-core` binary** carries only the patched versions:

```
$ strings -a target/debug/wayland-core | grep -oE "quick-xml-0\.[0-9]+\.[0-9]+" | sort -u
quick-xml-0.41.0
$ strings -a target/debug/wayland-core | grep -oE "calamine-0\.[0-9]+\.[0-9]+" | sort -u
calamine-0.36.1
$ strings -a … | grep -cE "quick-xml-0\.(31|39)\.|calamine-0\.26\."
0
```

**Partial, and I am labelling it as such.** I could not drive `doc_extract`
through the running binary process. `wayland-core mcp-serve` was the only
provider-key-free tool surface, and it exposes exactly three tools —
`['Read', 'Grep', 'Glob']` (verified live over MCP stdio: `INIT_OK=True`,
`TOOL_COUNT=3`, `DOC_EXTRACT_REGISTERED=False`). `DocExtractTool` is registered
in `crates/wcore-agent/src/bootstrap.rs:1082`, i.e. the full agent registry,
which needs an LLM provider credential I must not supply. So the live leg is:
real binary built, real binary's dependency set inspected, real office files
(`file` reports "Microsoft PowerPoint 2007+" / "Microsoft Excel 2007+")
extracted through the real `DocExtractTool` — but **not** through the
`wayland-core` process itself. A provider-free way to invoke the full registry
would close that gap; it does not exist today.

---

## 7. What the two config files now say

`.cargo/audit.toml`: `ignore = []`. Both entries **deleted**, not re-argued,
and the file records the three specific errors in the justification they
carried (one path of three named; reachability argued only for that one path;
"no fix to take at source" asserted while 0.41.0/1.10.0/0.36.1 were published).
Rotation policy extended: do not float quick-xml below 0.41 or calamine below
0.36, and any future entry must trace **every resolved version** of a crate,
with the path count stated.

`.github/osv-scanner.toml`: both `[[IgnoredVulns]]` blocks removed, same
account recorded, and the "Parent trace recipe" corrected to require one
`cargo tree -i` per resolved version.

---

## 8. Open items and honest gaps

- **`osv-scanner` itself was not re-run** — the binary is not installed on
  `hetzner-dsm`. The 4→0 measurement is `cargo audit`'s. I edited
  `.github/osv-scanner.toml` to match, and said so in its header rather than
  quoting an osv-scanner count I did not produce. Someone with the tool should
  confirm.
- **`cargo hakari verify` was not run** (`cargo-hakari` not installed).
  Assessed as no-impact: `workspace-hack/Cargo.toml` is a 15-line stub with an
  empty `[dependencies]` — hakari has never been generated, so no dep change
  can desync it.
- **New transitive crates** from the bump: `atoi_simd`, `debug_unsafe`,
  `fast-float2`, `zlib-rs`, and **`zip v8.6.0`** (calamine 0.36's own zip; the
  workspace still uses `zip = "2"` directly, so two zip majors now coexist).
  None are flagged by `cargo audit`. Worth a look at the next dep review.
- **`FerroxLabs/wayland-core#142`** was the tracking issue for this pair. It is
  now satisfied. Closing it is a Sean/release action — I did not touch it.
- I did **not** merge, open a PR, tag, release, or run `wcore-contract generate`.
- I did not re-run the full workspace suite; per the lane brief I built targeted
  (`-p wcore-tools`, `-p wcore-cli`) with four other lanes live, and every number
  above comes from an isolated per-crate run.

## Verdict

The HIGH is **closed at source, not argued away.** Four `cargo audit`
vulnerabilities became zero with the ignore list emptied, the two vulnerable
quick-xml versions are absent from `Cargo.lock` and from the compiled binary,
and document extraction is byte-for-byte unchanged on real docx/pptx/xlsx/csv
files. The one thing I could not do is drive `doc_extract` through the running
`wayland-core` process, and that is recorded above as a gap rather than dressed
up.
