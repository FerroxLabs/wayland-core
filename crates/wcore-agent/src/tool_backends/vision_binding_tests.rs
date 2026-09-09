use super::*;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn selected_vision_key_preserves_custom_and_default_destinations() {
    for (provider, base, expected) in [
        (
            ProviderType::OpenAI,
            "https://router.example.invalid/v1",
            "https://router.example.invalid/v1/chat/completions",
        ),
        (
            ProviderType::FluxRouter,
            "https://router.example.invalid/v1",
            "https://router.example.invalid/v1/chat/completions",
        ),
        (
            ProviderType::OpenAI,
            "",
            "https://api.openai.com/v1/chat/completions",
        ),
        (
            ProviderType::FluxRouter,
            "",
            "https://api.fluxrouter.ai/v1/chat/completions",
        ),
    ] {
        let config = Config {
            provider,
            base_url: base.into(),
            api_key: "fake-selected-key".into(),
            ..Config::default()
        };
        let backend =
            vision_backend_from_openai_env_key(&config, "fake-selected-key".into()).unwrap();
        assert_eq!(backend.endpoint(), expected);
    }
}

#[test]
fn independent_openai_vision_key_preserves_native_destination() {
    for selected_key in ["fake-selected-key", "", "   "] {
        let config = Config {
            provider: ProviderType::FluxRouter,
            base_url: "https://router.example.invalid/v1".into(),
            api_key: selected_key.into(),
            ..Config::default()
        };
        let backend =
            vision_backend_from_openai_env_key(&config, "fake-independent-key".into()).unwrap();
        assert_eq!(
            backend.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(backend.backend_id(), "openai");
    }
}

/// Exercise the production resolver with process-isolated credentials. Even a
/// regression to a native host goes to the fixture proxy, never the Internet.
#[tokio::test]
async fn vision_env_binding_uses_configured_route() {
    const CHILD: &str = "WCORE_VISION_BINDING_CASE";
    if let Ok(case) = std::env::var(CHILD) {
        let config = Config {
            provider: match case.as_str() {
                "openai" => ProviderType::OpenAI,
                "unsupported" | "anthropic_precedence" => ProviderType::Anthropic,
                _ => ProviderType::FluxRouter,
            },
            api_key: "fake-selected-key".into(),
            base_url: if case == "malformed" {
                "not-a-url".into()
            } else {
                std::env::var("WCORE_VISION_BINDING_BASE").unwrap()
            },
            ..Config::default()
        };
        let ledger = wcore_tools::media_cost::MediaCostLedger::shared();
        let accounting = MediaAccounting::new(Arc::clone(&ledger), Default::default());
        let backend = build_vision_backend_with_accounting(&config, &accounting);
        if matches!(case.as_str(), "unsupported" | "malformed") {
            assert!(
                backend.is_none(),
                "invalid selected binding must fail closed"
            );
            return;
        }
        if case == "anthropic_precedence" {
            assert!(
                backend.is_some(),
                "explicit Anthropic must retain precedence"
            );
            return;
        }
        let outcome = backend
            .expect("configured vision must resolve")
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        assert!(
            matches!(outcome, wcore_tools::vision_tools::VisionOutcome::Ok { analysis } if analysis == "fixture vision")
        );
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1, "selected arm must retain accounting");
        assert_eq!(records[0].cost_usd, Some(0.004));
        return;
    }

    for case in [
        "openai",
        "flux",
        "unsupported",
        "malformed",
        "anthropic_precedence",
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer fake-selected-key"))
            .and(body_partial_json(
                serde_json::json!({"model": "fixture-vision-model"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "fixture vision"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "cost_usd": 0.004}
            })))
            .expect(if matches!(case, "openai" | "flux") {
                1
            } else {
                0
            })
            .mount(&server)
            .await;
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "tool_backends::vision_binding_tests::vision_env_binding_uses_configured_route",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD, case)
            .env("OPENAI_API_KEY", "fake-selected-key")
            .env("WAYLAND_VISION_MODEL", "fixture-vision-model")
            .env("WCORE_VISION_BINDING_BASE", server.uri())
            .env("HTTP_PROXY", server.uri())
            .env("HTTPS_PROXY", server.uri())
            .env("NO_PROXY", "127.0.0.1,localhost,::1")
            .kill_on_drop(true);
        // Windows needs these OS paths even with operator credentials removed.
        for name in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(name) {
                child.env(name, value);
            }
        }
        if case == "anthropic_precedence" {
            child.env("ANTHROPIC_API_KEY", "fake-independent-anthropic-key");
        }
        let output = tokio::time::timeout(std::time::Duration::from_secs(15), child.output())
            .await
            .expect("isolated resolver exceeded fixture budget")
            .expect("start isolated resolver");
        assert!(
            output.status.success(),
            "{case} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        server.verify().await;
    }
}
