# wcore-exec-backend::conformance_matrix::every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not (Windows only) — container backend "a successful run publishes a content-addressed artifact (Failure { code: \"exit-125\" })"

**Confidence (self-reported):** probable

## Root cause

The container backend is being exercised when it should have been reported unavailable — the harness's "or reports why it did not" mechanism is intact and correct, and the availability PROBE that feeds it under-claims what it proved. `ContainerBackend::availability()` (container.rs:114-122) answers solely from `daemon_ping()`, which runs `docker version --format {{.Server.Version}}`. That establishes "a daemon is reachable and answers", which is strictly weaker than "this backend can run its containers here". This backend is a LINUX container backend by construction, not by configuration: `execute()` bind-mounts the workspace as `<hostpath>:/task`, passes `--workdir /task`, `--network none` and a POSIX argv (`cat input.bin`) (container.rs:180-205). A dockerd in Windows-container mode — which is the default and only mode on the GitHub-hosted `windows-latest` image — answers the version ping perfectly and then refuses `docker run` at the daemon, which is exactly what exit 125 means (client/daemon refused to create or start the container; here either "image operating system 'linux' cannot be used on this platform" / "no matching manifest for windows/amd64", or "invalid volume specification" for the `/task` container path). `run_conformance` therefore skips the unavailable branch (conformance.rs:126-137), runs the happy path, gets `Ok(receipt)` with `TerminalStatus::Failure { code: "exit-125" }` from `outcome_receipt`'s non-zero-exit arm (backends/mod.rs:239-257), and the single check "a successful run publishes a content-addressed artifact" reds. So the red is an environment fact laundered through a probe that claimed more than it measured — the same class of capability lie the module doc at container.rs:3-9 already rejects for a stale socket, one level up. Fix: make the platform the daemon SERVES part of the availability answer, read from the daemon's own reply.

## Evidence

- crates/wcore-exec-backend/src/backends/container.rs:114-122 — the whole availability answer: `match daemon_ping().await { Ok(version) => Availability::up(ProbeBasis::DaemonPing, format!("container daemon answered a version ping: server {version}")), Err(detail) => Availability::down(...) }`. Reachability only; nothing about what the daemon can run.
- crates/wcore-exec-backend/src/backends/container.rs:87-106 — `daemon_ping()` runs `docker version --format {{.Server.Version}}`. On a Windows-container-mode dockerd this SUCCEEDS.
- crates/wcore-exec-backend/src/backends/container.rs:180-205 — the run path is linux-shaped regardless of `WAYLAND_EXEC_CONTAINER_IMAGE`: `let mount = format!("{}:/task", workdir.display());` … `"--workdir".into(), "/task".into()` … plus `task.argv` = `["cat", "input.bin"]` from conformance.rs:99.
- crates/wcore-exec-backend/src/conformance.rs:126-137 — the report-don't-skip mechanism the test name refers to: `let availability = backend.availability().await; if !availability.available { return ConformanceReport { exercised: false, unavailable_reason: Some(...), checks: Vec::new() } }`. It is reached only if the probe says down, so a probe that over-claims defeats it.
- crates/wcore-exec-backend/src/conformance.rs:194-198 — the one check that failed: `check("a successful run publishes a content-addressed artifact", matches!(receipt.body.terminal, TerminalStatus::Success) && artifact_ok, format!("{:?}", receipt.body.terminal))`. Its detail is exactly the reported `Failure { code: "exit-125" }`.
- crates/wcore-exec-backend/src/backends/mod.rs:239-257 — `if outcome.exit_code != 0 { let code = format!("exit-{}", outcome.exit_code); … TerminalStatus::Failure { code } }`. This is where a daemon refusal becomes a backend 'failure'.
- crates/wcore-exec-backend/tests/conformance_matrix.rs:50-54 — the only vacuity guard today is `assert!(!exercised.is_empty())`, and `local` is always exercised (conformance_matrix.rs:78-103), so that guard cannot notice the container leg going quiet. This is why the patch adds an explicit required-backends assertion rather than relying on it.
- .github/workflows/ci.yml:1119-1141 and 1197-1203 — the Linux CI leg runs the suite INSIDE `rust:1.95-slim-bookworm` (`$DOCKER_RUN_SANDBOX "$CI_IMAGE" cargo nextest run --workspace`) with no docker CLI installed and no `/var/run/docker.sock` mounted, so the container backend already reports UNEXERCISED there. windows-latest is currently the ONLY leg that exercises it — into a false red.
- .github/workflows/ci.yml:1216-1252 — the house precedent this patch's env knob copies verbatim in spirit: `WCORE_REQUIRE_DELEGATED_BACKEND=1` / `WCORE_REQUIRE_ENFORCING_SANDBOX=1`, 'a gate that cannot pass is exactly as worthless as one that cannot fail'.
- Local measurement on this Mac (docker CLI 'present', daemon down): `docker version --format '{{.Client.Os}}/{{.Client.Arch}}'` → `darwin/arm64`, i.e. the CLI's version struct really does carry an `Os` field; `docker version --format '{{.Server.Version}}'` with the daemon down exits 1 with the connect error (the existing Refused path). I could NOT observe `{{.Server.Os}}` because no daemon was reachable — that field's value on the failing runner is inferred, not observed.

## How to verify

WHAT I ACTUALLY OBSERVED (no cargo was run anywhere, per the constraint): the patch applies with `patch -p1 --dry-run` against the wt-172 tree, reproduces my intended files byte-for-byte, and both post-patch files are `rustfmt --edition 2024 --check` clean (rustfmt also parses them, so there is no syntax error; it says nothing about types). I did NOT compile, clippy, or run a single test. Everything below is what someone with a build host must run.

1. THE DECISIVE MEASUREMENT, and it costs one command. On the failing hosted windows-latest runner (or any windows-latest job), run:
     docker version --format "{{.Server.Os}}"
   Expected `windows`. That single value is what my fix keys on and the one thing I could not observe. If it prints `linux`, my root cause is WRONG and the exit-125 is something else (start there: capture the daemon's stderr, e.g. `docker run --rm -v C:\some\dir:/task -w /task --network none docker.io/library/busybox:1.36 cat input.bin; echo $LASTEXITCODE`). If it errors with a template error, fall back to `docker info --format "{{.OSType}}"` and switch PLATFORM_TEMPLATE accordingly — the patch's unknown-platform branch means an unevaluable field leaves today's behaviour (red) in place, not a false green.
2. Unit tests for the classifier, on any platform, no docker needed:
     cargo nextest run -p wcore-exec-backend backends::container::tests
   Expect 5 passed. These are the falsifiable part of the fix.
3. The matrix on a LINUX docker host (hetzner), which is the case that must keep working:
     cargo nextest run -p wcore-exec-backend --test conformance_matrix --no-capture
   Expect the printed matrix to show `backend container: PASS` with the new detail line "…server <v> serving linux containers", i.e. the fix did not make the backend unavailable where it is genuinely usable.
4. The required-backends gate on that same linux docker host:
     WAYLAND_EXEC_REQUIRE_BACKENDS=container cargo nextest run -p wcore-exec-backend --test conformance_matrix
   Expect PASS. Then stop the daemon and re-run the same command: expect FAIL naming the reason. That is the proof the new assertion is not decorative.
5. Windows leg after the fix: the matrix test passes and prints `backend container: UNEXERCISED — container daemon answered a version ping (server …) but serves windows containers; …`. If it still prints a FAIL row, step 1 was wrong.

HOW THE PATCH KEEPS THE GATE ABLE TO FAIL (the thing you asked me to state explicitly):
(a) Unavailability is derived ONLY from a value the daemon itself returned. There is no `cfg!(windows)`, no host-OS check, no test-name filter, no `#[ignore]`. A Windows host running Docker Desktop/WSL2 or Colima or a remote DOCKER_HOST in linux mode reports `linux`, is exercised, and reds on any real defect — `the_platform_check_is_on_the_daemons_answer_not_on_the_host_os` is the unit test that a future host-OS shortcut would have to delete.
(b) Unknown platform fails OPEN. If a docker-compatible client will not answer `.Server.Os`, the backend stays AVAILABLE and every conformance check still runs. A probe regression can therefore produce a false red, never a false green.
(c) Every other check in the harness is untouched. On a linux daemon all 15 assertions still apply, including the happy-path artifact check that is failing today.
(d) `WAYLAND_EXEC_REQUIRE_BACKENDS` turns "honestly unavailable" back into a hard red wherever a caller declares the backend must work, mirroring WCORE_REQUIRE_DELEGATED_BACKEND / WCORE_REQUIRE_ENFORCING_SANDBOX in ci.yml.

## Mutant

Three independent ways to prove nothing here is vacuous:
1. Classifier (no docker required). Flip `REQUIRED_DAEMON_PLATFORM` to "windows", or delete the mismatching-platform arm of `classify_availability`, and `a_daemon_serving_another_platform_is_unavailable_and_names_the_platform` + `a_daemon_serving_this_backends_platform_is_available_and_says_which` fail. Replace the daemon-answer comparison with `if cfg!(windows) { down }` and `the_platform_check_is_on_the_daemons_answer_not_on_the_host_os` fails on the Windows leg — the one leg where that shortcut would be tempting. Change the unknown branch to `Availability::down` and `an_unreported_platform_leaves_the_backend_available` fails.
2. Conformance matrix, on a linux docker host — the positive control that the container leg can still RED while available: `WAYLAND_EXEC_CONTAINER_IMAGE=docker.io/library/hello-world:latest cargo nextest run -p wcore-exec-backend --test conformance_matrix`. That image ignores the argv and exits non-zero, so the receipt terminal becomes `Failure { code: "exit-N" }`, the artifact check reds, and the test fails with exactly the shape of today's report. If it does NOT fail, the gate has become vacuous and the whole fix is wrong.
3. Required-backends assertion: on a host with the daemon stopped, `WAYLAND_EXEC_REQUIRE_BACKENDS=container cargo nextest run -p wcore-exec-backend --test conformance_matrix` must fail with "declared this host must exercise these backends, and it did not: [container: docker daemon refused…]"; `WAYLAND_EXEC_REQUIRE_BACKENDS=containr` (typo) must fail with the not-a-reference-backend message. Both failing is what proves the knob is wired to something rather than being a name that silently matches nothing.

## Unknowns

- NOT COMPILED, NOT TESTED, NOT LINTED — cargo is forbidden on this Mac and I was not cleared to use hetzner. rustfmt parsed and formatted both files (so: no syntax errors, formatting is CI-clean), but type errors and clippy findings are unverified. Treat `cargo clippy -p wcore-exec-backend --all-targets -- -D warnings` as a required step before landing.
- The value of `docker version --format '{{.Server.Os}}'` on the hosted windows-latest runner is INFERRED, not observed. actions/runner-images ships dockerd in Windows-container mode there and a linux image under it exits 125, which fits the symptom exactly, but I never saw the field. If it comes back empty or unevaluable, my patch leaves the leg red (unknown platform = available) and the correct swap is `docker info --format '{{.OSType}}'`.
- I could not recover the daemon's actual stderr for the exit-125 — the receipt content-addresses stderr (sha256 only, backends/mod.rs:173-183) and the matrix line prints only `Failure { code: "exit-125" }`. So I cannot distinguish 'image OS mismatch' from 'invalid volume specification for /task' as the specific daemon refusal. Both are the same root cause class and both are covered, but that is a real diagnosability gap in the matrix output worth its own item.
- COVERAGE REGRESSION I AM NOT CLOSING, and it should be filed: after this fix the container backend is exercised on ZERO CI legs. Linux CI runs the suite inside rust:1.95-slim-bookworm with no docker CLI and no socket mount (ci.yml:1119-1141), macOS-hosted has no daemon, windows-latest now honestly reports unavailable. `WAYLAND_EXEC_REQUIRE_BACKENDS` is therefore an opt-in knob that NO workflow sets today — by the repo's own standard (ci.yml:389-398, the dead `runner.os == 'Linux'` guard) an unwired gate is a liability. The one-time wiring is: add `docker.io-cli` to the CI image + `-v /var/run/docker.sock:/var/run/docker.sock` to DOCKER_RUN_SANDBOX, then set `WAYLAND_EXEC_REQUIRE_BACKENDS=container` on that step. I did not do it because I cannot measure docker-in-docker inside that image from here, and a wrong guess there turns the whole Linux leg red.
- Whether any consumer outside this crate depends on the container backend reporting available on a Windows-container daemon: I grepped `DaemonPing` and `"container"` across crates/ and found only contract.rs, node/capability.rs (string mapping) and test fixtures, so I believe not — but that is a grep, not a compile.

## Proposed patch (NOT APPLIED, NOT COMPILED)

```diff
--- a/crates/wcore-exec-backend/src/backends/container.rs
+++ b/crates/wcore-exec-backend/src/backends/container.rs
@@ -8,6 +8,18 @@
 //! left behind by a stopped daemon would otherwise let this backend advertise
 //! readiness it does not have, and shipping that is shipping a capability lie.
 //!
+//! A reachable daemon is NOT the same claim as a usable one, and the ping alone
+//! could not tell them apart. This backend runs LINUX containers — it
+//! bind-mounts the workspace at `/task`, passes `--workdir /task` and executes a
+//! POSIX argv — so a daemon serving a different container platform answers the
+//! version ping perfectly and then refuses every `docker run` with exit 125.
+//! That is the hosted `windows-latest` runner exactly: dockerd is up, in Windows
+//! container mode, and the conformance matrix read the resulting exit-125
+//! receipt as a backend defect. The platform the daemon SERVES is therefore part
+//! of the availability answer, and it is read from the daemon's own reply — not
+//! from the host OS, so a Windows host running a linux-mode daemon is still
+//! exercised and can still fail.
+//!
 //! Every container this backend creates carries `wayland.task.nonce` as a
 //! label, so plan 25-04's orphan scan is one `docker ps --filter label=` away
 //! from an answer instead of a guess.
@@ -34,6 +46,15 @@
 pub const NONCE_LABEL: &str = "wayland.task.nonce";
 const DEFAULT_IMAGE: &str = "docker.io/library/busybox:1.36";
 
+/// The container platform this backend requires, spelled the way `docker
+/// version` spells it.
+///
+/// Not a preference. `execute` bind-mounts the workspace at `/task`, sets
+/// `--workdir /task` and hands the daemon a POSIX argv; a daemon serving any
+/// other platform cannot run this backend's containers at all, whatever image
+/// is configured.
+const REQUIRED_DAEMON_PLATFORM: &str = "linux";
+
 pub struct ContainerBackend {
     capabilities: BackendCapabilities,
     identity: BackendIdentity,
@@ -82,29 +103,134 @@
     }
 }
 
-/// A real daemon round trip, with a bound so an unreachable daemon cannot hang
-/// `backend list`.
-async fn daemon_ping() -> std::result::Result<String, String> {
-    let mut command = wcore_config::shell::shell_command_argv(
-        "docker",
-        &["version", "--format", "{{.Server.Version}}"],
-    );
+/// What the daemon said about itself.
+#[derive(Debug, Clone, PartialEq, Eq)]
+struct DaemonAnswer {
+    version: String,
+    /// The container platform the daemon SERVES. `None` means this client would
+    /// not tell us — which is not the same as, and must never be read as, the
+    /// wrong platform.
+    platform: Option<String>,
+}
+
+/// One round trip that asks for the version AND the served platform.
+const PLATFORM_TEMPLATE: &str = "{{.Server.Version}}\t{{.Server.Os}}";
+/// The original ping, kept as the fallback for a docker-compatible client that
+/// does not expose `.Server.Os`.
+const VERSION_TEMPLATE: &str = "{{.Server.Version}}";
+
+/// Why a `docker version` call produced no answer.
+///
+/// The variants are here so the platform fallback below can retry a REFUSAL —
+/// which is what a client that cannot evaluate `.Server.Os` returns, and which
+/// costs one fast round trip — without retrying a TIMEOUT, which would double
+/// the bound this ping exists to hold.
+enum PingFailure {
+    TimedOut(String),
+    NotLaunched(String),
+    Refused(String),
+}
+
+impl PingFailure {
+    fn detail(self) -> String {
+        match self {
+            Self::TimedOut(detail) | Self::NotLaunched(detail) | Self::Refused(detail) => detail,
+        }
+    }
+}
+
+async fn docker_version(template: &str) -> std::result::Result<String, PingFailure> {
+    let mut command =
+        wcore_config::shell::shell_command_argv("docker", &["version", "--format", template]);
     command.stdout(std::process::Stdio::piped());
     command.stderr(std::process::Stdio::piped());
     let fut = command.output();
     match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
-        Err(_) => Err("docker daemon did not answer a version ping within 5s".into()),
-        Ok(Err(e)) => Err(format!("docker client could not be launched: {e}")),
+        Err(_) => Err(PingFailure::TimedOut(
+            "docker daemon did not answer a version ping within 5s".into(),
+        )),
+        Ok(Err(e)) => Err(PingFailure::NotLaunched(format!(
+            "docker client could not be launched: {e}"
+        ))),
         Ok(Ok(output)) if output.status.success() => {
             Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
         }
-        Ok(Ok(output)) => Err(format!(
+        Ok(Ok(output)) => Err(PingFailure::Refused(format!(
             "docker daemon refused the version ping: {}",
             String::from_utf8_lossy(&output.stderr).trim()
-        )),
+        ))),
     }
 }
 
+/// A real daemon round trip, with a bound so an unreachable daemon cannot hang
+/// `backend list`.
+async fn daemon_ping() -> std::result::Result<DaemonAnswer, String> {
+    match docker_version(PLATFORM_TEMPLATE).await {
+        Ok(line) => {
+            let mut fields = line.split('\t');
+            let version = fields.next().unwrap_or_default().trim().to_string();
+            let platform = fields
+                .next()
+                .map(str::trim)
+                .filter(|value| !value.is_empty() && *value != "<no value>")
+                .map(str::to_string);
+            Ok(DaemonAnswer { version, platform })
+        }
+        // A docker-compatible client that cannot evaluate `.Server.Os` fails the
+        // WHOLE template, so the plain ping is retried and the platform reported
+        // as UNKNOWN. A template gap must not be able to take this backend down:
+        // that trades a false red for a permanent green, which is the worse of
+        // the two errors and the one this crate's harness exists to prevent.
+        Err(PingFailure::Refused(_)) => match docker_version(VERSION_TEMPLATE).await {
+            Ok(version) => Ok(DaemonAnswer {
+                version,
+                platform: None,
+            }),
+            Err(plain) => Err(plain.detail()),
+        },
+        Err(other) => Err(other.detail()),
+    }
+}
+
+/// Turn what the daemon said into an availability answer.
+///
+/// Split out from [`ContainerBackend::availability`] so the decision itself is
+/// testable without a daemon: the unit tests below drive every branch, which is
+/// what stops this from silently degenerating into "unavailable on Windows".
+fn classify_availability(answer: std::result::Result<DaemonAnswer, String>) -> Availability {
+    let answer = match answer {
+        Ok(answer) => answer,
+        Err(detail) => return Availability::down(ProbeBasis::DaemonPing, detail),
+    };
+    let version = answer.version;
+    match answer.platform {
+        Some(platform) if !platform.eq_ignore_ascii_case(REQUIRED_DAEMON_PLATFORM) => {
+            Availability::down(
+                ProbeBasis::DaemonPing,
+                format!(
+                    "container daemon answered a version ping (server {version}) but serves \
+                     {platform} containers; this backend runs {REQUIRED_DAEMON_PLATFORM} \
+                     containers (workspace bind-mounted at /task, POSIX argv), which such a \
+                     daemon refuses at `docker run` with exit 125"
+                ),
+            )
+        }
+        Some(platform) => Availability::up(
+            ProbeBasis::DaemonPing,
+            format!(
+                "container daemon answered a version ping: server {version} serving {platform} containers"
+            ),
+        ),
+        None => Availability::up(
+            ProbeBasis::DaemonPing,
+            format!(
+                "container daemon answered a version ping: server {version}; this client did not \
+                 report the served platform, so the platform was NOT used to rule the backend out"
+            ),
+        ),
+    }
+}
+
 #[async_trait]
 impl ExecutionBackend for ContainerBackend {
     fn capabilities(&self) -> &BackendCapabilities {
@@ -112,13 +238,7 @@
     }
 
     async fn availability(&self) -> Availability {
-        match daemon_ping().await {
-            Ok(version) => Availability::up(
-                ProbeBasis::DaemonPing,
-                format!("container daemon answered a version ping: server {version}"),
-            ),
-            Err(detail) => Availability::down(ProbeBasis::DaemonPing, detail),
-        }
+        classify_availability(daemon_ping().await)
     }
 
     fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
@@ -334,3 +454,86 @@
         .map(str::to_string)
         .collect())
 }
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    fn answered(
+        version: &str,
+        platform: Option<&str>,
+    ) -> std::result::Result<DaemonAnswer, String> {
+        Ok(DaemonAnswer {
+            version: version.into(),
+            platform: platform.map(str::to_string),
+        })
+    }
+
+    #[test]
+    fn a_daemon_serving_this_backends_platform_is_available_and_says_which() {
+        let availability = classify_availability(answered("28.1.0", Some("linux")));
+        assert!(availability.available, "{availability:?}");
+        assert_eq!(availability.probe, ProbeBasis::DaemonPing);
+        assert!(
+            availability.detail.contains("28.1.0") && availability.detail.contains("linux"),
+            "{}",
+            availability.detail
+        );
+    }
+
+    #[test]
+    fn a_daemon_serving_another_platform_is_unavailable_and_names_the_platform() {
+        // The hosted windows-latest runner: dockerd is up and answers the ping
+        // in Windows-container mode, then refuses every linux `docker run` with
+        // exit 125. Reading that exit-125 receipt as a backend failure is what
+        // made the conformance matrix red for an environment fact.
+        let availability = classify_availability(answered("26.1.3", Some("windows")));
+        assert!(!availability.available, "{availability:?}");
+        assert_eq!(availability.probe, ProbeBasis::DaemonPing);
+        assert!(
+            availability.detail.contains("windows") && availability.detail.contains("exit 125"),
+            "the reason must name the served platform and the symptom: {}",
+            availability.detail
+        );
+    }
+
+    #[test]
+    fn the_platform_check_is_on_the_daemons_answer_not_on_the_host_os() {
+        // A Windows or macOS HOST running a linux-mode daemon (Docker Desktop,
+        // Colima, a remote DOCKER_HOST) is still exercised. This is the
+        // assertion a future `cfg!(windows)` shortcut would have to delete, and
+        // it is the reason this fix cannot become a platform skip.
+        assert!(classify_availability(answered("28.1.0", Some("Linux"))).available);
+        assert!(classify_availability(answered("28.1.0", Some("LINUX"))).available);
+    }
+
+    #[test]
+    fn an_unreported_platform_leaves_the_backend_available() {
+        // Fail-OPEN on the platform question alone. A docker-compatible client
+        // that will not answer `.Server.Os` must not be able to make this
+        // backend permanently unexercised — an untestable backend proves
+        // nothing, and a gate that cannot fail is worth nothing.
+        let availability = classify_availability(answered("5.4.0", None));
+        assert!(availability.available, "{availability:?}");
+        assert!(
+            availability
+                .detail
+                .contains("NOT used to rule the backend out"),
+            "{}",
+            availability.detail
+        );
+    }
+
+    #[test]
+    fn a_daemon_that_never_answered_is_unavailable_with_the_pings_own_reason() {
+        let availability = classify_availability(Err(
+            "docker daemon did not answer a version ping within 5s".into(),
+        ));
+        assert!(!availability.available);
+        assert_eq!(availability.probe, ProbeBasis::DaemonPing);
+        assert_eq!(
+            availability.detail,
+            "docker daemon did not answer a version ping within 5s"
+        );
+    }
+}
--- a/crates/wcore-exec-backend/tests/conformance_matrix.rs
+++ b/crates/wcore-exec-backend/tests/conformance_matrix.rs
@@ -10,6 +10,28 @@
 use wcore_exec_backend::conformance::{ConformanceReport, reference_budget, run_conformance};
 use wcore_exec_backend::reference_backends;
 
+/// Comma-separated backend ids the CALLER declares this host must be able to
+/// exercise. Unset by default.
+///
+/// An honestly reported unavailable backend is a result on a host that cannot
+/// host it, and a hole on a leg that exists to certify it. The list of "this
+/// host must be able to" therefore lives at the invocation site, exactly as
+/// `WCORE_REQUIRE_DELEGATED_BACKEND` and `WCORE_REQUIRE_ENFORCING_SANDBOX`
+/// already do in `.github/workflows/ci.yml` — a docker-capable leg sets
+/// `WAYLAND_EXEC_REQUIRE_BACKENDS=container` and an unavailable container
+/// backend reds it instead of reading as an honest skip.
+const REQUIRE_ENV: &str = "WAYLAND_EXEC_REQUIRE_BACKENDS";
+
+fn required_backends() -> Vec<String> {
+    std::env::var(REQUIRE_ENV)
+        .unwrap_or_default()
+        .split(',')
+        .map(str::trim)
+        .filter(|name| !name.is_empty())
+        .map(str::to_string)
+        .collect()
+}
+
 fn temp_state() -> tempfile::TempDir {
     let dir = tempfile::tempdir().expect("tempdir");
     unsafe { std::env::set_var("WAYLAND_EXEC_BACKEND_STATE_DIR", dir.path()) };
@@ -45,6 +67,36 @@
     }
     println!("{matrix}");
 
+    // A backend the caller DEMANDED must have been exercised. This is the
+    // counterweight to the honest skip: without it, the only thing standing
+    // between a real container-backend regression and a green matrix is
+    // whichever host happens to be running.
+    let required = required_backends();
+    for name in &required {
+        assert!(
+            reports.iter().any(|r| &r.backend_id == name),
+            "{REQUIRE_ENV} names {name:?}, which is not a reference backend — a typo here would \
+             demand nothing at all. Valid ids: {:?}",
+            reports.iter().map(|r| &r.backend_id).collect::<Vec<_>>()
+        );
+    }
+    let required_but_unexercised: Vec<String> = reports
+        .iter()
+        .filter(|r| !r.exercised && required.iter().any(|name| name == &r.backend_id))
+        .map(|r| {
+            format!(
+                "{}: {}",
+                r.backend_id,
+                r.unavailable_reason.as_deref().unwrap_or("no reason given")
+            )
+        })
+        .collect();
+    assert!(
+        required_but_unexercised.is_empty(),
+        "{REQUIRE_ENV} declared this host must exercise these backends, and it did not: \
+         {required_but_unexercised:#?}{matrix}"
+    );
+
     // At least one backend must actually have been exercised. A matrix where
     // everything is unavailable proves nothing and must not read as green.
     let exercised: Vec<&ConformanceReport> = reports.iter().filter(|r| r.exercised).collect();

```
