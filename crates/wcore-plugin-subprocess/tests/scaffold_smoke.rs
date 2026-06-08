#[test]
fn crate_compiles_and_envelope_roundtrips() {
    use wcore_plugin_subprocess::rpc::SubprocessVerb;
    let init = SubprocessVerb::Init;
    let json = serde_json::to_string(&init).unwrap();
    assert!(json.contains("init"));
    let back: SubprocessVerb = serde_json::from_str(&json).unwrap();
    assert_eq!(back, init);
}
