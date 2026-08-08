//! The read-only media family must be treated consistently by the default
//! tool allow-list.
//!
//! Background: `default_allow_list()` in `wcore-config` is documented as
//! "read-only info-gathering tools -- no destructive action, safe to
//! auto-approve". It listed `vision_analyze` and `transcribe_audio` but not
//! `pdf_extract` or `doc_extract`, even though all four are read-only
//! extractors over a user-supplied path. The consequence, reproduced against
//! the shipped 0.12.26 binary: a user pastes a PDF path, the model correctly
//! calls `pdf_extract`, the approval gate denies it because no interactive
//! user exists, the model falls back to `Read` (which IS allow-listed), `Read`
//! honestly reports "(binary file, N bytes)", and the user gets no answer.
//! With the gate bypassed the very same extractor answers correctly in three
//! turns -- so the extractor was never the defect, the classification was.
//!
//! The tool names below are taken from each tool's own `Tool::name()` rather
//! than hardcoded, so renaming a tool cannot silently un-fix this.
//!
//! The two tests are a vacuity-closing pair. Without the negative control, the
//! positive test could be satisfied by dumping every tool into the allow-list,
//! which would disable the approval gate wholesale.

use wcore_config::config::ToolsConfig;
use wcore_tools::Tool;
use wcore_tools::doc_tool::DocExtractTool;
use wcore_tools::pdf_tool::PdfTool;

#[test]
fn read_only_document_extractors_are_auto_approved() {
    let allow = ToolsConfig::default().allow_list;

    let pdf = PdfTool;
    let doc = DocExtractTool;

    for name in [pdf.name(), doc.name()] {
        assert!(
            allow.iter().any(|t| t == name),
            "`{name}` is a read-only extractor but is absent from the default \
             allow-list, so it is denied whenever no interactive user is \
             present. `Read` is allow-listed and cannot decode these formats, \
             so the user gets no answer at all. Allow-list: {allow:?}"
        );
    }

    // The whole read-only media family must agree. `vision_analyze` and
    // `transcribe_audio` were already present; images and audio worked
    // headlessly while documents did not, which is the inconsistency this
    // test pins.
    for name in ["vision_analyze", "transcribe_audio"] {
        assert!(
            allow.iter().any(|t| t == name),
            "`{name}` must remain auto-approved: the media family is graded \
             as one class. Allow-list: {allow:?}"
        );
    }
}

/// Negative control for the test above.
///
/// Allow-list membership SKIPS the approval gate entirely (see the GHSA-8r7g
/// note in `wcore-config`), so widening it is a security change. This asserts
/// the widening stayed surgical: nothing that writes, executes, or sends may
/// appear, and the list must stay small enough to read at a glance.
#[test]
fn allow_list_admits_nothing_that_writes_executes_or_sends() {
    let allow = ToolsConfig::default().allow_list;

    for forbidden in [
        "Write",
        "Edit",
        "MultiEdit",
        "Bash",
        "NotebookEdit",
        "send_message",
        "Spawn",
        "image",
    ] {
        assert!(
            !allow.iter().any(|t| t == forbidden),
            "`{forbidden}` is not read-only and must never be auto-approved; \
             allow-list membership bypasses the approval gate. \
             Allow-list: {allow:?}"
        );
    }

    assert!(
        allow.len() <= 16,
        "the default allow-list is an auto-approve set and must stay small \
         and auditable; got {} entries: {allow:?}",
        allow.len()
    );
}
