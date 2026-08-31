//! FerroxLabs/wayland#1237 c2 and c4 — the terminal error exits of the run
//! loop, and the refusal to guess the router-versus-provider split.

use wcore_agent::engine::AgentError;
use wcore_protocol::events::FailureCategory;
use wcore_providers::ProviderError;

/// c2 — every terminal error exit of `AgentEngine::run` names its category.
///
/// The exits ARE the variants of `AgentError`: `run` returns
/// `Result<AgentResult, AgentError>`, so there is no terminal error exit that
/// is not one of these. This drives each one rather than sampling.
#[test]
fn every_terminal_run_exit_names_its_category() {
    let exits: Vec<(AgentError, FailureCategory)> = vec![
        (
            AgentError::ContextTooLong {
                input_tokens: 190_000,
                limit: 180_000,
            },
            FailureCategory::ContextLimit,
        ),
        (
            AgentError::SessionAuthority("journal is not initialized".to_string()),
            FailureCategory::LocalWayland,
        ),
        (AgentError::UserAborted, FailureCategory::LocalWayland),
        (
            AgentError::ApiError("upstream 503".to_string()),
            FailureCategory::Unknown,
        ),
        (
            AgentError::Provider(ProviderError::Api {
                status: 503,
                message: "{\"error\":{\"message\":\"service unavailable\"}}".to_string(),
            }),
            FailureCategory::Unknown,
        ),
    ];
    for (error, expected) in &exits {
        assert_eq!(
            error.failure_category(),
            *expected,
            "{error} must report {expected:?}"
        );
    }
    // Known-positive control: the assertion above can fail.
    assert_ne!(
        AgentError::UserAborted.failure_category(),
        FailureCategory::ContextLimit,
        "control: failure_category does not return one constant"
    );
    // All three categories core CAN decide are actually reachable from a
    // terminal exit or from a typed frame; a classifier that only ever says
    // `unknown` would satisfy the loop above by accident otherwise.
    assert!(
        exits
            .iter()
            .any(|(_, c)| *c == FailureCategory::ContextLimit),
        "the context-limit exit is the one #388 was filed about"
    );
}

/// c2's control — an unclassified exit cannot be added silently.
///
/// Two things stop it, and this asserts the one a plain `cargo test` can see.
/// The compiler refuses a non-exhaustive match, so a NEW `AgentError` variant
/// cannot reach `failure_category` unhandled. The remaining move is to add a
/// `_ =>` arm and let it swallow the new exit — which `#[deny(clippy::
/// wildcard_enum_match_arm)]` on the function rejects under the workspace's
/// `-D warnings` clippy gate, and which this asserts directly so the property
/// does not depend on clippy being run.
///
/// Both halves are derived from the source rather than listed here: the
/// variant names come out of the enum's own declaration, so a variant added
/// tomorrow is checked by the same assertion without anyone updating it.
#[test]
fn the_exit_classifier_has_no_default_arm_and_names_every_variant() {
    let engine_src = include_str!("../src/engine.rs");

    let enum_body = engine_src
        .split("pub enum AgentError {")
        .nth(1)
        .expect("AgentError is declared in engine.rs")
        .split("\n}\n")
        .next()
        .expect("the enum body ends");
    let variants: Vec<&str> = enum_body
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.chars().next().is_some_and(char::is_uppercase) && !line.starts_with("//")
        })
        .map(|line| {
            line.split(['(', '{', ','])
                .next()
                .expect("a variant name")
                .trim()
        })
        .collect();
    assert!(
        variants.len() >= 5,
        "control: the variant scrape found {} variants, so it is reading the \
         enum and not an empty string: {variants:?}",
        variants.len()
    );

    let classifier = engine_src
        .split("pub fn failure_category(&self)")
        .nth(1)
        .expect("the classifier exists")
        .split("\n    }\n")
        .next()
        .expect("the classifier body ends");
    for variant in &variants {
        assert!(
            classifier.contains(&format!("AgentError::{variant}")),
            "{variant} is a terminal exit of the run loop with no category"
        );
    }
    assert!(
        !classifier.contains("_ =>"),
        "a default arm here would report the next terminal exit as something \
         it is not; that is the failure #1237 c2 exists to prevent"
    );
    assert!(
        engine_src.contains("#[deny(clippy::wildcard_enum_match_arm)]"),
        "the clippy gate must refuse the default arm too, not only this test"
    );
}

/// c4 — a bare non-2xx from an OpenAI-shaped endpoint is NOT reported as a
/// provider rate limit or as a router failure.
///
/// `openai.rs` maps any unrecognised non-2xx to `ProviderError::Api { status,
/// message }` — that is the shape under test. Core cannot tell whether the
/// 503 came from the model provider or from the router in front of it
/// (wayland#1184): both are the same status from the same host. So it reports
/// `unknown`, and `unknown` is a claim a host can act on — ask the router —
/// rather than a guess it would have to distrust.
#[test]
fn a_bare_non_2xx_from_an_openai_shaped_endpoint_is_not_classified() {
    for status in [500u16, 502, 503, 529] {
        let error = AgentError::Provider(ProviderError::Api {
            status,
            message: "{\"error\":{\"message\":\"upstream error\",\"type\":\"server_error\"}}"
                .to_string(),
        });
        let category = error.failure_category();
        assert_eq!(
            category,
            FailureCategory::Unknown,
            "HTTP {status} from an OpenAI-shaped endpoint is not decidable here"
        );
        let wire = serde_json::to_string(&category).expect("serialises");
        assert!(
            !wire.contains("rate_limit") && !wire.contains("router_failure"),
            "HTTP {status} came back as {wire}"
        );
    }
    // A 429 is the case a classifier is most tempted to guess on, because the
    // provider layer DOES have a typed variant for it. It is still not
    // decidable which side of the router rate-limited, so it is still unknown.
    assert_eq!(
        AgentError::Provider(ProviderError::RateLimited {
            retry_after_ms: 2_000
        })
        .failure_category(),
        FailureCategory::Unknown,
        "429 is exactly the rate-limit-vs-router ambiguity #1184 owns"
    );
    // Known-positive control: an exit core CAN decide is decided, so the
    // assertions above are not passing because everything returns unknown.
    assert_eq!(
        AgentError::ContextTooLong {
            input_tokens: 190_000,
            limit: 180_000,
        }
        .failure_category(),
        FailureCategory::ContextLimit,
        "control: a decidable exit IS decided"
    );
}
