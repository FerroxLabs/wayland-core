//! TEMPORARY reproduction harness for the refuted cost of the #693 floor.
//! Deleted after the measurement; the surviving form lives in
//! `ordinary_work_is_untouched`.

use wcore_tools::bash::check_command_floor;

#[test]
fn repro_table() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    std::fs::create_dir_all(root.join(".git/hooks")).expect("mkdir hooks");
    std::fs::write(root.join(".git/config"), "SENTINEL\n").expect("seed config");
    std::fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\n").expect("seed hook");
    std::fs::create_dir_all(root.join(".wayland-core/skills/x")).expect("mkdir skills");
    std::fs::write(root.join(".wayland-core/skills/x/SKILL.md"), "s\n").expect("seed skill");
    std::fs::write(root.join(".wayland-core.toml"), "t\n").expect("seed toml");

    let cases = [
        r#"git commit -m "fix .git/config parsing""#,
        "grep -rn wayland .wayland-core",
        "ls .wayland-core/skills",
        "cat .wayland-core/skills/x/SKILL.md",
        "git config --file .git/config --list",
        "cat .git/hooks/pre-commit",
        "ls -la .git/hooks",
        "echo see .wayland-core.toml for config",
    ];

    let mut refused = 0usize;
    println!("| # | verdict | command |");
    println!("|---|---------|---------|");
    for (i, c) in cases.iter().enumerate() {
        let r = check_command_floor(c, Some(&root));
        if r.is_some() {
            refused += 1;
        }
        println!(
            "| {} | {} | `{}` |",
            i + 1,
            if r.is_some() { "REFUSED" } else { "allowed" },
            c
        );
    }
    println!("REFUSED_COUNT={} of {}", refused, cases.len());

    // Known-positive controls IN THE SAME RUN, so a predicate that answered
    // `None` to everything could not produce a clean table.
    let deny_ctl = check_command_floor("printf x > .git/hooks/pre-commit", Some(&root));
    let allow_ctl = check_command_floor("echo hello", Some(&root));
    println!(
        "CONTROL write-.git/hooks/pre-commit = {}",
        if deny_ctl.is_some() {
            "REFUSED"
        } else {
            "allowed"
        }
    );
    println!(
        "CONTROL echo-hello = {}",
        if allow_ctl.is_some() {
            "REFUSED"
        } else {
            "allowed"
        }
    );
    assert!(
        deny_ctl.is_some(),
        "control: authoring a hook must be refused"
    );
    assert!(allow_ctl.is_none(), "control: `echo hello` must be allowed");

    assert_eq!(
        refused,
        0,
        "REFUSED_COUNT={} of {} — ordinary reads are refused",
        refused,
        cases.len()
    );
}
