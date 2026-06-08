//! W3→W4 hand-off: RepoMapTool wraps wcore_repomap::RepoMap::build and
//! render_compact behind the Tool trait. The agent invokes it like any
//! other built-in.

use std::fs;

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::repomap::RepoMapTool;

fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(
        tmp.path().join("src/lib.rs"),
        "pub trait LlmProvider {}\npub fn run() {}\n",
    )
    .unwrap();
    tmp
}

#[tokio::test]
async fn repomap_tool_returns_rendered_compact_view() {
    let tmp = fixture();
    let tool = RepoMapTool::new(tmp.path().to_path_buf());

    let result = tool.execute(json!({"query": "LlmProvider"})).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("LlmProvider"),
        "expected query hit in compact render; got {}",
        result.content
    );

    // With no query, the full compact render includes the file path.
    let full = tool.execute(json!({})).await;
    assert!(!full.is_error, "{}", full.content);
    assert!(
        full.content.contains("lib.rs"),
        "expected file mention in unfiltered render; got {}",
        full.content
    );
    assert!(
        full.content.contains("LlmProvider"),
        "expected symbol in unfiltered render; got {}",
        full.content
    );
}

#[tokio::test]
async fn repomap_tool_schema_advertises_query_param() {
    let tmp = fixture();
    let tool = RepoMapTool::new(tmp.path().to_path_buf());
    let schema = tool.input_schema();
    let props = schema.get("properties").expect("schema has properties");
    assert!(props.get("query").is_some(), "schema must expose `query`");
    assert!(
        props.get("file_limit").is_some(),
        "schema must expose `file_limit`"
    );
    assert!(
        props.get("symbol_limit").is_some(),
        "schema must expose `symbol_limit`"
    );
}

#[tokio::test]
async fn repomap_tool_is_read_only_concurrency_safe() {
    let tmp = fixture();
    let tool = RepoMapTool::new(tmp.path().to_path_buf());
    assert!(tool.is_concurrency_safe(&json!({"query": "x"})));
}

#[test]
fn script_allow_list_includes_repomap() {
    // Audit HIGH-1 fix: RepoMap must be in Script's ALLOW_LIST since the
    // RepoMap tool is read-only and shape-bounded, semantically equivalent
    // to Grep.
    assert!(
        wcore_tools::script::ALLOW_LIST.contains(&"RepoMap"),
        "RepoMap must be allow-listed for Script"
    );
}
