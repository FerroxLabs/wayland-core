use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};

#[allow(clippy::disallowed_methods)]
async fn post(base_url: &str, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base_url}/v1/chat/completions"))
        .header(
            "authorization",
            "Bearer fixture-secret-must-not-be-recorded",
        )
        .json(&body)
        .send()
        .await
        .expect("fixture request")
}

fn request(model: &str) -> serde_json::Value {
    json!({
        "model": model,
        "stream": true,
        "messages": [{"role": "user", "content": "deterministic prompt"}]
    })
}

#[tokio::test]
async fn success_is_valid_sse_and_records_one_redacted_request() {
    let script = OpenAiFixtureScript::new([OpenAiStep::text("fixture answer")]);
    let first = script.start().await.expect("first fixture");
    let first_digest = first.fixture_sha256().to_string();
    let first_base_url = first.base_url().to_string();

    let response = post(first.base_url(), request("fixture-chat-v1")).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = response.text().await.expect("SSE body");
    assert!(body.contains("fixture answer"));
    assert!(body.contains("\"finish_reason\":\"stop\""));
    assert!(body.contains("data: [DONE]"));

    let observation = first.shutdown().await.expect("clean fixture shutdown");
    assert!(observation.complete());
    assert_eq!(observation.requests.len(), 1);
    assert_eq!(observation.attempts(), 1);
    assert!(observation.injected_faults().is_empty());
    assert_eq!(observation.requests[0].sequence, 1);
    assert_eq!(observation.requests[0].method, "POST");
    assert_eq!(observation.requests[0].path, "/v1/chat/completions");
    assert_eq!(
        observation.requests[0].model.as_deref(),
        Some("fixture-chat-v1")
    );
    assert_eq!(
        observation.requests[0].body_sha256,
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&request("fixture-chat-v1")).unwrap())
        )
    );
    assert!(!format!("{observation:?}").contains("fixture-secret-must-not-be-recorded"));

    let second = script.start().await.expect("second fixture");
    assert_ne!(first_base_url, second.base_url());
    assert_eq!(first_digest, second.fixture_sha256());
    let _ = post(second.base_url(), request("fixture-chat-v1")).await;
    assert!(second.shutdown().await.unwrap().complete());
}

#[tokio::test]
async fn fault_steps_are_fifo_and_extra_requests_fail_closed() {
    let script = OpenAiFixtureScript::new([
        OpenAiStep::http_error(503),
        OpenAiStep::rate_limited(10),
        OpenAiStep::truncated("partial"),
        OpenAiStep::duplicate_text("repeat"),
    ]);
    let fixture = script.start().await.expect("fault fixture");

    assert_eq!(
        post(fixture.base_url(), request("fixture-chat-v1"))
            .await
            .status(),
        reqwest::StatusCode::SERVICE_UNAVAILABLE
    );
    let rate_limited = post(fixture.base_url(), request("fixture-chat-v1")).await;
    assert_eq!(
        rate_limited.status(),
        reqwest::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        rate_limited.json::<serde_json::Value>().await.unwrap()["retry_after_ms"],
        10
    );
    let truncated = post(fixture.base_url(), request("fixture-chat-v1"))
        .await
        .text()
        .await
        .unwrap();
    assert!(truncated.contains("partial"));
    assert!(!truncated.contains("\"finish_reason\":\"stop\""));
    assert!(!truncated.contains("[DONE]"));

    let duplicate = post(fixture.base_url(), request("fixture-chat-v1"))
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(duplicate.matches("\"content\":\"repeat\"").count(), 2);
    assert!(duplicate.contains("data: [DONE]"));

    let extra = post(fixture.base_url(), request("fixture-chat-v1")).await;
    assert_eq!(extra.status(), reqwest::StatusCode::CONFLICT);

    let observation = fixture.shutdown().await.expect("fixture shutdown");
    assert_eq!(observation.requests.len(), 5);
    assert_eq!(observation.attempts(), 5);
    assert_eq!(
        observation.injected_faults(),
        ["http_503", "rate_limited", "truncated_stream"]
    );
    assert!(!observation.complete());
    assert_eq!(observation.violations, ["unexpected_request"]);
}

#[tokio::test]
async fn stalled_response_is_cancelled_by_bounded_shutdown() {
    let script = OpenAiFixtureScript::new([OpenAiStep::stall_before_headers(10_000)]);
    let fixture = script.start().await.expect("stall fixture");
    let base_url = fixture.base_url().to_string();
    let request_task =
        tokio::spawn(async move { post(&base_url, request("fixture-chat-v1")).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        if fixture.observation().requests.len() == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture saw no request"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let observation = tokio::time::timeout(Duration::from_millis(500), fixture.shutdown())
        .await
        .expect("shutdown must not wait for scripted stall")
        .expect("shutdown result");
    assert_eq!(observation.requests.len(), 1);
    request_task.abort();
}
