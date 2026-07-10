//! Integration tests for the `<provider>:<role>` model short-form
//! (closes debt B.4 / HC-3-followup).
//!
//! Black-box: feeds `CliArgs { model: Some("bedrock:sonnet"), ... }` to
//! `Config::resolve` and checks that the resolved `Config.model` is the
//! canonical Bedrock literal — i.e. the engine never sees the short form.

use serial_test::serial;
use tempfile::TempDir;
use wcore_config::config::{CliArgs, Config};
use wcore_types::model_aliases::{
    BEDROCK_HAIKU, BEDROCK_OPUS, BEDROCK_SONNET, VERTEX_GEMINI_FLASH, VERTEX_GEMINI_PRO,
    VERTEX_OPUS, VERTEX_SONNET,
};

/// Build a CliArgs for tests that uses an isolated empty project dir so we
/// don't pick up the host's `.wayland-core.toml`.
fn args(provider: &str, model: &str, project_dir: &TempDir) -> CliArgs {
    CliArgs {
        provider: Some(provider.into()),
        api_key: Some("test-key".into()),
        base_url: None,
        model: Some(model.into()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(project_dir.path().to_path_buf()),
    }
}

#[test]
fn bedrock_sonnet_short_form_expands_to_canonical_literal() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config::resolve(&args("bedrock", "bedrock:sonnet", &tmp)).unwrap();
    assert_eq!(
        cfg.model, BEDROCK_SONNET,
        "`--model bedrock:sonnet` must expand to the full Bedrock literal"
    );
}

#[test]
fn bedrock_opus_and_haiku_short_forms_expand() {
    let tmp = TempDir::new().unwrap();
    let opus = Config::resolve(&args("bedrock", "bedrock:opus", &tmp)).unwrap();
    assert_eq!(opus.model, BEDROCK_OPUS);
    let haiku = Config::resolve(&args("bedrock", "bedrock:haiku", &tmp)).unwrap();
    assert_eq!(haiku.model, BEDROCK_HAIKU);
}

#[test]
fn vertex_short_forms_expand() {
    let tmp = TempDir::new().unwrap();
    let sonnet = Config::resolve(&args("vertex", "vertex:sonnet", &tmp)).unwrap();
    assert_eq!(sonnet.model, VERTEX_SONNET);
    let opus = Config::resolve(&args("vertex", "vertex:opus", &tmp)).unwrap();
    assert_eq!(opus.model, VERTEX_OPUS);
    let pro = Config::resolve(&args("vertex", "vertex:gemini-pro", &tmp)).unwrap();
    assert_eq!(pro.model, VERTEX_GEMINI_PRO);
    let flash = Config::resolve(&args("vertex", "vertex:gemini-flash", &tmp)).unwrap();
    assert_eq!(flash.model, VERTEX_GEMINI_FLASH);
}

#[test]
fn full_literal_passes_through_unchanged() {
    // Regression guard: a user who already pins a fully-qualified Bedrock ID
    // (perhaps a `-v2:0` revision the alias table hasn't picked up yet) must
    // not have it silently rewritten.
    let tmp = TempDir::new().unwrap();
    let pinned = "anthropic.claude-sonnet-4-6-20251015-v2:0";
    let cfg = Config::resolve(&args("bedrock", pinned, &tmp)).unwrap();
    assert_eq!(cfg.model, pinned);
}

#[test]
fn unknown_role_passes_through_unchanged() {
    // `bedrock:gemini` is not a real Bedrock role — flow through so the
    // provider request surfaces the upstream "model not found" error
    // rather than the alias layer guessing.
    let tmp = TempDir::new().unwrap();
    let cfg = Config::resolve(&args("bedrock", "bedrock:gemini", &tmp)).unwrap();
    assert_eq!(cfg.model, "bedrock:gemini");
}

#[test]
#[serial]
fn default_bedrock_when_no_model_uses_alias() {
    // No --model flag, no inline config: `default_model_for(Bedrock)` must
    // pick the alias, not the deprecated 20250514 literal that was hard-
    // coded before this change.
    //
    // ISOLATION: `Config::resolve` reads the *global* config from
    // `wayland_config_dir()/config.toml` (macOS: ~/Library/Application
    // Support/wayland-core/config.toml).  On a developer host that has
    // `model = "<something>"` in that file the default-model path is
    // never exercised and the test fails spuriously.  Point `WAYLAND_HOME`
    // at an empty TempDir so the global config path resolves to a
    // non-existent file and `default_model_for()` wins.  This test and
    // `default_vertex_when_no_model_uses_alias` both mutate the *same*
    // WAYLAND_HOME key, so under `cargo test` (all tests of a binary share
    // one process across threads) they would race — a data race on the
    // environment (unsafe in edition 2024). `#[serial]` serializes them.
    let tmp = TempDir::new().unwrap();
    // SAFETY: `#[serial]` guarantees no other `#[serial]` test mutates the
    // environment concurrently, so this write/remove pair is race-free.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("WAYLAND_HOME", tmp.path());
    }
    let cfg = Config::resolve(&CliArgs {
        provider: Some("bedrock".into()),
        api_key: Some("test-key".into()),
        base_url: None,
        model: None,
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(tmp.path().to_path_buf()),
    })
    .unwrap();
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var("WAYLAND_HOME");
    }
    assert_eq!(cfg.model, BEDROCK_SONNET);
    assert!(
        !cfg.model.contains("20250514"),
        "Bedrock default must not pin deprecated 20250514 date"
    );
}

#[test]
#[serial]
fn default_vertex_when_no_model_uses_alias() {
    // See isolation note in `default_bedrock_when_no_model_uses_alias` above —
    // same problem, same fix: redirect WAYLAND_HOME so the host global config
    // is not loaded and `default_model_for(Vertex)` wins.
    let tmp = TempDir::new().unwrap();
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("WAYLAND_HOME", tmp.path());
    }
    let cfg = Config::resolve(&CliArgs {
        provider: Some("vertex".into()),
        api_key: Some("test-key".into()),
        base_url: None,
        model: None,
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(tmp.path().to_path_buf()),
    })
    .unwrap();
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var("WAYLAND_HOME");
    }
    assert_eq!(cfg.model, VERTEX_SONNET);
    assert!(
        !cfg.model.contains("20250514"),
        "Vertex default must not pin deprecated 20250514 date"
    );
}
