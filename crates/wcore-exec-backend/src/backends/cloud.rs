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
    ExecutionTask, Health, HibernationObservation, OrphanScan, OrphanSurface, OrphanSweep,
    ProbeBasis, ResourceBudget, SecretChannel,
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
/// The Fly **app** slug this backend is scoped to.
///
/// The name says ORG for historical reasons and the value is an APP. That is
/// deliberate and it is the safer of the two: an app is the narrowest surface
/// whose machine list can be asserted empty in one call, whereas an org-wide
/// assertion would go red the moment the owner created an unrelated app. The
/// credential probe that specified this variable asked for "one throwaway
/// Fly.io organization (or app)", and the app arm is the one taken.
pub const ORG_ENV: &str = "WAYLAND_F25_CLOUD_ORG";
/// Overrides the region a task machine is created in.
pub const REGION_ENV: &str = "WAYLAND_F25_CLOUD_REGION";
pub const NONCE_METADATA_KEY: &str = "wayland_task_nonce";
const API_BASE: &str = "https://api.machines.dev/v1";
const DEFAULT_REGION: &str = "iad";
/// The task machine's image. Pinned to a digest-stable tag rather than
/// `latest` so two runs a week apart are the same machine.
const MACHINE_IMAGE: &str = "alpine:3.20";
/// Where the hibernation probe plants its RAM-resident witness. `/dev/shm` is
/// a tmpfs, so its contents live in the guest's RAM and in nothing else — a
/// full machine stop loses them and a RAM-snapshot suspend keeps them. That
/// asymmetry is the whole measurement.
const RAM_WITNESS_PATH: &str = "/dev/shm/wayland-f25-hibernation.witness";

/// The remote runner, executed by `sh -c` inside the task machine.
///
/// A CONSTANT, exactly as the ssh backend's runner is. Nothing task-specific is
/// interpolated into the script text; every task-specific value arrives as a
/// positional argument that `sh` binds to `$1`, `$2`, … and never re-parses as
/// script. The task's own argv is expanded with `"$@"`, so a task argument
/// containing a shell metacharacter reaches the program as literal bytes.
const MACHINE_RUNNER: &str = r#"
set -eu
nonce="$1"; shift
b64input="$1"; shift
root="/tmp/wayland-f25-$nonce"
mkdir -p "$root"
printf '%s' "$b64input" | base64 -d > "$root/input.bin"
cd "$root"
export WAYLAND_TASK_NONCE="$nonce"
"$@"
"#;

/// Plants the RAM witness and reads back the two values that distinguish a
/// RAM-snapshot resume from a cold boot: the kernel's boot id and uptime.
const HIBERNATION_PLANT: &str = r#"
set -eu
witness_path="$1"; shift
witness="$1"; shift
printf '%s' "$witness" > "$witness_path"
printf 'WITNESS=%s\n' "$(cat "$witness_path")"
printf 'BOOT_ID=%s\n' "$(cat /proc/sys/kernel/random/boot_id)"
printf 'UPTIME=%s\n' "$(cut -d. -f1 /proc/uptime)"
"#;

/// Reads the same three values back after the resume. A missing witness file is
/// reported as the literal `MISSING` rather than allowed to fail the script,
/// because "the witness was gone" is the measurement, not an error.
const HIBERNATION_VERIFY: &str = r#"
set -eu
witness_path="$1"; shift
printf 'WITNESS=%s\n' "$(cat "$witness_path" 2>/dev/null || printf MISSING)"
printf 'BOOT_ID=%s\n' "$(cat /proc/sys/kernel/random/boot_id)"
printf 'UPTIME=%s\n' "$(cut -d. -f1 /proc/uptime)"
"#;

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
        Self::from_values(std::env::var(TOKEN_ENV).ok(), std::env::var(ORG_ENV).ok())
    }

    /// The resolution rule itself, over STATED values.
    ///
    /// Separated from [`Self::from_env`] so the absent-credential behaviour can
    /// be driven without touching the process environment. `TOKEN_ENV` and
    /// `ORG_ENV` are process-global and this crate's production code reads them
    /// on the `availability` path, so a test that removes them removes them for
    /// every sibling in the same lib binary too -- under plain `cargo test` one
    /// binary is one process (#1134).
    fn from_values(
        token: Option<String>,
        app: Option<String>,
    ) -> std::result::Result<Self, ExecError> {
        let token =
            token
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| ExecError::CredentialAbsent {
                    backend_id: BACKEND_ID.into(),
                    env: TOKEN_ENV.into(),
                })?;
        let app =
            app.filter(|v| !v.trim().is_empty())
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
    /// core#366: the nonce-scoped listing never read this, because the vendor
    /// had already filtered on it. The unscoped sweep filters here instead, so
    /// it needs the value.
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, String>,
}

/// What the vendor's `exec` endpoint returns: the task's real output, produced
/// on the machine.
#[derive(Debug, Deserialize)]
struct ExecResult {
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit_code: i32,
}

/// The vendor's answer to `start`. `previous_state` is the field that says
/// whether the machine came back from a RAM-snapshot `suspended` or from a
/// cold `stopped`, straight from the vendor rather than from our own record of
/// which call we made.
#[derive(Debug, Deserialize)]
struct StartResult {
    #[serde(default)]
    previous_state: String,
}

/// The three values the hibernation probe reads on both sides of the transition.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RamProbe {
    witness: String,
    boot_id: String,
    uptime_secs: u64,
}

impl RamProbe {
    /// Parse the runner's `KEY=value` lines. A field the probe did not emit is
    /// left empty / zero, which fails the comparison rather than passing it —
    /// an unparseable probe must not be able to certify hibernation.
    fn parse(stdout: &str) -> Self {
        let field = |key: &str| -> String {
            stdout
                .lines()
                .find_map(|line| line.trim().strip_prefix(key).map(|v| v.trim().to_string()))
                .unwrap_or_default()
        };
        Self {
            witness: field("WITNESS="),
            boot_id: field("BOOT_ID="),
            uptime_secs: field("UPTIME=").parse().unwrap_or(0),
        }
    }
}

/// Decide whether what was read back on the far side of `suspend`/`start` is a
/// RAM-snapshot resume or a cold boot wearing one's coat.
///
/// Split out as a pure function so it is unit-testable against the SHAPES both
/// transitions actually produce — measured on the vendor, recorded in this
/// phase's evidence — rather than only against the one that happened to run.
///
/// Every clause is a control that has been observed to fail. Against a real
/// `stop`/`start` on Fly, measured 2026-07-28: the witness came back `MISSING`,
/// the boot id changed, and uptime reset from 84s to 4s. All three clauses go
/// red on a stop, so a stop cannot reach [`HibernationObservation::Observed`].
fn hibernation_verdict(
    previous_state: &str,
    planted: &str,
    before: &RamProbe,
    after: &RamProbe,
    transitions: Vec<String>,
) -> HibernationObservation {
    let mut failures: Vec<String> = Vec::new();
    if previous_state != "suspended" {
        failures.push(format!(
            "the vendor reported the machine resumed from state '{previous_state}', not \
             'suspended' — a stop/start cycle is NOT hibernation (binding condition C1)"
        ));
    }
    if after.witness != planted {
        failures.push(format!(
            "the RAM witness planted in {RAM_WITNESS_PATH} did not survive the transition \
             (read back '{}'); a tmpfs file survives a RAM-snapshot suspend and is lost by a \
             machine stop, so this is a cold boot",
            after.witness
        ));
    }
    if before.boot_id.is_empty() || after.boot_id != before.boot_id {
        failures.push(format!(
            "the guest kernel boot id changed across the transition ({} -> {}), which means the \
             kernel rebooted rather than resuming from RAM",
            before.boot_id, after.boot_id
        ));
    }
    if after.uptime_secs < before.uptime_secs {
        failures.push(format!(
            "guest uptime went backwards across the transition ({}s -> {}s), which is a reboot; \
             a resumed guest continues counting",
            before.uptime_secs, after.uptime_secs
        ));
    }

    if failures.is_empty() {
        HibernationObservation::Observed { transitions }
    } else {
        HibernationObservation::NotObserved {
            reason: format!(
                "this run did NOT observe hibernation and does not claim it (binding condition \
                 C1): {}",
                failures.join("; ")
            ),
        }
    }
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
        Self::api_post_json(credential, path, &serde_json::Value::Null).await
    }

    /// POST with a JSON body.
    ///
    /// The body-less variant above was the ONLY post path this module had, and
    /// a machine cannot be created without one: the vendor requires a `config`
    /// object naming an image. That defect is why this backend had never
    /// created a machine — see the phase evidence.
    async fn api_post_json(
        credential: &CloudCredential,
        path: &str,
        body: &serde_json::Value,
    ) -> std::result::Result<(u16, String), String> {
        let client = wcore_egress::EgressClient::new();
        let url = format!("{API_BASE}{path}");
        let mut request = client.post(&url).bearer_auth(&credential.token);
        if !body.is_null() {
            request = request.json(body);
        }
        let response = request
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

    /// Run one argv on the machine and get its real stdout, stderr and exit
    /// status back.
    ///
    /// `command` is a JSON ARRAY, so every element is a separate argv entry on
    /// the far end and no shell parses any of them. This is argv mode carried
    /// across the vendor API, and it is the same rule the ssh backend follows.
    async fn machine_exec(
        credential: &CloudCredential,
        machine_id: &str,
        command: &[String],
        timeout_secs: u64,
    ) -> std::result::Result<ExecResult, String> {
        let path = format!("/apps/{}/machines/{}/exec", credential.app, machine_id);
        let body = serde_json::json!({ "command": command, "timeout": timeout_secs });
        let (status, text) = Self::api_post_json(credential, &path, &body).await?;
        if !(200..300).contains(&status) {
            return Err(format!("machine exec returned HTTP {status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("unparseable exec result: {e}"))
    }

    /// Poll the machine until it reports `want`, reading the state BACK from
    /// the vendor each time.
    ///
    /// Returns the last state seen, so a caller that timed out records what it
    /// actually saw rather than what it hoped for.
    async fn await_state(
        credential: &CloudCredential,
        machine_id: &str,
        want: &str,
        timeout_secs: u64,
    ) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let mut last;
        loop {
            last = read_state(credential, machine_id).await;
            if last == want || std::time::Instant::now() >= deadline {
                return last;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
    }

    /// Destroy a machine and report whether the vendor confirmed it.
    ///
    /// Cleanup has its own function because it must run on EVERY path out of
    /// `execute`, including the failing ones. A leaked cloud machine bills
    /// real money, and the failure paths are exactly where one leaks.
    async fn destroy_machine(credential: &CloudCredential, machine_id: &str) -> bool {
        let path = format!(
            "/apps/{}/machines/{}?force=true",
            credential.app, machine_id
        );
        matches!(
            Self::api_delete(credential, &path).await,
            Ok((status, _)) if (200..300).contains(&status)
        )
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

    /// Boot the machine, hibernate it, resume it, then run the task ON IT.
    ///
    /// The task runs AFTER the resume deliberately. Success Criterion 1 asks
    /// for the task to run "on one hibernating cloud machine", and a task that
    /// ran before the machine was ever suspended would satisfy the words while
    /// proving something weaker: this ordering makes the run itself depend on
    /// the resume having worked.
    async fn drive_machine(
        credential: &CloudCredential,
        machine_id: &str,
        task: &ExecutionTask,
    ) -> Result<(ExecResult, HibernationObservation)> {
        let mut transitions: Vec<String> = Vec::new();
        transitions.push(format!(
            "created:{}",
            read_state(credential, machine_id).await
        ));

        // 2. Wait for the machine to actually be running.
        let state = Self::await_state(credential, machine_id, "started", 90).await;
        transitions.push(format!("started:{state}"));
        if state != "started" {
            return Err(ExecError::Transport(format!(
                "machine {machine_id} never reached 'started' (last state '{state}')"
            )));
        }

        // 3. Plant a witness in the guest's RAM and read the two kernel values
        //    that a reboot cannot preserve.
        let witness = format!("f25-ram-witness-{}", task.nonce);
        let plant = Self::machine_exec(
            credential,
            machine_id,
            &[
                "/bin/sh".into(),
                "-c".into(),
                HIBERNATION_PLANT.into(),
                "f25".into(),
                RAM_WITNESS_PATH.into(),
                witness.clone(),
            ],
            30,
        )
        .await
        .map_err(ExecError::Transport)?;
        let before = RamProbe::parse(&plant.stdout);

        // 4. THE HIBERNATION TRANSITION. Condition C1: `suspend`, not `stop`.
        let suspend_path = format!("/apps/{}/machines/{}/suspend", credential.app, machine_id);
        let suspend = Self::api_post(credential, &suspend_path).await;
        let hibernation = match suspend {
            Ok((status, _)) if (200..300).contains(&status) => {
                let observed = Self::await_state(credential, machine_id, "suspended", 60).await;
                transitions.push(format!("suspended:{observed}"));

                // Resume, and take `previous_state` from the VENDOR rather
                // than inferring it from the call we chose to make.
                let start_path = format!("/apps/{}/machines/{}/start", credential.app, machine_id);
                let previous_state = match Self::api_post(credential, &start_path).await {
                    Ok((status, body)) if (200..300).contains(&status) => {
                        serde_json::from_str::<StartResult>(&body)
                            .map(|r| r.previous_state)
                            .unwrap_or_default()
                    }
                    Ok((status, _)) => format!("start-http-{status}"),
                    Err(_) => "start-unreachable".into(),
                };
                let resumed = Self::await_state(credential, machine_id, "started", 90).await;
                transitions.push(format!("resumed:{resumed} previous_state={previous_state}"));

                let verify = Self::machine_exec(
                    credential,
                    machine_id,
                    &[
                        "/bin/sh".into(),
                        "-c".into(),
                        HIBERNATION_VERIFY.into(),
                        "f25".into(),
                        RAM_WITNESS_PATH.into(),
                    ],
                    30,
                )
                .await
                .map_err(ExecError::Transport)?;
                let after = RamProbe::parse(&verify.stdout);
                transitions.push(format!(
                    "ram-witness before={} boot_id={} uptime={}s / after={} boot_id={} uptime={}s",
                    before.witness,
                    before.boot_id,
                    before.uptime_secs,
                    after.witness,
                    after.boot_id,
                    after.uptime_secs
                ));

                hibernation_verdict(&previous_state, &witness, &before, &after, transitions)
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

        // 5. Run the REAL task on the machine, in argv mode.
        use base64::Engine as _;
        let input_b64 = base64::engine::general_purpose::STANDARD.encode(&task.input);
        let mut command = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            MACHINE_RUNNER.to_string(),
            "f25".to_string(),
            task.nonce.clone(),
            input_b64,
        ];
        command.extend(task.argv.iter().cloned());
        let wall_secs = task.resources.wall_time_ms.div_ceil(1000).clamp(5, 120);
        let exec = Self::machine_exec(credential, machine_id, &command, wall_secs)
            .await
            .map_err(ExecError::Transport)?;

        Ok((exec, hibernation))
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

    /// core#366 d1. Every machine in the app that carries the nonce metadata
    /// key, whatever its value.
    async fn machines_carrying_a_nonce(
        credential: &CloudCredential,
    ) -> std::result::Result<Vec<OrphanSurface>, String> {
        let path = format!("/apps/{}/machines", credential.app);
        let (status, body) = Self::api_get(credential, &path).await?;
        if status != 200 {
            return Err(format!("machine listing returned HTTP {status}: {body}"));
        }
        let machines: Vec<MachineSummary> =
            serde_json::from_str(&body).map_err(|e| format!("unparseable machine listing: {e}"))?;
        Ok(machines
            .into_iter()
            .filter_map(|m| {
                let nonce = m.metadata.get(NONCE_METADATA_KEY)?;
                Some(OrphanSurface {
                    id: format!("machine {} ({}) state={}", m.id, m.name, m.state),
                    nonce: nonce.clone(),
                })
            })
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

impl CloudBackend {
    /// [`ExecutionBackend::availability`] over an ALREADY-RESOLVED
    /// credential.
    ///
    /// The verdict logic lives here so a test can exercise it -- including
    /// the fail-closed arm -- by STATING the credential instead of removing
    /// `TOKEN_ENV`/`ORG_ENV` from the process the whole lib binary shares.
    async fn availability_of(
        &self,
        credential: std::result::Result<CloudCredential, ExecError>,
    ) -> Availability {
        let credential = match credential {
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
}

#[async_trait]
impl ExecutionBackend for CloudBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        self.availability_of(CloudCredential::from_env()).await
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

        // 1. Create the machine, TAGGED WITH THE TASK NONCE.
        //
        // The tag is not decoration. `scan_orphans` and `cancel` both find a
        // machine by filtering on this exact metadata key, so a machine created
        // without it is invisible to both — an orphan that no scan can ever
        // return, and a clean scan that means nothing. The create call
        // previously sent no body at all, which set no metadata and in fact
        // created no machine.
        let create_path = format!("/apps/{}/machines", credential.app);
        let create_body = machine_create_body(task);
        let (status, body) =
            match Self::api_post_json(&credential, &create_path, &create_body).await {
                Ok(response) => response,
                Err(detail) => {
                    registry::forget(&task.task_id)?;
                    return Err(ExecError::Transport(detail));
                }
            };
        if !(200..300).contains(&status) {
            registry::forget(&task.task_id)?;
            return Err(ExecError::Transport(format!(
                "machine create returned HTTP {status}: {body}"
            )));
        }
        let created: MachineSummary = match serde_json::from_str(&body) {
            Ok(created) => created,
            Err(e) => {
                registry::forget(&task.task_id)?;
                return Err(ExecError::Transport(format!(
                    "unparseable machine create: {e}"
                )));
            }
        };
        let machine_id = created.id.clone();

        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Cloud,
            pid: None,
            handle: Some(machine_id.clone()),
            started_unix_ms: started,
        })?;

        // 2-4 run against a live machine and can fail at any point. Whatever
        // they return, the machine is destroyed below BEFORE the result is
        // examined — an early `?` here would leak a billable machine, and the
        // failure paths are precisely where a leak happens.
        let run = Self::drive_machine(&credential, &machine_id, task).await;

        let destroyed = Self::destroy_machine(&credential, &machine_id).await;

        let finished = now_unix_ms();
        let cancelled = cancel_marker_taken(&task.task_id);
        registry::forget(&task.task_id)?;

        // A CANCELLED cloud run reaches this point as a FAILED vendor call, and
        // that is not a transport fault to report as one.
        //
        // `cancel` destroys the machine out from under the in-flight exec, so
        // the vendor answers the exec with an error — measured live on
        // 2026-07-29 as `HTTP 412 failed_precondition: exec request failed:
        // EOF`, and as `HTTP 408 deadline_exceeded` when the timing differs.
        // Without this arm the error propagated, `execute` returned `Err`, and
        // the cloud surface wrote NO RECEIPT AT ALL for a cancellation — while
        // local, container and ssh all write one carrying
        // `Cancelled { reason: "operator cancelled" }`. Criterion 1 asks for
        // equivalent receipts AND cancellation across the four backends, so a
        // cancellation the fourth backend cannot attest is a real gap, and it
        // was invisible until the leg was actually driven.
        //
        // The cancel marker is the discriminator and it is authoritative: only
        // `cancel` writes it, and `cancel_marker_taken` above has already
        // consumed it. An error with no marker is still a genuine failure and
        // still propagates.
        let (exec, hibernation) = match run {
            Ok(pair) => pair,
            Err(err) => {
                let Some(reason) = cancelled else {
                    return Err(err);
                };
                return outcome_receipt(
                    task,
                    &self.capabilities,
                    &self.identity,
                    &self.signer,
                    &policy,
                    RunOutcome {
                        // Nothing of the vendor's error text reaches the
                        // receipt: it is control-plane output, and the receipt
                        // is for the TASK.
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        exit_code: -1,
                        endpoint: machine_id,
                        cancelled: Some(reason),
                        // The hibernation observation died with the failed
                        // drive, and a cancelled run must not claim one it
                        // cannot show (binding condition C1).
                        hibernation: HibernationObservation::NotObserved {
                            reason: "the run was cancelled before it could report a hibernation \
                                     observation; this receipt does not claim one"
                                .into(),
                        },
                        started_unix_ms: started,
                        finished_unix_ms: finished,
                    },
                );
            }
        };
        if !destroyed {
            return Err(ExecError::Transport(format!(
                "the task machine {machine_id} could not be destroyed; it may still be running \
                 and billing. Enumerate with `wayland-core backend orphans --nonce {}`",
                task.nonce
            )));
        }

        // The cloud leg's stdout is the machine's OWN captured output, returned
        // by the vendor's exec endpoint. It is not the submitted input echoed
        // back: an echo would produce a receipt digest identical to the other
        // three backends' while nothing whatsoever ran in the cloud, which is
        // the precise shape of a false equivalence.
        outcome_receipt(
            task,
            &self.capabilities,
            &self.identity,
            &self.signer,
            &policy,
            RunOutcome {
                stdout: exec.stdout.into_bytes(),
                stderr: exec.stderr.into_bytes(),
                exit_code: exec.exit_code,
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

    /// core#366: the whole app listing, filtered on the metadata KEY rather
    /// than on its value.
    ///
    /// The vendor filter is `metadata.<key>=<value>` and takes no key-presence
    /// form, so the key-presence match happens here, in Rust, over the full
    /// listing. That is the same reason `orphan::enumerate_process_table`
    /// filters in Rust: a filter expressed in someone else's query language can
    /// silently drop rows, and here a dropped row is a leaked machine that
    /// still bills.
    async fn sweep_orphans(&self) -> Result<OrphanSweep> {
        let credential = match CloudCredential::from_env() {
            Ok(credential) => credential,
            Err(err) => {
                return Ok(OrphanSweep {
                    backend_id: BACKEND_ID.into(),
                    kind: BackendKind::Cloud,
                    method: err.to_string(),
                    found: Vec::new(),
                    enumerated: false,
                });
            }
        };
        match Self::machines_carrying_a_nonce(&credential).await {
            Ok(found) => Ok(OrphanSweep {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Cloud,
                method: format!(
                    "GET /apps/<app>/machines, filtered on metadata.{NONCE_METADATA_KEY} \
                     being PRESENT"
                ),
                found,
                enumerated: true,
            }),
            Err(detail) => Ok(OrphanSweep {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Cloud,
                method: format!("vendor machine listing failed: {detail}"),
                found: Vec::new(),
                enumerated: false,
            }),
        }
    }
}

/// The guest size for a task, in the units the vendor accepts.
///
/// Rounded UP to the vendor's 256 MB granularity so a task is never given less
/// memory than it asked for, and clamped to a ceiling because this backend
/// creates machines on a real account and an unbounded value here is an
/// unbounded bill.
fn guest_memory_mb(memory_bytes: u64) -> u64 {
    let requested_mb = memory_bytes.div_ceil(1024 * 1024);
    requested_mb
        .div_ceil(256)
        .saturating_mul(256)
        .clamp(256, 2048)
}

/// The machine-create body.
///
/// `metadata` carries the task nonce under [`NONCE_METADATA_KEY`] — the same
/// key `scan_orphans` and `cancel` filter on. `init.exec` holds the machine
/// open with a sleeper rather than letting the image's own entrypoint exit,
/// because a machine that has already stopped cannot be suspended and cannot
/// run the task. `restart.policy: no` stops the vendor resurrecting a machine
/// this backend believes it destroyed.
fn machine_create_body(task: &ExecutionTask) -> serde_json::Value {
    let region = std::env::var(REGION_ENV)
        .ok()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    serde_json::json!({
        "region": region,
        "config": {
            "image": MACHINE_IMAGE,
            "guest": {
                "cpu_kind": "shared",
                "cpus": 1,
                "memory_mb": guest_memory_mb(task.resources.memory_bytes),
            },
            "init": { "exec": ["/bin/sleep", "inf"] },
            "auto_destroy": false,
            "restart": { "policy": "no" },
            "metadata": { (NONCE_METADATA_KEY): task.nonce },
        }
    })
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

    /// The fail-closed rule, over stated values, in both directions.
    ///
    /// `availability` reports `CredentialAbsent` on the strength of this
    /// resolver, so every way of being absent must reach it -- including the
    /// two that are NOT `None`: an empty value and a whitespace-only one, which
    /// is what an unset variable in a CI file (`WAYLAND_F25_CLOUD_TOKEN=`)
    /// actually produces. Neither had coverage. The positive direction is
    /// asserted too, so a resolver that refused everything could not pass.
    #[test]
    fn the_cloud_credential_resolver_fails_closed_on_every_absent_shape() {
        let present = |c: std::result::Result<CloudCredential, ExecError>| {
            c.expect("a complete credential must resolve")
        };
        let absent_var = |c: std::result::Result<CloudCredential, ExecError>| match c {
            Err(ExecError::CredentialAbsent { env, .. }) => env,
            Err(other) => panic!("expected CredentialAbsent, got {other:?}"),
            Ok(_) => panic!("an incomplete credential must not resolve"),
        };

        // Positive control: both present resolves, and carries the values.
        let ok = present(CloudCredential::from_values(
            Some("fo1-fixture-token".into()),
            Some("fixture-app".into()),
        ));
        assert_eq!(ok.app, "fixture-app");

        // Every absent shape of the TOKEN, each naming the token variable.
        for token in [None, Some(String::new()), Some("   ".into())] {
            assert_eq!(
                absent_var(CloudCredential::from_values(
                    token.clone(),
                    Some("fixture-app".into())
                )),
                TOKEN_ENV,
                "an absent token ({token:?}) must fail closed naming {TOKEN_ENV}"
            );
        }

        // Every absent shape of the ORG, each naming the org variable. The
        // token is present here, so a resolver that always blamed the token
        // would fail this half.
        for app in [None, Some(String::new()), Some("   ".into())] {
            assert_eq!(
                absent_var(CloudCredential::from_values(
                    Some("fo1-fixture-token".into()),
                    app.clone()
                )),
                ORG_ENV,
                "an absent org ({app:?}) must fail closed naming {ORG_ENV}"
            );
        }
    }

    /// The credential is STATED absent rather than removed from the process.
    ///
    /// This test used to `remove_var(TOKEN_ENV)` and `remove_var(ORG_ENV)` and
    /// never put them back, from a plain `#[tokio::test]` with no
    /// serialization. Under `cargo nextest` that is invisible -- every test
    /// gets its own process -- but under plain `cargo test` this lib binary is
    /// ONE process, production `CloudCredential::from_env` reads both vars on
    /// the `availability` path, and the removal outlived this test for every
    /// sibling that ran after it (#1134).
    #[tokio::test]
    async fn an_absent_credential_fails_closed_rather_than_falling_back() {
        let backend = CloudBackend::new(ResourceBudget::new(1, 1, 1, 1).unwrap()).unwrap();
        let availability = backend
            .availability_of(CloudCredential::from_values(None, None))
            .await;
        assert!(!availability.available);
        assert_eq!(availability.probe, ProbeBasis::CredentialAbsent);
        assert!(
            availability.detail.contains(TOKEN_ENV),
            "the unavailable verdict must name the missing credential, got: {}",
            availability.detail
        );
    }

    /// A cancelled cloud run must produce a CANCELLED RECEIPT, not a bare
    /// transport error.
    ///
    /// Driven live on 2026-07-29 and this is the defect that drive found:
    /// `cancel` destroys the machine out from under the in-flight exec, the
    /// vendor answers `HTTP 412 failed_precondition`, and before the arm in
    /// `execute` existed that error propagated and the cloud surface wrote
    /// **no receipt at all** — while local, container and ssh each wrote one
    /// carrying `Cancelled { reason: "operator cancelled" }`. See
    /// `evidence/25-c1-cleanup/cloud-cancel-BEFORE.txt` (ABSENT) against
    /// `cloud-cancel-AFTER.txt` (WRITTEN).
    ///
    /// This pins the receipt SHAPE that arm builds. The live re-drive is what
    /// proves the arm is reached; a unit test cannot destroy a real machine.
    #[test]
    fn a_cancelled_cloud_run_yields_a_cancelled_receipt_and_claims_no_hibernation() {
        use crate::receipt::TerminalStatus;

        let backend = CloudBackend::new(crate::conformance::reference_budget()).unwrap();
        let task = crate::conformance::reference_task(
            "f25c1cancel",
            "f25c1cancel",
            crate::conformance::reference_budget(),
        );
        let policy = backend.effective_policy(&task).unwrap();

        // Exactly the RunOutcome the cancelled arm constructs.
        let cancelled = |reason: Option<String>| {
            outcome_receipt(
                &task,
                &backend.capabilities,
                &backend.identity,
                &backend.signer,
                &policy,
                RunOutcome {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: -1,
                    endpoint: "8d967d9fe40308".into(),
                    cancelled: reason,
                    hibernation: HibernationObservation::NotObserved {
                        reason: "the run was cancelled before it could report a hibernation \
                                 observation; this receipt does not claim one"
                            .into(),
                    },
                    started_unix_ms: 1,
                    finished_unix_ms: 2,
                },
            )
            .unwrap()
        };

        let receipt = cancelled(Some("operator cancelled".into()));
        assert!(
            matches!(receipt.body.terminal, TerminalStatus::Cancelled { .. }),
            "a cancelled run must terminate as Cancelled, got {:?}",
            receipt.body.terminal
        );
        // A cancelled run must not carry a hibernation claim it cannot show.
        assert!(matches!(
            receipt.body.hibernation,
            HibernationObservation::NotObserved { .. }
        ));
        // And the receipt must still be a receipt: integrity holds.
        receipt.verify_integrity_only().unwrap();

        // NEGATIVE CONTROL. Without the cancel marker the identical outcome
        // must NOT read as a cancellation — otherwise the arm would relabel
        // every genuine cloud failure as an operator cancellation, which is a
        // worse defect than the one it fixes.
        let uncancelled = cancelled(None);
        assert!(
            !matches!(uncancelled.body.terminal, TerminalStatus::Cancelled { .. }),
            "an uncancelled failure was labelled Cancelled: {:?}",
            uncancelled.body.terminal
        );
    }

    // ---- the hibernation discriminator ----------------------------------
    //
    // These are pinned against the two shapes MEASURED on the live vendor from
    // hetzner-dsm on 2026-07-28, recorded in
    // `evidence/25-cloud-suspend-vs-stop-control.txt`. Both transitions were
    // driven on the same machine minutes apart, so the difference between them
    // is the transition and nothing else.

    /// The suspend shape, verbatim from the live run: witness survived, boot id
    /// unchanged, uptime continued.
    fn measured_suspend() -> (RamProbe, RamProbe) {
        (
            RamProbe {
                witness: "f25-ram-witness-n1".into(),
                boot_id: "c5a791f8-71cf-4f0a-92ae-b29315f08002".into(),
                uptime_secs: 55,
            },
            RamProbe {
                witness: "f25-ram-witness-n1".into(),
                boot_id: "c5a791f8-71cf-4f0a-92ae-b29315f08002".into(),
                uptime_secs: 63,
            },
        )
    }

    /// The stop shape, verbatim from the live control: witness MISSING, boot id
    /// changed, uptime reset from 84s to 4s.
    fn measured_stop() -> (RamProbe, RamProbe) {
        (
            RamProbe {
                witness: "f25-ram-witness-n1".into(),
                boot_id: "c5a791f8-71cf-4f0a-92ae-b29315f08002".into(),
                uptime_secs: 84,
            },
            RamProbe {
                witness: "MISSING".into(),
                boot_id: "8e4a0900-5666-4878-8fa2-4e03bf48f1e1".into(),
                uptime_secs: 4,
            },
        )
    }

    #[test]
    fn a_real_suspend_resume_is_the_only_shape_that_reaches_observed() {
        let (before, after) = measured_suspend();
        let verdict = hibernation_verdict(
            "suspended",
            "f25-ram-witness-n1",
            &before,
            &after,
            vec!["suspended:suspended".into()],
        );
        assert!(
            matches!(verdict, HibernationObservation::Observed { .. }),
            "the measured suspend/resume shape must be accepted: {verdict:?}"
        );
    }

    /// The control. A stop/start cycle reported as hibernation would be a false
    /// green on the ONE property distinguishing this backend from the other
    /// three, so it must be rejected — and rejected by every clause
    /// independently, not by one that a vendor change could quietly remove.
    #[test]
    fn a_stop_start_cycle_is_refused_by_every_clause_independently() {
        let (before, after) = measured_stop();
        let verdict = hibernation_verdict(
            "stopped",
            "f25-ram-witness-n1",
            &before,
            &after,
            vec!["stopped:stopped".into()],
        );
        let reason = match &verdict {
            HibernationObservation::NotObserved { reason } => reason.clone(),
            other => panic!("a stop/start cycle was reported as hibernation: {other:?}"),
        };
        assert!(reason.contains("not 'suspended'"), "{reason}");
        assert!(reason.contains("RAM witness"), "{reason}");
        assert!(reason.contains("boot id changed"), "{reason}");
        assert!(reason.contains("uptime went backwards"), "{reason}");
    }

    /// Each clause must be able to redden ON ITS OWN. A discriminator that only
    /// fails when all four signals fail together would pass a vendor that
    /// preserved tmpfs across a reboot, or a `previous_state` string that drifted.
    #[test]
    fn each_clause_reddens_alone() {
        let (before, after) = measured_suspend();
        let witness = "f25-ram-witness-n1";

        // Only previous_state wrong.
        assert!(matches!(
            hibernation_verdict("stopped", witness, &before, &after, vec![]),
            HibernationObservation::NotObserved { .. }
        ));
        // Only the witness lost.
        let lost = RamProbe {
            witness: "MISSING".into(),
            ..after.clone()
        };
        assert!(matches!(
            hibernation_verdict("suspended", witness, &before, &lost, vec![]),
            HibernationObservation::NotObserved { .. }
        ));
        // Only the boot id changed.
        let rebooted = RamProbe {
            boot_id: "8e4a0900-5666-4878-8fa2-4e03bf48f1e1".into(),
            ..after.clone()
        };
        assert!(matches!(
            hibernation_verdict("suspended", witness, &before, &rebooted, vec![]),
            HibernationObservation::NotObserved { .. }
        ));
        // Only uptime went backwards.
        let reset = RamProbe {
            uptime_secs: 4,
            ..after.clone()
        };
        assert!(matches!(
            hibernation_verdict("suspended", witness, &before, &reset, vec![]),
            HibernationObservation::NotObserved { .. }
        ));
    }

    /// A probe whose output could not be parsed must not certify hibernation.
    /// Empty fields compare equal to each other, so a naive implementation
    /// would report Observed for a machine it never actually reached.
    #[test]
    fn an_unparseable_probe_cannot_certify_hibernation() {
        let empty = RamProbe::parse("");
        assert_eq!(empty.witness, "");
        assert_eq!(empty.boot_id, "");
        assert!(matches!(
            hibernation_verdict("suspended", "w", &empty, &empty, vec![]),
            HibernationObservation::NotObserved { .. }
        ));
    }

    #[test]
    fn the_probe_parses_the_runner_output_shape() {
        let probe = RamProbe::parse(
            "WITNESS=f25-ram-witness-n1\nBOOT_ID=c5a791f8-71cf-4f0a-92ae-b29315f08002\nUPTIME=63\n",
        );
        assert_eq!(probe.witness, "f25-ram-witness-n1");
        assert_eq!(probe.boot_id, "c5a791f8-71cf-4f0a-92ae-b29315f08002");
        assert_eq!(probe.uptime_secs, 63);
    }

    // ---- the create body -------------------------------------------------

    fn probe_task(nonce: &str) -> ExecutionTask {
        crate::conformance::reference_task("t-1", nonce, crate::conformance::reference_budget())
    }

    /// The nonce tag is what makes an orphan findable. Without it
    /// `scan_orphans` filters on a key no machine carries, so it returns an
    /// empty list unconditionally — a scan that cannot see anything, reported
    /// as zero orphans.
    #[test]
    fn a_created_machine_is_tagged_with_the_task_nonce() {
        let body = machine_create_body(&probe_task("f25-nonce-abc"));
        assert_eq!(
            body["config"]["metadata"][NONCE_METADATA_KEY]
                .as_str()
                .unwrap(),
            "f25-nonce-abc",
            "an untagged machine is invisible to the orphan scan that is supposed to find it"
        );
    }

    /// The machine must stay up long enough to be suspended and to run the
    /// task. An image whose entrypoint exits leaves a stopped machine, and a
    /// stopped machine cannot be suspended.
    #[test]
    fn the_machine_is_held_open_and_not_auto_restarted() {
        let body = machine_create_body(&probe_task("f25-nonce-abc"));
        assert_eq!(
            body["config"]["init"]["exec"][0].as_str().unwrap(),
            "/bin/sleep"
        );
        assert_eq!(body["config"]["restart"]["policy"].as_str().unwrap(), "no");
        assert!(!body["config"]["auto_destroy"].as_bool().unwrap());
        assert!(
            body["config"]["image"]
                .as_str()
                .unwrap()
                .starts_with("alpine:")
        );
    }

    #[test]
    fn guest_memory_rounds_up_and_is_bounded() {
        // The reference budget's 256 MB maps to exactly one unit.
        assert_eq!(guest_memory_mb(256 * 1024 * 1024), 256);
        // Anything smaller still gets the vendor's minimum.
        assert_eq!(guest_memory_mb(1), 256);
        // A request between granularities rounds UP, never down.
        assert_eq!(guest_memory_mb(300 * 1024 * 1024), 512);
        // And an absurd request is clamped rather than billed.
        assert_eq!(guest_memory_mb(u64::MAX), 2048);
    }

    /// The task's argv crosses as separate argv entries and is never spliced
    /// into the runner's script text — the same rule the ssh backend follows.
    #[test]
    fn the_runner_binds_task_values_positionally_rather_than_interpolating_them() {
        assert!(MACHINE_RUNNER.contains(r#"nonce="$1""#));
        assert!(MACHINE_RUNNER.contains(r#"b64input="$1""#));
        assert!(MACHINE_RUNNER.contains("base64 -d"));
        assert!(
            MACHINE_RUNNER.contains(r#""$@""#),
            "the task argv must be expanded as argv, not re-parsed as script"
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
