//! FerroxLabs/wayland#1137 + core#340 — one test PER RUNNER FORM.
//!
//! The gate's coverage was asserted form-by-form rather than reasoned about:
//! `pipx run <pkg>` and `pipx install <pkg>` were REPORTED as querying OSV for
//! the literal subcommand token. This file grades every form the gate claims,
//! so the doc comment can name exactly what is and is not covered.
//!
//! Every case asserts the PACKAGE THAT WOULD BE FETCHED is the package that
//! gets queried. A form that queries the wrong name is worse than one that
//! queries nothing: it reports clean on a name nobody installed.

use std::sync::Arc;

use wcore_tools::osv_check::{
    CapturedOsvCall, CapturingOsvBackend, Ecosystem, MalwareCheckOutcome, OsvAdvisory, OsvBackend,
    OsvBackendError, check_package_for_malware,
};

/// A literal public IP, never a hostname: `check_package_for_malware` gates
/// the endpoint through `url_safety::is_safe_url`, which does a REAL DNS
/// lookup and fails closed on an empty answer. A hostname here would make
/// every case in this file depend on the runner's resolver, and the
/// fail-closed direction is a VACUOUS pass (the backend is never called).
const ENDPOINT: &str = "https://93.184.216.34/v1/query";

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A backend that reports malware for exactly ONE package name and clean for
/// every other. Lets a test distinguish "the gate checked this package" from
/// "the gate checked something and the canned answer happened to be a hit".
struct OnlyPackageIsMalware {
    malicious: &'static str,
}

impl OnlyPackageIsMalware {
    fn new(malicious: &'static str) -> Self {
        Self { malicious }
    }
}

#[async_trait::async_trait]
impl OsvBackend for OnlyPackageIsMalware {
    async fn query(
        &self,
        _endpoint: &str,
        _ecosystem: Ecosystem,
        package: &str,
        _version: Option<&str>,
    ) -> Result<Vec<OsvAdvisory>, OsvBackendError> {
        if package == self.malicious {
            Ok(vec![OsvAdvisory {
                id: "MAL-2024-0002".into(),
                summary: format!("postinstall exfiltration in {package}"),
            }])
        } else {
            Ok(Vec::new())
        }
    }
}

/// Drive the real entry point and return every OSV query it made.
async fn queries(command: &str, args: &[&str]) -> (MalwareCheckOutcome, Vec<CapturedOsvCall>) {
    let backend = Arc::new(CapturingOsvBackend::with_response(vec![]));
    let outcome = check_package_for_malware(command, &argv(args), ENDPOINT, backend.as_ref()).await;
    let calls = backend.calls.lock().clone();
    (outcome, calls)
}

/// Assert the set of package names queried, order-insensitive.
fn assert_queried(calls: &[CapturedOsvCall], expected: &[(&str, Ecosystem)], form: &str) {
    let mut got: Vec<(String, Ecosystem)> = calls
        .iter()
        .map(|c| (c.package.clone(), c.ecosystem))
        .collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want: Vec<(String, Ecosystem)> = expected
        .iter()
        .map(|(p, e)| ((*p).to_string(), *e))
        .collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got, want, "form `{form}` queried the wrong package set");
}

// ── npx ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn form_npx_positional() {
    let (outcome, calls) = queries("npx", &["-y", "evil-pkg@1.0.0"]).await;
    assert_eq!(outcome, MalwareCheckOutcome::Allowed);
    assert_queried(&calls, &[("evil-pkg", Ecosystem::Npm)], "npx -y <pkg>");
    assert_eq!(calls[0].version.as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn form_npx_package_flag_names_the_package_not_the_entry_point() {
    // `npx --package X -c "cmd"` installs X and runs `cmd` out of it.
    let (_, calls) = queries("npx", &["--package", "evil-pkg", "-c", "server"]).await;
    assert_queried(
        &calls,
        &[("evil-pkg", Ecosystem::Npm)],
        "npx --package <pkg> -c <cmd>",
    );
}

// ── uvx ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn form_uvx_positional() {
    let (outcome, calls) = queries("uvx", &["evil-pkg==1.0"]).await;
    assert_eq!(outcome, MalwareCheckOutcome::Allowed);
    assert_queried(&calls, &[("evil-pkg", Ecosystem::PyPI)], "uvx <pkg>");
    assert_eq!(calls[0].version.as_deref(), Some("1.0"));
}

#[tokio::test]
async fn form_uvx_from_names_the_package_not_the_entry_point() {
    let (_, calls) = queries("uvx", &["--from", "evil-pkg==1.0", "server"]).await;
    assert_queried(
        &calls,
        &[("evil-pkg", Ecosystem::PyPI)],
        "uvx --from <pkg> <entry>",
    );
}

/// `--with` packages are INSTALLED AND IMPORTED by the run. A gate that
/// checks only the primary package clears an argv whose second package is
/// the malicious one — and `--with` is the form where that hides best,
/// because the primary is a famous name.
#[tokio::test]
async fn form_uvx_with_extras_are_also_fetched_and_must_be_checked() {
    let (_, calls) = queries("uvx", &["--with", "evil-helper==2.0", "mcp-server"]).await;
    assert_queried(
        &calls,
        &[
            ("mcp-server", Ecosystem::PyPI),
            ("evil-helper", Ecosystem::PyPI),
        ],
        "uvx --with <extra> <pkg>",
    );
}

/// A malware hit on a `--with` extra must block, not just one on the primary.
///
/// The backend answers for ONE package name only. A `CapturingOsvBackend` with
/// a canned advisory list would pass this vacuously — it returns the same hit
/// for the primary, so the test could not tell a gate that checks the extra
/// from one that does not.
#[tokio::test]
async fn form_uvx_with_extra_malware_blocks() {
    let backend = OnlyPackageIsMalware::new("evil-helper");
    let outcome = check_package_for_malware(
        "uvx",
        &argv(&["--with", "evil-helper", "mcp-server"]),
        ENDPOINT,
        &backend,
    )
    .await;
    let MalwareCheckOutcome::Blocked(message) = outcome else {
        panic!("a malware hit on a --with extra must block, got {outcome:?}");
    };
    assert!(
        message.contains("evil-helper"),
        "the refusal must name the package that is malicious: {message}"
    );
}

/// NEGATIVE CONTROL for the arm above: with the SAME backend, an argv whose
/// only package is the clean primary must be allowed. Without this, "blocks"
/// could be satisfied by a gate that blocks every `--with` argv.
#[tokio::test]
async fn form_uvx_with_clean_extra_is_allowed() {
    let backend = OnlyPackageIsMalware::new("evil-helper");
    let outcome = check_package_for_malware(
        "uvx",
        &argv(&["--with", "pydantic", "mcp-server"]),
        ENDPOINT,
        &backend,
    )
    .await;
    assert_eq!(outcome, MalwareCheckOutcome::Allowed);
}

// ── pipx ────────────────────────────────────────────────────────────────────

/// The reported defect. `pipx run evil-pkg` fetches `evil-pkg`; a
/// subcommand-unaware scan queries the literal token `run`, which is a real
/// PyPI project name and comes back clean.
#[tokio::test]
async fn form_pipx_run_queries_the_package_not_the_subcommand() {
    let (outcome, calls) = queries("pipx", &["run", "evil-pkg"]).await;
    assert_eq!(outcome, MalwareCheckOutcome::Allowed);
    assert_queried(&calls, &[("evil-pkg", Ecosystem::PyPI)], "pipx run <pkg>");
}

#[tokio::test]
async fn form_pipx_install_queries_the_package_not_the_subcommand() {
    let (_, calls) = queries("pipx", &["install", "evil-pkg==3.1"]).await;
    assert_queried(
        &calls,
        &[("evil-pkg", Ecosystem::PyPI)],
        "pipx install <pkg>",
    );
    assert_eq!(calls[0].version.as_deref(), Some("3.1"));
}

#[tokio::test]
async fn form_pipx_run_spec_names_the_package_not_the_entry_point() {
    let (_, calls) = queries("pipx", &["run", "--spec", "evil-pkg", "server"]).await;
    assert_queried(
        &calls,
        &[("evil-pkg", Ecosystem::PyPI)],
        "pipx run --spec <pkg> <entry>",
    );
}

/// KNOWN-POSITIVE CONTROL for the subcommand handling: a pipx invocation that
/// fetches nothing must query nothing. Without this, "queries the package"
/// could be satisfied by a gate that queries every token.
#[tokio::test]
async fn form_pipx_non_fetching_subcommand_is_not_applicable() {
    let (outcome, calls) = queries("pipx", &["list"]).await;
    assert_eq!(
        outcome,
        MalwareCheckOutcome::NotApplicable,
        "`pipx list` fetches nothing from a registry"
    );
    assert!(
        calls.is_empty(),
        "`pipx list` must not be queried at all, got {calls:?}"
    );
}

/// core#340 — a NOT-covered form, pinned so it cannot be re-described as
/// coverage.
///
/// `pipx inject <venv> <pkg>` fetches `<pkg>` from PyPI and runs its install
/// hooks, and the gate does not check it. The gap is deliberate: `inject`'s
/// first positional is a venv NAME, so the parser that reads `run`/`install`
/// would query the venv and report a confident CLEAN on a package nobody
/// looked at — a wrong answer, which this module holds to be worse than no
/// answer.
///
/// The assertion that matters is the SECOND one: nothing was queried. A test
/// that only checked the outcome would still pass if some future edit started
/// querying the venv name.
#[tokio::test]
async fn form_pipx_inject_is_a_documented_gap() {
    let (outcome, calls) = queries("pipx", &["inject", "my-venv", "evil-pkg"]).await;
    assert_eq!(
        outcome,
        MalwareCheckOutcome::NotApplicable,
        "`pipx inject` is a documented coverage GAP, not a check"
    );
    assert!(
        calls.is_empty(),
        "nothing may be queried for an argv whose package operand this parser \
         cannot place — querying the venv name would report CLEAN on a package \
         nobody looked at, got {calls:?}"
    );
}

/// KNOWN-POSITIVE CONTROL for the gap above: the SAME package name, in the
/// SAME `pipx` command, under a subcommand the parser CAN place, IS queried.
/// Without this, the arm above passes just as well on a `pipx` arm that was
/// broken outright.
#[tokio::test]
async fn form_pipx_install_control_queries_the_same_package() {
    let (_, calls) = queries("pipx", &["install", "evil-pkg"]).await;
    assert_queried(
        &calls,
        &[("evil-pkg", Ecosystem::PyPI)],
        "pipx install evil-pkg",
    );
}

/// `pipx run` with no package is a runner whose argv names nothing readable.
#[tokio::test]
async fn form_pipx_run_with_no_package_is_unidentified() {
    let (outcome, calls) = queries("pipx", &["run"]).await;
    assert_eq!(outcome, MalwareCheckOutcome::Unidentified);
    assert!(calls.is_empty());
}

// ── the boundary the doc comment must state ─────────────────────────────────

/// NOT covered, and the doc comment says so. These are real package runners
/// that fetch from a public registry; the gate does not recognise them, so it
/// returns NotApplicable and the launch proceeds unchecked.
///
/// This test EXISTS to pin the boundary. If someone adds coverage for one of
/// these, this test fails and forces the doc comment to be updated with it.
#[tokio::test]
async fn unrecognised_runners_are_not_applicable_and_the_doc_says_so() {
    for (command, args) in [
        ("bunx", vec!["evil-pkg"]),
        ("pnpm", vec!["dlx", "evil-pkg"]),
        ("yarn", vec!["dlx", "evil-pkg"]),
        ("npm", vec!["exec", "evil-pkg"]),
        ("uv", vec!["tool", "run", "evil-pkg"]),
        ("deno", vec!["run", "npm:evil-pkg"]),
    ] {
        let (outcome, calls) = queries(command, &args).await;
        assert_eq!(
            outcome,
            MalwareCheckOutcome::NotApplicable,
            "`{command} {}` — if this now resolves, add it to the doc comment",
            args.join(" ")
        );
        assert!(calls.is_empty());
    }
}

/// The indirect-runner case (core#340 item 1). A shell whose argv embeds a
/// package runner is NOT classifiable by this parser and is permitted. The
/// gate reads the CONFIGURED command; it does not analyse what the launched
/// program subsequently executes. Pinned here so the doc comment cannot drift
/// back into claiming otherwise.
#[tokio::test]
async fn a_shell_wrapped_runner_is_not_applicable() {
    let (outcome, calls) = queries("sh", &["-c", "npx -y evil-pkg"]).await;
    assert_eq!(outcome, MalwareCheckOutcome::NotApplicable);
    assert!(calls.is_empty());
}
