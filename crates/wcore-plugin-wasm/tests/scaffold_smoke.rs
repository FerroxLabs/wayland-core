#[test]
fn crate_compiles_and_error_displays() {
    use wcore_plugin_wasm::error::WasmPluginError;
    let err = WasmPluginError::Timeout;
    assert!(err.to_string().contains("timeout"));
}
