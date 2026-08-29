p = "crates/wcore-cli/tests/doctor_probe_mcp_honours_the_gate_mode.rs"
s = open(p).read()
old = """        "[default]\\n\\
         provider = \\"anthropic\\"\\n\\
         api_key = \\"test-key-not-a-real-credential\\"\\n\\
         \\n\\
         [mcp]\\n\\"""
new = """        "[mcp]\\n\\"""
assert old in s, "config anchor miss"
s = s.replace(old, new, 1)

old2 = """    let cli = CliArgs {
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };"""
new2 = """    // Credentials on the command line, the way `--doctor` itself passes them
    // through (`doctor_args` in `main.rs`): this host has no keyring and a
    // `[default] api_key` key is not a recognised config setting, so a config
    // file cannot satisfy the resolver here.
    let cli = CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("test-key-not-a-real-credential".to_string()),
        project_dir: Some(project.path().to_path_buf()),
        ..CliArgs::default()
    };"""
assert old2 in s, "cli anchor miss"
s = s.replace(old2, new2, 1)
open(p, "w").write(s)
print("test fixed")
