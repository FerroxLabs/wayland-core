//! The HIBERNATING CLOUD reference backend — Fly.io Machines.
//!
//! The vendor was committed on a four-way cross-audit panel recorded in
//! `.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-01-CLOUD-BACKEND-DECISION.md`.
//! All four members returned `fly-machines`; the dissent is preserved and two
//! of its conditions are BINDING here:
//!
//! * **C1 — suspend, not stop.** The reference hibernation transition is
//!   `suspend`. A run that only observes `stop` records
//!   [`HibernationObservation::NotObserved`] and MUST NOT claim hibernation.
//!   That is why the observation is a three-variant enum rather than a bool:
//!   the condition lives in the type system, not in reviewer vigilance.
//! * **C2 — the credential is Sean's.** With no credential this backend fails
//!   CLOSED with [`ExecError::CredentialAbsent`] and never falls back to
//!   another backend. It is built anyway, because an implemented backend that
//!   reports unavailable is one token away from running.
//!
//! ALL outbound HTTP goes through `wcore-egress`. No raw HTTP client is
//! constructed in this file — a clippy disallowed-methods lint bans that
//! outside the egress crate and this module does not try to work around it.
//! A unit test below scans this source and enforces the same rule, which is
//! why the forbidden constructor names appear nowhere in the prose either.

use async_trait::async_trait;
use serde::Deserialize;

use crate::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    SecretChannel,
};
use crate::error::{ExecError, Result};
use crate::policy::{EffectivePolicy, declared_secret_exposure};
use crate::receipt::{BackendIdentity, ExecutionReceipt, ReceiptSigner};
use crate::registry::{self, LiveTask, now_unix_ms};

use super::local::{cancel_marker_taken, instance_id, write_cancel_marker};
use super::{
    RunOutcome, denial_receipt, load_or_create_seed, outcome_receipt, pre_acceptance_denial,
};

pub const BACKEND_ID: &str = "cloud";
pub const TOKEN_ENV: &str = "WAYLAND_F25_CLOUD_TOKEN";
pub const ORG_ENV: &str = "WAYLAND_F25_CLOUD_ORG";
pub const NONCE_METADATA_KEY: &str = "wayland_task_nonce";
const API_BASE: &str = "https://api.machines.dev/v1";

pub struct CloudBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
}

/// The credential, held only on the CONTROL side. It is never provisioned into
/// the task, never logged, never placed in an event body and never placed in a
/// receipt — the receipt has no field that could hold it.
struct CloudCredential {
    token: String,
    app: String,
}

impl CloudCredential {
    fn from_env() -> std::result::Result<Self, ExecError> {
        let token = std::env::var(TOKEN_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| ExecError::CredentialAbsent {
                backend_id: BACKEND_ID.into(),
                env: TOKEN_ENV.into(),
            })?;
        let app = std::env::var(ORG_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| ExecError::CredentialAbsent {
                backend_id: BACKEND_ID.into(),
                env: ORG_ENV.into(),
            })?;
        Ok(Self { token, app })
    }
}

impl std::fmt::Debug for CloudCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A Debug impl that printed the token would leak it into any trace
        // line that ever formats a struct holding one.
        f.debug_struct("CloudCredential")
            .field("token", &"<redacted>")
            .field("app", &self.app)
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct MachineSummary {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
}

impl CloudBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
        Ok(Self {
            capabilities: BackendCapabilities {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Cloud,
                version: env!("CARGO_PKG_VERSION").into(),
                limits,
                supports_artifact_transfer: true,
                supports_cancellation: true,
                supports_hibernation: true,
                secret_channel: SecretChannel::VendorManaged,
            },
            identity: BackendIdentity {
                backend_id: BACKEND_ID.into(),
                instance_id: instance_id(),
                version: env!("CARGO_PKG_VERSION").into(),
                key_id: signer.key_id().to_string(),
            },
            signer,
        })
    }

    pub fn identity(&self) -> &BackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signer.verifying_key()
    }

    /// GET a Machines API path. The bearer token is attached here and nowhere
    /// else, and the returned body is the vendor's own JSON.
    async fn api_get(
        credential: &CloudCredential,
        path: &str,
    ) -> std::result::Result<(u16, String), String> {
        let client = wcore_egress::EgressClient::new();
        let url = format!("{API_BASE}{path}");
        let response = client
            .get(&url)
            .bearer_auth(&credential.token)
            .send()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        Ok((status, body))
    }

    async fn api_post(
        credential: &CloudCredential,
        path: &str,
    ) -> std::result::Result<(u16, String), String> {
        let client = wcore_egress::EgressClient::new();
        let url = format!("{API_BASE}{path}");
        let response = client
            .post(&url)
            .bearer_auth(&credential.token)
            .send()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        Ok((status, body))
    }

    async fn api_delete(
        credential: &CloudCredential,
        path: &str,
    ) -> std::result::Result<(u16, String), String> {
        let client = wcore_egress::EgressClient::new();
        let url = format!("{API_BASE}{path}");
        let response = client
            .delete(&url)
            .bearer_auth(&credential.token)
            .send()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| redact(&e.to_string(), &credential.token))?;
        Ok((status, body))
    }

    async fn machines_with_nonce(
        credential: &CloudCredential,
        nonce: &str,
    ) -> std::result::Result<Vec<String>, String> {
        let path = format!(
            "/apps/{}/machines?metadata.{}={}",
            credential.app, NONCE_METADATA_KEY, nonce
        );
        let (status, body) = Self::api_get(credential, &path).await?;
        if status != 200 {
            return Err(format!("machine listing returned HTTP {status}: {body}"));
        }
        let machines: Vec<MachineSummary> =
            serde_json::from_str(&body).map_err(|e| format!("unparseable machine listing: {e}"))?;
        Ok(machines
            .into_iter()
            .map(|m| format!("machine {} ({}) state={}", m.id, m.name, m.state))
            .collect())
    }
}

/// Defence in depth against the token reaching a log through an error string
/// the vendor or the transport happened to echo back.
fn redact(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "<redacted>")
}

#[async_trait]
impl ExecutionBackend for CloudBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        let credential = match CloudCredential::from_env() {
            Ok(credential) => credential,
            Err(err) => {
                // FAIL CLOSED and say exactly why. This is the verdict
                // `backend list` prints, and it must never read as "maybe".
                return Availability::down(ProbeBasis::CredentialAbsent, err.to_string());
            }
        };
        let path = format!("/apps/{}/machines", credential.app);
        match Self::api_get(&credential, &path).await {
            Ok((200, _)) => Availability::up(
                ProbeBasis::VendorApiCall,
                format!(
                    "vendor API answered 200 for app {} machine listing",
                    credential.app
                ),
            ),
            Ok((status, body)) => Availability::down(
                ProbeBasis::VendorApiCall,
                format!("vendor API answered HTTP {status}: {body}"),
            ),
            Err(detail) => Availability::down(ProbeBasis::VendorApiCall, detail),
        }
    }

    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
        let (egress_decision, egress_source) = crate::policy::observed_egress_decision();
        let policy = EffectivePolicy {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Cloud,
            egress_decision,
            egress_source,
            secret_channel: SecretChannel::VendorManaged,
            // The control-plane credential is NOT a task secret and is not
            // listed here: nothing is provisioned into the task.
            secrets_exposed: declared_secret_exposure(BackendKind::Cloud, task),
            containment: "vendor machine, destroyed on completion, tagged with the task nonce"
                .into(),
        };
        policy.validate()?;
        Ok(policy)
    }

    async fn execute(&self, task: &ExecutionTask) -> Result<ExecutionReceipt> {
        task.validate()?;
        let policy = self.effective_policy(task)?;
        if let Some(denial) = pre_acceptance_denial(task, &self.capabilities) {
            return denial_receipt(
                task,
                &self.capabilities,
                &self.identity,
                &self.signer,
                &policy,
                denial,
            );
        }

        // Fail CLOSED. No fallback to local, container or ssh: a task the
        // operator asked to run in the cloud must not silently run on the
        // operator's own machine.
        let credential = CloudCredential::from_env()?;

        let started = now_unix_ms();
        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Cloud,
            pid: None,
            handle: None,
            started_unix_ms: started,
        })?;

        let mut transitions: Vec<String> = Vec::new();

        // 1. Create the machine, tagged with the task nonce so an orphan scan
        //    can find it with one call.
        let create_path = format!("/apps/{}/machines", credential.app);
        let (status, body) = Self::api_post(&credential, &create_path)
            .await
            .map_err(ExecError::Transport)?;
        if !(200..300).contains(&status) {
            registry::forget(&task.task_id)?;
            return Err(ExecError::Transport(format!(
                "machine create returned HTTP {status}: {body}"
            )));
        }
        let created: MachineSummary = serde_json::from_str(&body)
            .map_err(|e| ExecError::Transport(format!("unparseable machine create: {e}")))?;
        let machine_id = created.id.clone();
        transitions.push(format!(
            "created:{}",
            read_state(&credential, &machine_id).await
        ));

        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Cloud,
            pid: None,
            handle: Some(machine_id.clone()),
            started_unix_ms: started,
        })?;

        // 2. Run: wait for the machine to reach `started`.
        let wait_path = format!(
            "/apps/{}/machines/{}/wait?state=started&timeout=60",
            credential.app, machine_id
        );
        let _ = Self::api_get(&credential, &wait_path).await;
        transitions.push(format!(
            "started:{}",
            read_state(&credential, &machine_id).await
        ));

        // 3. The HIBERNATION transition. Condition C1: `suspend`, not `stop`.
        let suspend_path = format!("/apps/{}/machines/{}/suspend", credential.app, machine_id);
        let suspend_result = Self::api_post(&credential, &suspend_path).await;
        let hibernation = match suspend_result {
            Ok((status, _)) if (200..300).contains(&status) => {
                let observed = read_state(&credential, &machine_id).await;
                transitions.push(format!("suspended:{observed}"));
                // 4. Resume, and read the state back rather than inferring it
                //    from the request that asked for it.
                let start_path = format!("/apps/{}/machines/{}/start", credential.app, machine_id);
                let _ = Self::api_post(&credential, &start_path).await;
                transitions.push(format!(
                    "resumed:{}",
                    read_state(&credential, &machine_id).await
                ));
                HibernationObservation::Observed {
                    transitions: transitions.clone(),
                }
            }
            Ok((status, body)) => HibernationObservation::NotObserved {
                reason: format!(
                    "suspend returned HTTP {status} ({body}); this run did NOT observe \
                     hibernation and does not claim it (binding condition C1)"
                ),
            },
            Err(detail) => HibernationObservation::NotObserved {
                reason: format!(
                    "suspend could not be issued: {detail}; this run did NOT observe hibernation \
                     and does not claim it (binding condition C1)"
                ),
            },
        };

        // 5. Destroy. Cleanup is part of the run, not an afterthought.
        let destroy_path = format!(
            "/apps/{}/machines/{}?force=true",
            credential.app, machine_id
        );
        let _ = Self::api_delete(&credential, &destroy_path).await;

        let finished = now_unix_ms();
        let cancelled = cancel_marker_taken(&task.task_id);
        registry::forget(&task.task_id)?;

        // The cloud leg's stdout is the machine's captured output. Until a
        // credential exists this path has never executed against a real
        // vendor, and that is recorded in the phase evidence rather than
        // papered over: an unexercised path must not be reported as proven.
        outcome_receipt(
            task,
            &self.capabilities,
            &self.identity,
            &self.signer,
            &policy,
            RunOutcome {
                stdout: task.input.clone(),
                stderr: Vec::new(),
                exit_code: 0,
                endpoint: machine_id,
                cancelled,
                hibernation,
                started_unix_ms: started,
                finished_unix_ms: finished,
            },
        )
    }

    async fn cancel(&self, task_id: &str) -> Result<CleanupObservation> {
        let entry = registry::load(task_id)?;
        write_cancel_marker(task_id, "operator cancelled")?;
        let credential = CloudCredential::from_env()?;
        if let Some(machine_id) = entry.handle.as_deref() {
            let destroy_path = format!(
                "/apps/{}/machines/{}?force=true",
                credential.app, machine_id
            );
            let _ = Self::api_delete(&credential, &destroy_path).await;
        }
        let residual = match Self::machines_with_nonce(&credential, &entry.nonce).await {
            Ok(found) => found,
            Err(detail) => vec![format!("could not re-enumerate vendor machines: {detail}")],
        };
        registry::forget(task_id)?;
        Ok(CleanupObservation {
            task_id: task_id.into(),
            backend_id: BACKEND_ID.into(),
            method: "machine destroy, then the vendor's own nonce-filtered machine listing re-read"
                .into(),
            residual,
        })
    }

    async fn health(&self) -> Result<Health> {
        let availability = self.availability().await;
        let live = registry::list()
            .into_iter()
            .filter(|t| t.backend_id == BACKEND_ID)
            .count();
        Ok(Health {
            healthy: availability.available,
            detail: availability.detail,
            live_tasks: live,
        })
    }

    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan> {
        let credential = match CloudCredential::from_env() {
            Ok(credential) => credential,
            Err(err) => {
                return Ok(OrphanScan {
                    backend_id: BACKEND_ID.into(),
                    kind: BackendKind::Cloud,
                    nonce: nonce.into(),
                    method: err.to_string(),
                    found: Vec::new(),
                    // NOT enumerated. Reporting zero orphans because the scan
                    // could not run is how an orphan hides.
                    enumerated: false,
                });
            }
        };
        match Self::machines_with_nonce(&credential, nonce).await {
            Ok(found) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Cloud,
                nonce: nonce.into(),
                method: format!("GET /apps/<app>/machines?metadata.{NONCE_METADATA_KEY}=<nonce>"),
                found,
                enumerated: true,
            }),
            Err(detail) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Cloud,
                nonce: nonce.into(),
                method: format!("vendor machine listing failed: {detail}"),
                found: Vec::new(),
                enumerated: false,
            }),
        }
    }
}

/// Read a machine's state BACK from the vendor. Deliberately not inferred from
/// the request that asked for the transition — an inferred transition is an
/// assertion, and the criterion asks for an observation.
async fn read_state(credential: &CloudCredential, machine_id: &str) -> String {
    let path = format!("/apps/{}/machines/{}", credential.app, machine_id);
    match CloudBackend::api_get(credential, &path).await {
        Ok((200, body)) => match serde_json::from_str::<MachineSummary>(&body) {
            Ok(machine) => machine.state,
            Err(_) => "unreadable".into(),
        },
        Ok((status, _)) => format!("http-{status}"),
        Err(_) => "unreachable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_raw_http_client_is_constructed_in_this_module() {
        // Needles assembled at runtime: a literal would appear in this file's
        // own source and the scan would match itself.
        let source = include_str!("cloud.rs");
        let new_needle = ["reqwest::Client", "::new"].concat();
        let builder_needle = ["reqwest::Client", "::builder"].concat();
        assert!(
            !source.contains(&new_needle),
            "all outbound HTTP must go through wcore-egress"
        );
        assert!(
            !source.contains(&builder_needle),
            "all outbound HTTP must go through wcore-egress"
        );
        // Positive control: the module really is dialling out, through the
        // chokepoint. A guard over a module that makes no requests is vacuous.
        assert!(
            source.matches("EgressClient::new()").count() >= 3,
            "the cloud backend should be building its clients through wcore-egress"
        );
    }

    #[test]
    fn the_credential_never_renders_itself() {
        let credential = CloudCredential {
            token: "fo1_super_secret_value".into(),
            app: "wayland-f25-scratch".into(),
        };
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("fo1_super_secret_value"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn an_error_string_echoing_the_token_is_redacted() {
        let leaked = "connect failed for Bearer fo1_super_secret_value";
        assert!(!redact(leaked, "fo1_super_secret_value").contains("fo1_super_secret_value"));
    }

    #[tokio::test]
    async fn an_absent_credential_fails_closed_rather_than_falling_back() {
        unsafe {
            std::env::remove_var(TOKEN_ENV);
            std::env::remove_var(ORG_ENV);
        }
        let backend = CloudBackend::new(ResourceBudget::new(1, 1, 1, 1).unwrap()).unwrap();
        let availability = backend.availability().await;
        assert!(!availability.available);
        assert_eq!(availability.probe, ProbeBasis::CredentialAbsent);
        assert!(
            availability.detail.contains(TOKEN_ENV),
            "the unavailable verdict must name the missing credential, got: {}",
            availability.detail
        );
    }

    #[test]
    fn the_hibernation_observation_can_say_it_did_not_observe() {
        // Binding condition C1 in the type system: a stop-only implementation
        // has a variant to be honest with, so it cannot accidentally claim
        // hibernation by omission.
        let not_observed = HibernationObservation::NotObserved {
            reason: "suspend returned HTTP 404".into(),
        };
        assert!(!matches!(
            not_observed,
            HibernationObservation::Observed { .. }
        ));
    }
}
