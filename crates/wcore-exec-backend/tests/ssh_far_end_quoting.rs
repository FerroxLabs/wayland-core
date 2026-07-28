//! Does a value survive the far end's SHELL?
//!
//! `ssh host cmd a b c` does not carry an argument vector. The client joins its
//! remote-command arguments with spaces and the far end's login shell re-splits
//! the string. So the safety of the ssh backend rests on quoting each value for
//! *that* shell, and the only honest way to test quoting is to hand the result
//! to a real shell and look at what comes back.
//!
//! This lives in an integration test rather than beside `posix_quote` for one
//! concrete reason: the ssh module carries a guard asserting that its own source
//! contains no shell-string execution path, and that guard scans the file's text.
//! Writing a shell invocation into that file — even in a test — would trip it.
//! The guard is correct and stays; the shell round-trip belongs out here.
//!
//! These tests are Unix-only because they need a POSIX shell to be the
//! authority. That is not a gap: the ssh backend's remote runner requires
//! `setsid`, `base64` and `ps -eo`, so its far end is POSIX by construction.

#![cfg(unix)]

/// Ask a real `sh` what argv it received, given a command string built the way
/// ssh builds one: the quoted values joined with single spaces.
///
/// Returns the far-end argv, one element per line, so a dropped, split or
/// executed value is directly visible.
async fn far_end_argv(quoted_values: &[String]) -> (Vec<String>, String) {
    // `printf '%s\n'` on each positional is the smallest faithful reader of
    // argv: it prints exactly what the shell bound, with no re-interpretation.
    let script = r#"for a in "$@"; do printf '%s\n' "$a"; done"#;
    let command_string = format!("set -- {}; {}", quoted_values.join(" "), script);

    let output = wcore_config::shell::shell_command(&command_string)
        .await
        .expect("a POSIX shell must be runnable on a unix test host");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let argv = stdout.lines().map(str::to_string).collect();
    (argv, stdout)
}

/// The three shapes measured breaking against a live far end on 2026-07-28,
/// each round-tripped through a real shell.
#[tokio::test]
async fn quoted_values_reach_the_far_end_shell_byte_for_byte() {
    let originals = vec![
        // Measured: vanished entirely, shifting every later argument left.
        String::new(),
        // Measured: arrived as two arguments.
        "hello world".to_string(),
        // Measured: EXECUTED on the far end as root.
        "x;id>/tmp/w;echo y".to_string(),
        // An attempt to break out of the quoting itself.
        "a';id>/tmp/w;'b".to_string(),
        // Ordinary values must be unharmed.
        "f25-reference-nonce".to_string(),
        "sleep".to_string(),
        "600".to_string(),
        // Other metacharacter families.
        "$(id)".to_string(),
        "`id`".to_string(),
        "*".to_string(),
        "a\\b".to_string(),
        "tab\there".to_string(),
    ];

    let quoted: Vec<String> = originals
        .iter()
        .map(|v| wcore_exec_backend::backends::ssh::posix_quote(v))
        .collect();

    let (argv, raw) = far_end_argv(&quoted).await;

    // Nothing was dropped and nothing was split.
    assert_eq!(
        argv.len(),
        originals.len(),
        "the far end received {} arguments for {} values.\nraw:\n{raw}",
        argv.len(),
        originals.len()
    );
    // Every byte survived. `printf '%s\n'` cannot represent an embedded
    // newline unambiguously, and none of the values above carries one.
    for (sent, received) in originals.iter().zip(argv.iter()) {
        assert_eq!(sent, received, "a value was altered crossing the shell");
    }
    // Nothing was executed: the far end never saw `uid=`, which every `id`
    // implementation prints.
    assert!(
        !raw.contains("uid="),
        "a value executed on the far end:\n{raw}"
    );
}

/// The positive control. Without quoting, the assertions above MUST fail —
/// otherwise they are measuring nothing.
///
/// This reproduces the original defect deliberately, so the test file itself
/// demonstrates that the round-trip can go red.
#[tokio::test]
async fn without_quoting_the_same_round_trip_is_corrupted_and_executes() {
    let originals = vec![
        String::new(),
        "hello world".to_string(),
        "x;id;echo y".to_string(),
    ];
    // The pre-fix behaviour: values pushed onto ssh's remote command with no
    // quoting at all.
    let unquoted: Vec<String> = originals.clone();
    let (argv, raw) = far_end_argv(&unquoted).await;

    // The empty value vanished and the spaced value split, so the count cannot
    // match. That is the corruption that shifted task argv left in the field.
    assert_ne!(
        argv.len(),
        originals.len(),
        "unquoted values round-tripped intact, so the control proves nothing:\n{raw}"
    );

    // And the metacharacter value executed. This is the finding, reproduced.
    assert!(
        raw.contains("uid="),
        "the unquoted control did not execute, so the executed-value assertion \
         in the test above is not actually load-bearing:\n{raw}"
    );
}
