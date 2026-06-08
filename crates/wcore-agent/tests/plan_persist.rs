//! F16 plan persistence tests — roundtrip, missing, corrupted, source_product.

use wcore_agent::plan::persist::{load_plan_json, save_plan_json};

#[test]
fn save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = save_plan_json("sess-1", "## Plan\n\n1. Step", Some(tmp.path())).unwrap();
    assert!(path.exists());

    let loaded = load_plan_json("sess-1", Some(tmp.path())).unwrap();
    assert!(loaded.is_some());
    let p = loaded.unwrap();
    assert_eq!(p.session_id, "sess-1");
    assert_eq!(p.plan_text, "## Plan\n\n1. Step");
}

#[test]
fn load_returns_none_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let loaded = load_plan_json("nope", Some(tmp.path())).unwrap();
    assert!(loaded.is_none());
}

#[test]
fn load_returns_err_when_corrupted() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("plans")).unwrap();
    std::fs::write(tmp.path().join("plans/sess-2.json"), "not json").unwrap();
    let result = load_plan_json("sess-2", Some(tmp.path()));
    assert!(result.is_err());
}

#[test]
fn source_product_field_is_present() {
    let tmp = tempfile::tempdir().unwrap();
    save_plan_json("sess-3", "x", Some(tmp.path())).unwrap();
    let p = load_plan_json("sess-3", Some(tmp.path())).unwrap().unwrap();
    assert_eq!(p.source_product, "wayland-core");
}

#[test]
fn ts_unix_is_populated() {
    let tmp = tempfile::tempdir().unwrap();
    save_plan_json("sess-4", "x", Some(tmp.path())).unwrap();
    let p = load_plan_json("sess-4", Some(tmp.path())).unwrap().unwrap();
    // Far enough in the future that 0 would clearly fail; far enough in the
    // past that a wildly-wrong now() would also fail.
    assert!(
        p.ts_unix > 1_700_000_000,
        "ts_unix should be a recent epoch"
    );
}
