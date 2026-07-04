//! Wave SA — BashTool credential-exfiltration denylist tests.
//!
//! v0.2.0 SECURITY audit MAJOR: BashTool returns full stdout to the
//! model, so any command that dumps env vars or named secrets exfils
//! credentials into the LLM's context. We refuse the obvious shapes
//! BEFORE invoking the shell. These tests exercise both the
//! refused-cases (each denylist pattern fires) and the allowed-cases
//! (the denylist is not over-broad).
//!
//! The denylist is queried via `wcore_tools::bash::check_denylist` —
//! a pure-string check that does NOT spawn a shell, so the tests are
//! deterministic and fast on every platform.

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::{BashTool, check_denylist};

fn assert_refused(cmd: &str) {
    let reason = check_denylist(cmd);
    assert!(
        reason.is_some(),
        "expected denylist to refuse {cmd:?} but it was allowed"
    );
    let msg = reason.unwrap();
    assert!(
        msg.contains("Refused"),
        "denylist message should start with 'Refused': {msg:?}"
    );
}

fn assert_allowed(cmd: &str) {
    let reason = check_denylist(cmd);
    assert!(
        reason.is_none(),
        "expected denylist to allow {cmd:?} but it refused with {reason:?}"
    );
}

// ---------------------------------------------------------------------------
// Refused cases — each maps to one or more denylist patterns from bash.rs.
// ---------------------------------------------------------------------------

#[test]
fn refuses_bare_env() {
    assert_refused("env");
    assert_refused("  env  ");
    assert_refused("ENV");
}

#[test]
fn allows_env_runner_idioms() {
    // #657 Part 2: `env VAR=value cmd` and `env -u/-i … cmd` are legitimate
    // runner idioms (set/unset a var for one command), NOT env dumps. They
    // are allowed; only a bare `env` (a pure dump) is refused. The sandbox
    // env is already secret-scrubbed, so these leak no credentials.
    assert_allowed("env FOO=bar /usr/bin/whoami");
    assert_allowed("env -u PATH /usr/bin/whoami");
    assert_allowed("env -i /usr/bin/node app.js");
    // But a bare env dump is still refused.
    assert_refused("env");
}

#[test]
fn refuses_bare_printenv() {
    assert_refused("printenv");
    assert_refused("  printenv  ");
}

#[test]
fn refuses_printenv_named_secret() {
    // Even with the `printenv\b` rule, the more-specific named-secret
    // rule guarantees coverage.
    assert_refused("printenv ANTHROPIC_API_KEY");
    assert_refused("printenv OPENAI_API_KEY");
    assert_refused("printenv AWS_SECRET_ACCESS_KEY");
}

#[test]
fn refuses_bare_set() {
    assert_refused("set");
    assert_refused("  set  ");
}

#[test]
fn refuses_powershell_env_enum() {
    assert_refused("Get-ChildItem env:");
    assert_refused("Get-ChildItem env:ANTHROPIC_API_KEY");
    assert_refused("$env:OPENAI_API_KEY");
}

#[test]
fn refuses_echo_named_env_var() {
    assert_refused("echo $ANTHROPIC_API_KEY");
    assert_refused("echo \"$OPENAI_API_KEY\"");
    assert_refused("echo $AWS_SECRET_ACCESS_KEY");
    assert_refused("echo $SOMETHING_TOKEN");
    assert_refused("echo $FOO_PASSWORD");
}

#[test]
fn refuses_reading_dotenv_files() {
    assert_refused("cat .env");
    assert_refused("cat /tmp/.env");
    assert_refused("cat /home/user/project/.env");
    assert_refused("tee .env.production");
    assert_refused("less .env");
    assert_refused("more .env");
    assert_refused("head .env");
    assert_refused("tail .env");
    assert_refused("head -n 5 .env");
}

#[test]
fn refuses_denylist_pattern_in_chained_subcommand() {
    // `;` separator
    assert_refused("ls -la; env");
    // `&&` separator
    assert_refused("ls && env");
    // `||` separator
    assert_refused("false || printenv");
    // pipe
    assert_refused("env | tee /tmp/x");
    // newline
    assert_refused("ls\nenv\n");
}

// ---------------------------------------------------------------------------
// Allowed cases — the denylist must NOT be over-broad.
// ---------------------------------------------------------------------------

#[test]
fn allows_ordinary_commands() {
    assert_allowed("ls");
    assert_allowed("ls -la");
    assert_allowed("pwd");
    assert_allowed("echo hello world");
    assert_allowed("echo 'hello, world!'");
    assert_allowed("git status");
    assert_allowed("cargo build");
    assert_allowed("rg --files");
}

#[test]
fn allows_echo_of_plain_text() {
    // `echo $HOME` is a common, non-secret env var — but we err on the
    // conservative side: only `*_API_KEY|SECRET|TOKEN|PASSWORD|PASSWD`
    // patterns match. `$HOME` and `$PATH` should pass.
    assert_allowed("echo $HOME");
    assert_allowed("echo $PATH");
    assert_allowed("echo $USER");
    // Echo of literal text containing the word "env" — must pass.
    assert_allowed("echo development environment");
    assert_allowed("echo 'my env vars'");
}

#[test]
fn allows_commands_that_mention_env_substring_but_arent_env_dumps() {
    // `envoy` starts with `env` but is a distinct command.
    assert_allowed("envoy --version");
    // `envrc` files — different from `.env`.
    assert_allowed("cat .envrc");
    // env-related cargo / npm subcommands.
    assert_allowed("cargo run --bin server");
}

#[test]
fn allows_env_template_reads() {
    // #657 Part 2: committed placeholder templates are readable for onboarding.
    assert_allowed("cat .env.example");
    assert_allowed("cat .env.sample");
    assert_allowed("head -n 20 .env.template");
    assert_allowed("cat config/.env.dist");
    // But the real secret file is still refused.
    assert_refused("cat .env");
    assert_refused("cat .env.production");
}

#[test]
fn allows_non_secret_powershell_env_vars() {
    // #657 Part 2: only secret-shaped `$env:` names are refused.
    assert_allowed("echo $env:PATH");
    assert_allowed("echo $env:USERPROFILE");
    assert_allowed("echo $env:NODE_ENV");
    // Secret-shaped names still refused.
    assert_refused("echo $env:MY_API_KEY");
    assert_refused("$env:OPENAI_API_KEY");
}

#[test]
fn allows_set_with_args() {
    // POSIX `set -e`, `set +x`, etc. are common shell-script flags;
    // only bare `set` (which dumps all vars) is denied.
    assert_allowed("set -e");
    assert_allowed("set +x");
    assert_allowed("set -euo pipefail");
}

// ---------------------------------------------------------------------------
// End-to-end via BashTool.execute() — ensures the wiring is right.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bash_execute_refuses_env_without_spawning_shell() {
    let tool = BashTool;
    let result = tool.execute(json!({"command": "env"})).await;
    assert!(result.is_error, "env must be refused");
    assert!(
        result.content.contains("Refused"),
        "expected refusal message, got {:?}",
        result.content
    );
    // Hard check: the refusal must NOT have run a shell that produced
    // env-style output. If the denylist had been bypassed, the result
    // would contain lines like `PATH=...` or `HOME=...`. Refuse any
    // hint of that.
    assert!(
        !result.content.contains("PATH="),
        "looks like env was actually executed: {}",
        result.content
    );
}

#[tokio::test]
async fn bash_execute_refuses_named_secret_echo() {
    let tool = BashTool;
    let result = tool
        .execute(json!({"command": "echo $ANTHROPIC_API_KEY"}))
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("Refused"));
}

#[tokio::test]
#[serial_test::serial]
async fn bash_execute_allows_normal_command() {
    // BashTool routes through wcore-sandbox, which fails closed when no real
    // backend can spawn (e.g. bwrap can't make user namespaces in an
    // unprivileged CI container). This test exercises the allow path, not
    // isolation, so opt into the documented no-sandbox degraded mode.
    // SAFETY: test-only env mutation; `#[serial]` prevents env races.
    unsafe {
        std::env::set_var("WAYLAND_SANDBOX", "none");
        std::env::set_var("WAYLAND_ALLOW_NO_SANDBOX", "1");
    }
    let tool = BashTool;
    let result = tool.execute(json!({"command": "echo wave_sa_smoke"})).await;
    assert!(!result.is_error, "got error: {}", result.content);
    assert!(
        result.content.contains("wave_sa_smoke"),
        "expected echo output, got {}",
        result.content
    );
}

// ─────────────────────────────────────────────────────────────────────
// v0.6.1 hardening (Sec3) — additional denylist coverage.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn v0_6_1_blocks_aws_credentials_read() {
    assert_refused("cat ~/.aws/credentials");
    assert_refused("less ~/.aws/config");
    assert_refused("head -n 5 ~/.aws/credentials");
}

#[test]
fn v0_6_1_blocks_ssh_key_read() {
    assert_refused("cat ~/.ssh/id_rsa");
    assert_refused("cat ~/.ssh/id_ed25519");
    assert_refused("cat ~/.ssh/identity");
}

#[test]
fn v0_6_1_blocks_netrc_npmrc_pypirc() {
    assert_refused("cat ~/.netrc");
    assert_refused("cat ~/.npmrc");
    assert_refused("cat ~/.pypirc");
}

#[test]
fn v0_6_1_blocks_kube_gcloud_azure_configs() {
    assert_refused("cat ~/.kube/config");
    assert_refused("cat ~/.gcloud/credentials.json");
    assert_refused("cat ~/.azure/accessTokens.json");
}

#[test]
fn v0_6_1_blocks_wayland_own_auth_file() {
    assert_refused("cat ~/.config/wayland/auth.json");
    assert_refused("cat /home/user/wayland/auth.json");
    assert_refused("cat /home/user/wayland-core/credentials.json");
}

#[test]
fn v0_6_1_blocks_etc_shadow_sudoers() {
    assert_refused("cat /etc/shadow");
    assert_refused("cat /etc/sudoers");
}

#[test]
fn v0_6_1_blocks_base64_encoding_of_secrets() {
    // Encoded exfil bypass attempts.
    assert_refused("base64 ~/.aws/credentials");
    assert_refused("xxd ~/.ssh/id_rsa");
    assert_refused("hexdump -C .env");
    assert_refused("openssl enc -base64 -in ~/.aws/credentials");
}

#[test]
fn v0_6_1_blocks_macos_keychain_extraction() {
    assert_refused("security find-generic-password -s SomeService");
    assert_refused("security dump-keychain");
    assert_refused("security export -t certs");
}

#[test]
fn v0_6_1_blocks_compgen_env_enum() {
    assert_refused("compgen -e");
    assert_refused("for v in $(compgen -e); do echo $v; done");
}

#[test]
fn v0_6_1_blocks_indirect_var_expansion() {
    assert_refused("echo ${!FOO_TOKEN}");
    assert_refused("for v in ${!FOO_*}; do echo $v; done");
}

#[test]
fn v0_6_1_blocks_printf_and_awk_env_dump() {
    assert_refused("printf %s $OPENAI_API_KEY");
    assert_refused("awk 'BEGIN{for(k in ENVIRON) print k, ENVIRON[k]}'");
}

#[test]
fn v0_6_1_blocks_set_minus_o_posix_dump() {
    assert_refused("set -o posix; set");
}

#[test]
fn v0_6_1_does_not_block_legitimate_usage() {
    // Sanity — these must STILL be allowed after the v0.6.1 additions.
    use wcore_tools::bash::check_denylist;
    let allowed = [
        "ls ~/.aws",                  // listing dir is fine
        "test -f ~/.aws/credentials", // existence check is fine
        "cat README.md",              // unrelated file
        "echo hello world",           // plain echo
        "base64 README.md",           // encoding a non-secret file
        "awk '{print $1}' data.txt",  // awk without ENVIRON
        "printf '%s\\n' hello",       // printf without secret var
    ];
    for cmd in allowed {
        assert!(
            check_denylist(cmd).is_none(),
            "denylist must not refuse legitimate command: {cmd:?}"
        );
    }
}
