use std::collections::HashSet;
use std::process::{Command, Output};

const EXPECTED_IDS: &[&str] = &[
    "canary",
    "persona_coder",
    "persona_web_builder",
    "persona_marketer",
    "persona_researcher",
    "persona_writer",
    "persona_contradictory",
    "persona_graceful_degradation",
    "qa_slash_style",
    "qa_slash_clear",
    "qa_approval_allow",
    "qa_approval_deny",
    "qa_cov_read",
    "qa_cov_write",
    "qa_cov_edit",
    "qa_cov_edit_replace",
    "qa_cov_bash",
    "qa_cov_grep",
    "qa_cov_glob",
    "qa_cov_repomap",
    "qa_cov_web_search",
    "qa_cov_web_fetch",
    "qa_cov_grep_write_chain",
    "qa_cov_read_edit_chain",
    "qa_cov_glob_read_chain",
    "qa_cov_plan_mode",
    "qa_cov_approval_deny",
    "mcp_echo_roundtrip",
    "hook_pre_blocks_write",
    "hook_stop_leaves_artifact",
    "protocol_set_config_model_swap",
    "protocol_set_mode_force_refused",
    "protocol_stop_mid_turn",
    "cron_create_recurring",
    "cron_list_jobs",
    "cron_create_then_list",
];

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wayland-eval"))
        .args(args)
        .output()
        .expect("run wayland-eval")
}

fn ids(stdout: &[u8]) -> Vec<&str> {
    std::str::from_utf8(stdout)
        .expect("stdout must be UTF-8")
        .lines()
        .filter(|line| !line.is_empty())
        .collect()
}

fn output_context(out: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn list_is_exact_unique_and_byte_stable() {
    let first = run(&["--list"]);
    let second = run(&["--list"]);

    assert!(first.status.success(), "{}", output_context(&first));
    assert!(second.status.success(), "{}", output_context(&second));
    assert!(first.stderr.is_empty(), "{}", output_context(&first));
    assert!(second.stderr.is_empty(), "{}", output_context(&second));
    assert_eq!(first.stdout, second.stdout, "listing must be byte-stable");

    let actual = ids(&first.stdout);
    assert_eq!(actual, EXPECTED_IDS);
    assert_eq!(actual.len(), 36);
    assert_eq!(
        actual.iter().copied().collect::<HashSet<_>>().len(),
        actual.len(),
        "scenario IDs must be unique"
    );
}

#[test]
fn scenario_is_repeatable_exact_and_catalog_ordered() {
    let out = run(&[
        "--list",
        "--scenario",
        "qa_slash_clear",
        "--scenario",
        "canary",
    ]);

    assert!(out.status.success(), "{}", output_context(&out));
    assert_eq!(ids(&out.stdout), ["canary", "qa_slash_clear"]);
    assert!(out.stderr.is_empty(), "{}", output_context(&out));
}

#[test]
fn scenario_rejects_a_substring_near_match() {
    let out = run(&["--scenario", "qa_approval"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "{}", output_context(&out));
    assert!(out.stdout.is_empty(), "{}", output_context(&out));
    assert!(
        stderr.contains("unknown scenario") && stderr.contains("qa_approval"),
        "{}",
        output_context(&out)
    );
}

#[test]
fn filter_selects_substrings_in_catalog_order() {
    let out = run(&["--list", "--filter", "approval"]);

    assert!(out.status.success(), "{}", output_context(&out));
    assert_eq!(
        ids(&out.stdout),
        [
            "qa_approval_allow",
            "qa_approval_deny",
            "qa_cov_approval_deny"
        ]
    );
    assert!(out.stderr.is_empty(), "{}", output_context(&out));
}

#[test]
fn zero_match_filter_exits_nonzero_before_execution() {
    let query = "__definitely_no_scenario__";
    let out = run(&["--filter", query]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "{}", output_context(&out));
    assert!(out.stdout.is_empty(), "{}", output_context(&out));
    assert!(
        stderr.contains("no scenarios matched") && stderr.contains(query),
        "{}",
        output_context(&out)
    );
}

#[test]
fn list_output_can_be_persisted_atomically() {
    let directory = tempfile::tempdir().expect("output tempdir");
    let destination = directory.path().join("catalog.txt");
    let out = run(&[
        "--list",
        "--scenario",
        "canary",
        "--output",
        destination.to_str().expect("UTF-8 test path"),
    ]);

    assert!(out.status.success(), "{}", output_context(&out));
    assert_eq!(out.stdout, b"canary\n");
    assert_eq!(
        std::fs::read(&destination).expect("persisted output"),
        out.stdout
    );
}
