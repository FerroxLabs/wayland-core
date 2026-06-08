//! Validate WIT syntax without requiring the wasm32 toolchain
//! (cross-audit finding N12).
#[test]
fn tool_wit_parses() {
    use wit_parser::Resolve;
    let mut r = Resolve::new();
    r.push_path("wit").expect("WIT files parse");
}
