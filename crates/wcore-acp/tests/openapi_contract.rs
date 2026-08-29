//! Wire-shape gate for the public `GET /openapi.json` endpoint.
//!
//! # Why this file exists
//!
//! On 2026-07-29 the `utoipa` 4→5 bump — taken to remove RUSTSEC-2024-0370
//! (`proc-macro-error`) from the dependency tree at source — moved this
//! document from OpenAPI **3.0.3** to **3.1.0**. It was a side effect of a
//! security fix, not a deliberate API change, and it was found by a lane
//! driving the real binary rather than by review.
//!
//! The **shape** changed, not only the version string: nine nullable fields
//! moved from 3.0's `nullable: true` to 3.1's `type: [T, "null"]`. A consumer
//! pinned strictly to 3.0.x cannot read the 3.1 encoding.
//!
//! Nothing outside `wcore-acp` could have failed on that. The pre-existing
//! coverage was a pair of version-**prefix** assertions living in the same
//! crate as the emitter, and both were edited from `"3.0"` to `"3.1"` inside
//! the very commit that made the change — a one-character edit inside a large
//! commit. Nothing anywhere examined the nullable encoding.
//!
//! # What this file adds
//!
//! An external fixture (`tests/fixtures/openapi/rest-openapi-shape.json`) that
//! pins the emitted version **exactly** and pins the nullable-encoding shape by
//! listing every site in each encoding. Moving either now requires a deliberate,
//! reviewable edit to a committed artifact.
//!
//! # Self-test — three assertions, not two
//!
//! A repaired instrument needs a third assertion or the self-test passes on the
//! broken instrument too:
//!
//! 1. [`openapi_shape_matches_committed_fixture`] — **known-positive**: the
//!    document served over a live listener satisfies the fixture.
//! 2. [`shape_checker_rejects_a_3_0_encoded_document`] — **known-negative**: a
//!    3.0-encoded document is REJECTED. The gate can fail.
//! 3. [`old_coverage_is_blind_to_a_pure_encoding_flip`] — **the repair does
//!    something**: a document whose nullable encoding has reverted to 3.0 while
//!    the version string still reads `3.1.0` passes *every* pre-existing
//!    assertion for this endpoint, and is caught only by the new checker.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream};
use serde_json::{Value, json};
use std::pin::Pin;

use wcore_acp::error::AcpError;
use wcore_acp::protocol::{
    MessageEvent, MessageSendRequest, SessionCreateRequest, SessionCreateResponse,
    SessionGetResponse, SessionListResponse, SessionMetadata,
};
use wcore_acp::transport::RestTransport;
use wcore_acp::transport::http::HttpHandler;

// ── The committed fixture ────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/openapi/rest-openapi-shape.json");

// ── Shape scanner ────────────────────────────────────────────────────────

/// The two nullable encodings, located by path, plus the declared version.
#[derive(Debug, Default)]
struct Shape {
    version: String,
    /// Paths of schema nodes written in OpenAPI **3.1** form:
    /// `"type": [T, "null"]`.
    type_array_null: Vec<String>,
    /// Paths of schema nodes written in OpenAPI **3.0** form:
    /// `"nullable": true`.
    nullable_keyword: Vec<String>,
}

fn scan(doc: &Value) -> Shape {
    let mut shape = Shape {
        version: doc["openapi"].as_str().unwrap_or_default().to_string(),
        ..Default::default()
    };
    walk(doc, String::new(), &mut shape);
    shape.type_array_null.sort();
    shape.nullable_keyword.sort();
    shape
}

fn walk(node: &Value, path: String, shape: &mut Shape) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Array(types)) = map.get("type")
                && types.iter().any(|t| t.as_str() == Some("null"))
            {
                shape.type_array_null.push(path.clone());
            }
            if map.get("nullable") == Some(&Value::Bool(true)) {
                shape.nullable_keyword.push(path.clone());
            }
            for (k, v) in map {
                walk(v, format!("{path}/{k}"), shape);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                walk(v, format!("{path}[{i}]"), shape);
            }
        }
        _ => {}
    }
}

/// Compare a scanned shape against the committed fixture.
///
/// Returns every mismatch rather than the first, so a failure names the whole
/// wire delta in one read.
fn check_against_fixture(shape: &Shape) -> Result<(), Vec<String>> {
    let fx: Value = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut bad = Vec::new();

    let want_version = fx["openapi_version"].as_str().expect("openapi_version");
    if shape.version != want_version {
        bad.push(format!(
            "emitted OpenAPI version is {:?}, fixture pins {:?}. This is a wire-visible \
             change to a PUBLIC endpoint — decide whether consumers can absorb it before \
             updating tests/fixtures/openapi/rest-openapi-shape.json",
            shape.version, want_version
        ));
    }

    let want_31: Vec<String> = as_strings(&fx["type_array_null_sites"]);
    let want_30: Vec<String> = as_strings(&fx["nullable_keyword_sites"]);

    if shape.type_array_null != want_31 {
        bad.push(format!(
            "3.1-form `type: [T, \"null\"]` sites changed.\n  emitted ({}): {:#?}\n  fixture ({}): {:#?}",
            shape.type_array_null.len(),
            shape.type_array_null,
            want_31.len(),
            want_31
        ));
    }
    if shape.nullable_keyword != want_30 {
        bad.push(format!(
            "3.0-form `nullable: true` sites changed.\n  emitted ({}): {:#?}\n  fixture ({}): {:#?}",
            shape.nullable_keyword.len(),
            shape.nullable_keyword,
            want_30.len(),
            want_30
        ));
    }

    // The encoding must agree with the declared version. A 3.1 document that
    // emits `nullable: true`, or a 3.0 document that emits `type: [T,"null"]`,
    // is internally inconsistent and unreadable by a conforming client of
    // either version — independent of what the fixture happens to list.
    if shape.version.starts_with("3.1") && !shape.nullable_keyword.is_empty() {
        bad.push(format!(
            "document declares {} but uses 3.0's `nullable: true` at {:?}",
            shape.version, shape.nullable_keyword
        ));
    }
    if shape.version.starts_with("3.0") && !shape.type_array_null.is_empty() {
        bad.push(format!(
            "document declares {} but uses 3.1's `type: [T, \"null\"]` at {:?}",
            shape.version, shape.type_array_null
        ));
    }

    if bad.is_empty() { Ok(()) } else { Err(bad) }
}

fn as_strings(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .expect("fixture site list is an array")
        .iter()
        .map(|s| s.as_str().expect("site is a string").to_string())
        .collect();
    out.sort();
    out
}

// ── The pre-existing coverage, replicated verbatim ───────────────────────

/// Every assertion that examined `GET /openapi.json` **before** this file
/// existed, gathered into one predicate.
///
/// Sources, all at the merge-base:
/// * `transport::rest::tests::get_openapi_json_has_paths_and_resolves_schemas`
///   — version prefix, two keystone paths, no dangling `$ref`, `SessionMetadata`
///   present.
/// * `tests/rest_roundtrip.rs::rest_openapi_doc_served_over_live_listener`
///   — version prefix, `/v1/sessions` present, `SessionMetadata` present.
///
/// (`tests/roles_and_idempotency.rs` also touches the endpoint, but asserts the
/// unauthenticated carve-out — an auth property, not a shape one.)
///
/// This is deliberately the COMPLETE old gate, not a strawman: assertion 3 is
/// only meaningful if the mutant survives all of it.
fn old_coverage_passes(doc: &Value) -> bool {
    let version_ok = doc["openapi"]
        .as_str()
        .is_some_and(|s| s.starts_with("3.1"));
    let paths_ok = doc["paths"]["/v1/sessions"].is_object()
        && doc["paths"]["/v1/sessions/{id}/prompt"].is_object();
    let schemas = match doc["components"]["schemas"].as_object() {
        Some(s) => s,
        None => return false,
    };
    let mut refs = Vec::new();
    collect_refs(doc, &mut refs);
    let refs_ok = refs.iter().all(|r| {
        r.strip_prefix("#/components/schemas/")
            .is_none_or(|name| schemas.contains_key(name))
    });
    version_ok && paths_ok && refs_ok && schemas.contains_key("SessionMetadata")
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "$ref" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    collect_refs(val, out);
                }
            }
        }
        Value::Array(arr) => arr.iter().for_each(|i| collect_refs(i, out)),
        _ => {}
    }
}

// ── Mutator: re-encode a 3.1 document in 3.0's nullable form ─────────────

/// Rewrite every `"type": [T, "null"]` back to `"type": T` + `"nullable": true`.
///
/// This is the exact inverse of what the utoipa bump did, so it reconstructs
/// the pre-fix document shape from the current one.
fn encode_nullables_as_3_0(node: &mut Value) {
    match node {
        Value::Object(map) => {
            // Computed under an immutable borrow, applied after it ends.
            let downgraded: Option<Value> = match map.get("type") {
                Some(Value::Array(types)) if types.iter().any(|t| t.as_str() == Some("null")) => {
                    let rest: Vec<Value> = types
                        .iter()
                        .filter(|t| t.as_str() != Some("null"))
                        .cloned()
                        .collect();
                    Some(if rest.len() == 1 {
                        rest[0].clone()
                    } else {
                        Value::Array(rest)
                    })
                }
                _ => None,
            };
            if let Some(new_type) = downgraded {
                map.insert("type".into(), new_type);
                map.insert("nullable".into(), json!(true));
            }
            for (_, v) in map.iter_mut() {
                encode_nullables_as_3_0(v);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(encode_nullables_as_3_0),
        _ => {}
    }
}

// ── Live document under test ─────────────────────────────────────────────

/// Fetch `/openapi.json` over a real TCP listener, exactly as a consumer would.
async fn serve_and_fetch_openapi() -> Value {
    let app = RestTransport::new(Arc::new(MockHandler)).router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _serve = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    #[allow(clippy::disallowed_methods)] // localhost roundtrip; no proxy/timeout policy needed
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/openapi.json must be publicly served");
    resp.json().await.unwrap()
}

// ── ASSERTION 1 — known-positive ─────────────────────────────────────────

#[tokio::test]
async fn openapi_shape_matches_committed_fixture() {
    let doc = serve_and_fetch_openapi().await;
    let shape = scan(&doc);

    // Guard against a vacuous pass: a document with no nullable fields at all
    // would trivially satisfy an "encoding is consistent" rule.
    assert!(
        !shape.type_array_null.is_empty() || !shape.nullable_keyword.is_empty(),
        "scanner found ZERO nullable fields in either encoding — the instrument \
         is dead, not the document clean"
    );

    if let Err(problems) = check_against_fixture(&shape) {
        panic!(
            "GET /openapi.json no longer matches the committed wire-shape fixture:\n\n{}\n\n\
             See tests/fixtures/openapi/rest-openapi-shape.json before changing it.",
            problems.join("\n\n")
        );
    }
}

// ── ASSERTION 2 — known-negative: the gate can fail ──────────────────────

#[tokio::test]
async fn shape_checker_rejects_a_3_0_encoded_document() {
    let mut doc = serve_and_fetch_openapi().await;

    // Reconstruct the pre-fix (utoipa 4 / OpenAPI 3.0.3) document.
    doc["openapi"] = json!("3.0.3");
    encode_nullables_as_3_0(&mut doc);

    let shape = scan(&doc);
    assert_eq!(
        shape.version, "3.0.3",
        "mutator must actually have changed the version"
    );
    assert!(
        shape.type_array_null.is_empty(),
        "mutator must have removed every 3.1-form site, left: {:?}",
        shape.type_array_null
    );
    // Ten since #305 c2 added `SessionCreateRequest.cwd`; nine before it. The
    // count is spelled out rather than compared to the fixture so a fixture
    // edit and a mutator regression cannot cancel each other out.
    assert_eq!(
        shape.nullable_keyword.len(),
        10,
        "mutator must have produced the ten 3.0-form sites, got {:?}",
        shape.nullable_keyword
    );

    let err = check_against_fixture(&shape)
        .expect_err("a 3.0-encoded document MUST be rejected — otherwise this gate is decorative");
    assert!(
        err.iter().any(|m| m.contains("version")),
        "rejection must name the version change, got: {err:?}"
    );
    assert!(
        err.iter().any(|m| m.contains("nullable: true")),
        "rejection must name the encoding change, got: {err:?}"
    );
}

// ── ASSERTION 3 — the repair does something the old coverage did not ─────

#[tokio::test]
async fn old_coverage_is_blind_to_a_pure_encoding_flip() {
    let mut doc = serve_and_fetch_openapi().await;

    // Control: the unmutated document satisfies the old coverage. Without this
    // the next assertion could pass on a predicate that is simply broken.
    assert!(
        old_coverage_passes(&doc),
        "old-coverage replica must PASS on the real document, or it is not a \
         faithful replica of the pre-existing gate"
    );

    // The mutant: nullable encoding reverted to 3.0's `nullable: true`, while
    // the declared version string is left untouched at 3.1.0. This is the
    // half of the incident that no assertion anywhere examined.
    encode_nullables_as_3_0(&mut doc);
    assert_eq!(
        doc["openapi"],
        json!("3.1.0"),
        "version deliberately unchanged"
    );

    // (a) The COMPLETE pre-existing coverage sails straight through it.
    assert!(
        old_coverage_passes(&doc),
        "expected the old coverage to be blind to a pure encoding flip; if this \
         fails, the old gate was stronger than recorded and this file's premise \
         needs revisiting"
    );

    // (b) The new checker catches it.
    let shape = scan(&doc);
    let err = check_against_fixture(&shape)
        .expect_err("the new shape fixture MUST catch what the old coverage missed");
    assert!(
        err.iter()
            .any(|m| m.contains("declares 3.1.0 but uses 3.0's `nullable: true`")),
        "rejection must name the version/encoding inconsistency, got: {err:?}"
    );

    // (a) AND (b) together are the third assertion: the old instrument would
    // have missed this, and the repaired one does not.
}

// ── Minimal handler so the REST router can be built ──────────────────────

/// In-memory handler. `/openapi.json` is generated from the `ApiDoc` derive and
/// never reaches the handler, but `RestTransport` requires one to build.
struct MockHandler;

#[async_trait]
impl HttpHandler for MockHandler {
    async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        Ok(SessionCreateResponse {
            session_id: "sess-openapi".into(),
            model: req.model,
        })
    }

    async fn list_sessions(&self) -> Result<SessionListResponse, AcpError> {
        Ok(SessionListResponse { sessions: vec![] })
    }

    async fn get_session(&self, session_id: String) -> Result<SessionGetResponse, AcpError> {
        Ok(SessionGetResponse {
            session: SessionMetadata {
                session_id,
                model: None,
                created_at: 1_700_000_000,
                last_activity: 1_700_000_000,
                message_count: 0,
            },
        })
    }

    async fn delete_session(&self, _session_id: String) -> Result<(), AcpError> {
        Ok(())
    }

    async fn send_message(
        &self,
        _req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(Box::pin(stream::empty()))
    }
}
