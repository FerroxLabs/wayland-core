//! G.10 — `ScopedMemoryClient` rejects reads/writes against any
//! partition not granted by the manifest. The wayland-ijfw manifest
//! omits P5; both `read(P5, ..)` and `write(P5, ..)` must fail with
//! `PluginError::PermissionDenied`.

use chrono::Utc;
use wayland_ijfw::MANIFEST_TOML;
use wcore_plugin_api::registry::memory::{MemoryHost, ScopedMemoryClient};
use wcore_plugin_api::{MemoryItem, MemoryQuery, Partition, PluginError, PluginManifest};

#[derive(Default)]
struct PermissiveMemory;
impl MemoryHost for PermissiveMemory {
    fn host_read(&self, _p: Partition, _q: &MemoryQuery) -> Result<Vec<MemoryItem>, String> {
        Ok(Vec::new())
    }
    fn host_write(&mut self, _p: Partition, _i: MemoryItem) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn p5_read_is_denied_per_manifest() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    let mut host = PermissiveMemory;
    let client = ScopedMemoryClient::new(&m, &mut host);

    let err = client
        .read(
            Partition::P5,
            &MemoryQuery {
                q: "anything".to_string(),
                limit: None,
            },
        )
        .expect_err("P5 read must be denied");

    match err {
        PluginError::PermissionDenied { plugin, operation } => {
            assert_eq!(plugin, "wayland-ijfw");
            assert!(
                operation.starts_with("memory.read(P5)"),
                "unexpected operation: {operation}"
            );
        }
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

#[test]
fn p5_write_is_denied_per_manifest() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    let mut host = PermissiveMemory;
    let mut client = ScopedMemoryClient::new(&m, &mut host);

    let err = client
        .write(
            Partition::P5,
            MemoryItem {
                key: "k".into(),
                content: "v".into(),
                metadata: serde_json::json!({}),
                timestamp: Utc::now(),
            },
        )
        .expect_err("P5 write must be denied");

    match err {
        PluginError::PermissionDenied { plugin, operation } => {
            assert_eq!(plugin, "wayland-ijfw");
            assert!(
                operation.starts_with("memory.write(P5)"),
                "unexpected operation: {operation}"
            );
        }
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

#[test]
fn p2_read_is_allowed_per_manifest() {
    let m = PluginManifest::from_toml_str(MANIFEST_TOML).expect("manifest must parse");
    let mut host = PermissiveMemory;
    let client = ScopedMemoryClient::new(&m, &mut host);

    // Sanity-check: P2 is in the readable list, so the call must succeed.
    let items = client
        .read(
            Partition::P2,
            &MemoryQuery {
                q: "x".to_string(),
                limit: None,
            },
        )
        .expect("P2 read should succeed for wayland-ijfw");
    assert!(items.is_empty());
}
