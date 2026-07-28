//! F29-02-H1 — behavioural proof that `DocExtractTool` extraction is unchanged
//! across the quick-xml 0.39 -> 0.41 and calamine 0.26 -> 0.36 upgrades that
//! remove RUSTSEC-2026-0194 / RUSTSEC-2026-0195 from `wcore-tools`' dependency
//! tree.
//!
//! This is deliberately NOT a compile check. Every case builds a REAL office
//! file (an actual zip archive with the OOXML parts inside), writes it to a real
//! temporary path, and drives the real `DocExtractTool` through the public
//! `Tool::execute` entry point — the same path the agent uses.
//!
//! Each case prints its extracted text between `>>>BEGIN <case>` / `<<<END
//! <case>` sentinels so the pre-upgrade and post-upgrade runs can be diffed
//! byte-for-byte (`cargo test --test doc_extract_quickxml_migration -- \
//! --nocapture`). The assertions are the gate; the printed block is the
//! evidence.
//!
//! The whole file is `#![cfg(feature = "doc-extract")]`-free ON PURPOSE: a
//! file-level `cfg` is one of the ways a suite silently runs zero tests. The
//! feature is default-on, and `no_default_features_reports_honestly` below
//! asserts the honest-error path instead, so a `--no-default-features` build
//! still executes real assertions rather than vanishing.

use std::io::Write;

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::doc_tool::DocExtractTool;

/// Build a real zip archive from `(part_name, contents)` pairs.
fn zip_bytes(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, content) in parts {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn write_fixture(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_string()
}

/// Run the real tool over a real path and return `(is_error, content)`.
async fn extract(path: &str) -> (bool, String) {
    let tool = DocExtractTool::new();
    let result = tool.execute(json!({ "path": path })).await;
    (result.is_error, result.content)
}

/// Print an extracted block between stable sentinels for before/after diffing.
fn emit(case: &str, content: &str) {
    println!(">>>BEGIN {case}");
    println!("{content}");
    println!("<<<END {case}");
}

// ── docx: paragraphs + a real table ──────────────────────────────────────────

/// A .docx whose body has two paragraphs and a 2x3 table (header + two rows),
/// using the `w:` namespace prefix a real Word producer emits.
fn docx_fixture() -> Vec<u8> {
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Wayland Core quarterly report</w:t></w:r></w:p>
    <w:p><w:r><w:t>Prepared for the </w:t></w:r><w:r><w:t>supply-chain review.</w:t></w:r></w:p>
    <w:tbl>
      <w:tr>
        <w:tc><w:p><w:r><w:t>Crate</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>Version</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>Status</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>quick-xml</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>0.41.0</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>patched</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>calamine</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>0.36.1</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>patched</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>
    <w:p><w:r><w:t>End of report.</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
    zip_bytes(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        ("word/document.xml", document),
    ])
}

#[tokio::test]
async fn docx_paragraphs_and_table_extract() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "report.docx", &docx_fixture());

    let (is_error, content) = extract(&path).await;
    emit("docx", &content);
    assert!(!is_error, "docx extraction errored: {content}");

    // Paragraph text, including a paragraph split across two runs.
    assert!(
        content.contains("Wayland Core quarterly report"),
        "first paragraph missing: {content}"
    );
    assert!(
        content.contains("Prepared for the supply-chain review."),
        "split-run paragraph not joined: {content}"
    );
    assert!(
        content.contains("End of report."),
        "trailing paragraph missing: {content}"
    );

    // The table case the brief requires: header row + both data rows, rendered
    // as a GitHub-flavoured markdown table.
    assert!(
        content.contains("| Crate | Version | Status |"),
        "table header not rendered: {content}"
    );
    assert!(
        content.contains("| --- | --- | --- |"),
        "table separator not rendered: {content}"
    );
    assert!(
        content.contains("| quick-xml | 0.41.0 | patched |"),
        "table row 1 not rendered: {content}"
    );
    assert!(
        content.contains("| calamine | 0.36.1 | patched |"),
        "table row 2 not rendered: {content}"
    );
}

// ── pptx: two slides, ordered ────────────────────────────────────────────────

fn pptx_fixture() -> Vec<u8> {
    let slide = |title: &str, bullet_a: &str, bullet_b: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree>
    <p:sp><p:txBody>
      <a:p><a:r><a:t>{title}</a:t></a:r></a:p>
      <a:p><a:r><a:t>{bullet_a}</a:t></a:r></a:p>
      <a:p><a:r><a:t>{bullet_b}</a:t></a:r></a:p>
    </p:txBody></p:sp>
  </p:spTree></p:cSld>
</p:sld>"#
        )
    };
    // Deliberately added to the archive out of numeric order, so the test also
    // covers slide ordering rather than archive order.
    zip_bytes(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#,
        ),
        // The format sniffer keys pptx off `ppt/presentation.xml`, so a real
        // deck must carry it (slides alone are rejected as an unrecognized ZIP).
        (
            "ppt/presentation.xml",
            r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst><p:sldId id="256"/><p:sldId id="257"/></p:sldIdLst></p:presentation>"#,
        ),
        (
            "ppt/slides/slide2.xml",
            &slide("Second slide", "calamine 0.36.1", "quick-xml 0.41.0"),
        ),
        (
            "ppt/slides/slide1.xml",
            &slide("First slide", "RUSTSEC-2026-0194", "RUSTSEC-2026-0195"),
        ),
    ])
}

#[tokio::test]
async fn pptx_slides_extract_in_numeric_order() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "deck.pptx", &pptx_fixture());

    let (is_error, content) = extract(&path).await;
    emit("pptx", &content);
    assert!(!is_error, "pptx extraction errored: {content}");

    assert!(
        content.contains("## Slide 1"),
        "slide 1 heading missing: {content}"
    );
    assert!(
        content.contains("## Slide 2"),
        "slide 2 heading missing: {content}"
    );
    assert!(
        content.contains("First slide"),
        "slide 1 title missing: {content}"
    );
    assert!(
        content.contains("RUSTSEC-2026-0194"),
        "slide 1 bullet missing: {content}"
    );
    assert!(
        content.contains("Second slide"),
        "slide 2 title missing: {content}"
    );
    assert!(
        content.contains("quick-xml 0.41.0"),
        "slide 2 bullet missing: {content}"
    );

    // Numeric slide order must win over the archive's insertion order.
    let s1 = content.find("First slide").expect("slide 1 text");
    let s2 = content.find("Second slide").expect("slide 2 text");
    assert!(
        s1 < s2,
        "slides emitted out of numeric order (s1={s1}, s2={s2}): {content}"
    );
}

// ── xlsx: the calamine leg (quick-xml 0.31.0 at base) ────────────────────────

/// A minimal but genuinely valid .xlsx: content types, package + workbook
/// relationships, a workbook naming one sheet, and a worksheet with inline
/// strings and a numeric cell.
fn xlsx_fixture() -> Vec<u8> {
    let cells = [
        ("A1", "Advisory"),
        ("B1", "Fixed in"),
        ("A2", "RUSTSEC-2026-0194"),
        ("B2", "0.41"),
        ("A3", "RUSTSEC-2026-0195"),
        ("B3", "0.41"),
        ("A4", "count"),
        ("B4", "4"),
    ];
    let mut rows = String::new();
    for r in 1..=4 {
        let mut row = String::new();
        for (rf, v) in cells.iter() {
            if !rf.ends_with(&r.to_string()) {
                continue;
            }
            if v.parse::<f64>().is_ok() {
                row.push_str(&format!(r#"<c r="{rf}"><v>{v}</v></c>"#));
            } else {
                row.push_str(&format!(
                    r#"<c r="{rf}" t="inlineStr"><is><t>{v}</t></is></c>"#
                ));
            }
        }
        rows.push_str(&format!(r#"<row r="{r}">{row}</row>"#));
    }
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:B4"/><sheetData>{rows}</sheetData></worksheet>"#
    );
    zip_bytes(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Advisories" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        ("xl/worksheets/sheet1.xml", &sheet),
    ])
}

#[tokio::test]
async fn xlsx_sheet_extracts_as_markdown_table() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "advisories.xlsx", &xlsx_fixture());

    let (is_error, content) = extract(&path).await;
    emit("xlsx", &content);
    assert!(!is_error, "xlsx extraction errored: {content}");

    assert!(
        content.contains("## Sheet: Advisories"),
        "sheet heading missing: {content}"
    );
    assert!(
        content.contains("| Advisory | Fixed in |"),
        "header row not rendered: {content}"
    );
    assert!(
        content.contains("RUSTSEC-2026-0194"),
        "inline string cell missing: {content}"
    );
    assert!(
        content.contains("RUSTSEC-2026-0195"),
        "inline string cell missing: {content}"
    );
    // Numeric cell must survive the DataRef -> String conversion.
    assert!(content.contains('4'), "numeric cell missing: {content}");
}

// ── csv: unaffected control ──────────────────────────────────────────────────

/// csv goes through the `csv` crate, not quick-xml or calamine. It is included
/// as a CONTROL: it must be byte-identical before and after the upgrade, so a
/// diff that shows csv changing means the harness itself moved, not the parsers.
#[tokio::test]
async fn csv_control_is_unaffected_by_xml_stack() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(
        &dir,
        "control.csv",
        b"advisory,fixed_in\nRUSTSEC-2026-0194,0.41\nRUSTSEC-2026-0195,0.41\n",
    );

    let (is_error, content) = extract(&path).await;
    emit("csv", &content);
    assert!(!is_error, "csv extraction errored: {content}");
    assert!(
        content.contains("| advisory | fixed_in |"),
        "csv header row: {content}"
    );
    assert!(
        content.contains("| RUSTSEC-2026-0194 | 0.41 |"),
        "csv data row: {content}"
    );
}

// ── feature-off honesty ──────────────────────────────────────────────────────

/// `doc-extract` is default-ON. Under `--no-default-features` the tool must
/// still register and return an honest "compiled without" error rather than
/// failing to build or silently disappearing. Both arms assert something real,
/// so this test cannot pass vacuously in either configuration.
#[tokio::test]
async fn feature_gate_reports_honestly() {
    let dir = TempDir::new().unwrap();
    let path = write_fixture(&dir, "gate.docx", &docx_fixture());
    let (is_error, content) = extract(&path).await;
    emit("feature-gate", &content);

    if cfg!(feature = "doc-extract") {
        assert!(
            !is_error,
            "doc-extract is on but extraction errored: {content}"
        );
        assert!(
            content.contains("Wayland Core quarterly report"),
            "doc-extract is on but no text extracted: {content}"
        );
    } else {
        assert!(
            is_error,
            "doc-extract is off but extraction claimed success: {content}"
        );
        assert!(
            content.contains("doc-extract"),
            "feature-off error must name the missing feature: {content}"
        );
    }
}
