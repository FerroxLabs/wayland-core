# F29-02-H1 — path count and reachability, measured at `b2ddf113`

Every figure below was produced by an unproxied tool (`/usr/bin/grep`,
`/usr/bin/python3`), redirected to a file and read back with the Read tool, never
through Bash stdout. Each absence claim carries a known-positive in the same capture.

---

## 1. The true path count: **3 direct parent edges** — the ledger claim HOLDS

Derived from the pre-fix `Cargo.lock` (`git show 8c5eaa8f^:Cargo.lock`,
sha256 `60f35e73…`) by a **reverse-dependency parser I wrote** (`/tmp/f29h1-revdep.py`),
deliberately a *different instrument* from the `cargo tree -i` the prior lane used, so
this is corroboration rather than a re-run.

Parser self-test in the same capture: 1017 package stanzas parsed, `serde` found —
instrument alive. (A parser returning nothing makes every absence claim true.)

```
=== resolved versions of quick-xml: 2 ===
  quick-xml 0.31.0
  quick-xml 0.39.4

=== DIRECT parent edges ===
  quick-xml 0.31.0 <- calamine 0.26.1
  quick-xml 0.39.4 <- plist 1.9.0
  quick-xml 0.39.4 <- wcore-tools 0.12.25
direct parent edge count: 3
```

**Three, two of them through `wcore-tools`** (the direct dep, and the calamine leg).
The brief's and the ledger's claim is confirmed exactly. `.cargo/audit.toml`'s
predecessor documented **one** of these three and called it the "sole path".

A second figure, reported so the "3" is not mistaken for something it is not: counting
*whole transitive paths* up to the workspace root rather than direct edges gives
**67** distinct paths, all terminating at `wcore-cli 0.12.25`. The documented one was
1 of 3 direct edges, and 1 of 67 full paths. I report the direct-edge count as the
meaningful one because that is what a `cargo tree -i` parent trace enumerates.

## 2. Reachability — **0194 YES, 0195 NO**, and the suppression argued the wrong path

The advisories' triggers are specific, not "parses XML" (fetched raw from
rustsec/advisory-db):

- **RUSTSEC-2026-0194** (quadratic duplicate-attribute check): triggered by
  `BytesStart::attributes()` / `try_get_attribute` **with the default
  `with_checks(true)`**, or by `NsReader`. Explicitly *not* triggered by consumers
  using `.with_checks(false)` and no `NsReader`.
- **RUSTSEC-2026-0195** (unbounded namespace allocation): triggered **only** by
  `NsReader` / `NamespaceResolver::push`. "A plain `Reader` that does not perform
  namespace resolution is not affected."

Both `patched = [">= 0.41.0"]`, neither carries an `unaffected` range — so 0.31.0 was
in scope as well as 0.39.4.

### Trigger-surface census

Instrument alive in the same capture: 32,010 `fn ` and 8 `quick_xml` occurrences under
`crates/`; 437 `fn ` in calamine 0.26.1; 568 `fn ` in plist 1.9.0.

*(An earlier run of this census returned 0 for everything because zsh ate the unquoted
`--include=*.rs`. The known-positive control returned 0 too and caught it. Without that
control I would have reported a clean false absence — the §3b-i trap, live.)*

| Consumer | `.attributes()` | `try_get_attribute` | `NsReader` | `with_checks(false)` |
|---|---|---|---|---|
| workspace `crates/**` own code | 0 | 0 | 0 | 0 |
| **calamine 0.26.1** (vulnerable) | **25** | **6** | 0 | **0** |
| calamine 0.36.1 (patched) | 0 | 0 | 0 | 0 |
| plist 1.9.0 | 0 | 0 | 0 | — |

calamine 0.36.1 reads 0 because upstream **replaced** `.attributes()` with its own
`src/attrs.rs` iterator — an API change, not a dead instrument (21 `attributes`
mentions remain, and a dedicated `attrs.rs` module exists).

### The reachable call path, end to end

```
user-supplied .xlsx
  -> DocExtractTool::execute            crates/wcore-tools/src/doc_tool.rs:220
  -> xlsx_to_markdown                   doc_tool.rs:529-549
       Xlsx::new                        doc_tool.rs:531
       sheet_names                      doc_tool.rs:532
       worksheet_cells_reader           doc_tool.rs:545
       next_cell                        doc_tool.rs:549
  -> calamine-0.26.1 src/xlsx/cells_reader.rs::next_cell   (line 95)
       .attributes()  at lines 102, 113, 161, 172, 190, 197, 218
  -> quick_xml 0.31.0 BytesStart::attributes(), checks ENABLED (default)
  -> O(N^2) duplicate-name scan  == RUSTSEC-2026-0194
```

Seven trigger call sites inside `next_cell` itself; 21 across the xlsx path
(`xlsx/mod.rs` 11 + `xlsx/cells_reader.rs` 10). `doc-extract` is a **default-on**
feature. **RUSTSEC-2026-0194 was genuinely reachable from an attacker-supplied
spreadsheet.**

**RUSTSEC-2026-0195 was NOT reachable** on any of the three paths: it requires
`NsReader`/`NamespaceResolver`, and there are zero uses in the workspace, in calamine
0.26.1, or in plist 1.9.0. The ledger says only "`0194` is reachable" and is right to
name just that one.

### The inversion worth recording

Of the three paths, the one `.cargo/audit.toml` **documented** and argued unreachable —
`quick-xml <- plist <- syntect <- wcore-cli` — is the one with **zero** trigger sites,
i.e. genuinely unreachable. The suppression's reachability argument was *correct about
the path it examined* and it examined the only safe one. The reachable path was among
the two it omitted.

So the defect was not a sloppy argument; it was a **correct argument about an
unrepresentative sample**, presented as covering the whole graph. That is why the
repair has to be an enumeration gate over *every* path, not better prose.

### Where I refine the prior lane

`29-H1-SUMMARY.md` §1 hedges: "`doc_tool.rs` happens never to call
`BytesStart::attributes()` and uses plain `Reader`, not `NsReader` — so a strict
reading says neither advisory's *specific* trigger was demonstrably reachable even
there." That checked `doc_tool.rs`'s **own** calls and not what **calamine** does
internally on the same path. It understated the finding: 0194 was reachable. The prior
lane took the source fix anyway, so the outcome was right — but "the fix was merely
prudent" and "the fix was necessary" are different claims and the second is the true one.

## 3. The fix survived the merge into `b2ddf113`

`Cargo.lock` sha256 `200e0d8d54a070e3b49d71a80832e1f20d39ad0007268e2811f38b50338b7995`,
identical before and after every command in this investigation — nothing I ran mutated it.

```
resolved versions of quick-xml: 1  ->  0.41.0
calamine 0.36.1 ; plist 1.10.0
known-negative: version "0.39.4" -> 0 ; version "0.31.0" -> 0
known-positive control (a version string that IS present, "0.41.0") -> 2   [grep alive]
```

`.cargo/audit.toml` carries `ignore = []`.

**So the brief's premise — "an OPEN and UNFIXED HIGH" — is stale.** The fix landed in
`0ea36bcc` (an ancestor of HEAD) and holds. What remains unbuilt is the *gate*: the
prior lane's rotation policy is a prose comment in `audit.toml`, and a comment cannot
fail.
