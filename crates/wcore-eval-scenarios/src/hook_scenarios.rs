//! D7 — hook-fixture scenarios.
//!
//! These exercise wayland-core's **lifecycle hooks** (the `[[hooks.*]]`
//! config surface in `wcore-config::hooks`) through the real binary, end to
//! end. Two guarantees are covered:
//!
//!   1. A `pre_tool_use` hook that exits non-zero **blocks** the matched
//!      mutating tool — the engine substitutes a `Blocked by hook: …` tool
//!      result and the write never lands. `FileAbsent` proves enforcement.
//!   2. A `stop` hook fires when the session ends and leaves an observable
//!      filesystem artifact. `FileExists` proves the hook ran.
//!
//! # Hook block-contract (verified against source, not docs)
//!
//! `wcore-config/src/hooks.rs::ShellHooks::run_pre_tool_use` runs each matching
//! `pre_tool_use` hook's `command` as a child process and, on
//! `!status.success()` (i.e. any **non-zero exit code**), returns
//! `HookError::Blocked { hook_name, output }`. `docs/advanced.md` §"Hook
//! System": *"`pre_tool_use` — Before tool execution — Non-zero exit blocks the
//! tool."* The agent side
//! (`wcore-agent/src/orchestration/mod.rs:578-584`) catches that error and
//! emits a synthetic tool result with `content = "Blocked by hook: {reason}"`
//! INSTEAD of executing the tool — so a blocked `Write`/`Bash` produces no file.
//!
//! `stop` hooks run via `ShellHooks::run_stop`
//! (`engine.rs::run_stop_hooks`) at session end; they are non-blocking and
//! ignore exit code, so the assertion is purely "did the side effect happen".
//!
//! # Matching
//!
//! `tool_match` is a glob list over the tool NAME (empty = all). `file_match`
//! is a glob list over the tool's `file_path` input (empty = all). Env vars
//! `${TOOL_NAME}`, `${TOOL_INPUT_FILE_PATH}`, `${TOOL_INPUT_COMMAND}` are
//! interpolated into `command` before execution.
//!
//! ===========================================================================
//! WIRING NEEDED (the harness cannot run these as-is — two seams must close):
//! ===========================================================================
//!
//! ## (A) The runner must INVOKE `scenario.setup` before spawning the binary.
//!
//! As of this commit, `Scenario::setup` stores a closure but NOTHING in
//! `runner.rs` ever calls it (verified: the only reference to `.setup` in
//! `src/` is the builder assignment). Without this, the hook config below is
//! never written and BOTH scenarios silently degrade (the agent just writes
//! the file → false FAIL on #1, no artifact → FAIL on #2). In
//! `runner.rs::run_session_in`, immediately AFTER the working dir exists and
//! BEFORE `spawn_for_run`, add:
//!
//! ```ignore
//! if let Some(setup) = &scenario.setup {
//!     setup(cwd).map_err(|e| anyhow::anyhow!("scenario setup failed: {e}"))?;
//! }
//! ```
//!
//! (Symmetrically, call `scenario.cleanup` on the way out if desired — not
//! required for D7.)
//!
//! ## (B) The hook config must land in the file tempenv ALREADY wrote.
//!
//! `tempenv::build` writes `<cwd>/.wayland-core/config.toml` (DIR form) with
//! the absolute `[session].directory` + provider key. `config.rs`
//! `project_config_path()` PREFERS `.wayland-core.toml` (FILE form) when BOTH
//! exist (F-011). Therefore `setup()` must **append** the `[[hooks.*]]` blocks
//! to the EXISTING dir-form file — creating the file form would shadow it and
//! drop the session dir + key. The `append_hooks_config` helper below does
//! exactly that (open-append `<cwd>/.wayland-core/config.toml`).
//!
//! No `tempenv.rs` change is needed: the project-level config in cwd is read
//! by the engine's normal cwd-walk, and `merge_config` concatenates
//! `project.hooks.pre_tool_use` / `.stop` into the effective config
//! (`config.rs:1939-1941`). This is the preferred path — it requires no
//! `TempEnvOptions` extension.
//!
//! If (A) is judged out of scope for this crate's runner, an alternative is to
//! teach `tempenv::build_with` a `hooks_toml: Option<String>` option and write
//! it inline — but that edits `tempenv.rs`, which (A) avoids.

use std::time::Duration;

use crate::assertions::Assertion;
use crate::providers::ProviderChoice;
use crate::scenario::{Category, Scenario, Turn};

/// Relative path (under the scenario cwd) of the config file tempenv seeds and
/// that the engine reads via its cwd-walk. `setup()` appends hook blocks here.
const CONFIG_REL: &str = ".wayland-core/config.toml";

/// Append `[[hooks.*]]` TOML to the tempenv-seeded `config.toml` in `cwd`.
///
/// Open-append (NOT truncate) so the `[session]` + `[provider.*]` blocks
/// tempenv already wrote survive. A leading newline guarantees the new table
/// header starts on its own line regardless of how the seed file ended.
fn append_hooks_config(cwd: &std::path::Path, hooks_toml: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let path = cwd.join(CONFIG_REL);
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("open {} for append: {e}", path.display()))?;
    writeln!(f, "\n{hooks_toml}")
        .map_err(|e| anyhow::anyhow!("append hooks to {}: {e}", path.display()))?;
    Ok(())
}

/// Write a small executable shell script into `cwd` and return nothing; the
/// script path is referenced from the hook `command`. We write a `.sh` and
/// invoke it via `sh <script>` from the hook command so the exit-code / touch
/// semantics are explicit and platform-portable enough for the macOS/Linux CI
/// lanes (the hook command itself runs through `shell_command_builder`, i.e.
/// `sh -c` on unix / `cmd /C` on windows).
fn write_script(cwd: &std::path::Path, name: &str, body: &str) -> anyhow::Result<()> {
    let path = cwd.join(name);
    std::fs::write(&path, body)
        .map_err(|e| anyhow::anyhow!("write script {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// Hooks-QA: a `pre_tool_use` hook denies every `Write` by exiting non-zero,
/// so the agent's attempt to create a file is BLOCKED and the file never
/// lands. `FileAbsent` proves the block was enforced at the engine boundary
/// (not merely that the model declined).
///
/// The hook matches `tool_match = ["Write"]` and runs `block_write.sh`, which
/// prints a reason and `exit 1`. Per the block-contract (module docs), a
/// non-zero pre-hook → `HookError::Blocked` → synthetic "Blocked by hook"
/// tool result → no filesystem effect.
pub fn pre_hook_blocks_write() -> Scenario {
    Scenario::new("hook_pre_blocks_write", Category::Hardening)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(120))
        .max_total_cost_usd(0.05)
        .setup(|cwd| {
            // Script: deny the Write. Non-zero exit = block.
            write_script(
                cwd,
                "block_write.sh",
                "#!/bin/sh\n\
                 echo \"policy: Write denied for ${TOOL_INPUT_FILE_PATH}\" 1>&2\n\
                 exit 1\n",
            )?;
            // Hook config: PreToolUse on Write → run the deny script.
            append_hooks_config(
                cwd,
                "[[hooks.pre_tool_use]]\n\
                 name = \"deny-write\"\n\
                 tool_match = [\"Write\"]\n\
                 command = \"sh block_write.sh\"\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "Create a file called hooked.txt containing exactly the word HELLO. \
                 If a tool is blocked, stop and report it — do not retry with a different tool.",
            )
            .max_time(Duration::from_secs(100))
            .max_steps(6)
            // The write must NOT land — the pre-hook blocked it.
            .assert(Assertion::FileAbsent("hooked.txt")),
        )
}

/// Hooks-QA: a `stop` hook fires at session end and `touch`es a flag file.
/// `FileExists` after the run proves the stop hook actually executed.
///
/// The turn itself is a trivial no-op question so the run terminates quickly;
/// the artifact is produced by the lifecycle hook, NOT by any tool the model
/// chose to call.
pub fn stop_hook_leaves_artifact() -> Scenario {
    Scenario::new("hook_stop_leaves_artifact", Category::Hardening)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(90))
        .max_total_cost_usd(0.03)
        .setup(|cwd| {
            // Script: leave an observable artifact at session end.
            write_script(
                cwd,
                "on_stop.sh",
                "#!/bin/sh\n\
                 printf 'stop hook ran\\n' > stop_ran.flag\n",
            )?;
            // Hook config: Stop → run the artifact script.
            append_hooks_config(
                cwd,
                "[[hooks.stop]]\n\
                 name = \"leave-flag\"\n\
                 command = \"sh on_stop.sh\"\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new("Reply with the single word: done.")
                .max_time(Duration::from_secs(60))
                .max_steps(3)
                // The stop hook wrote this after the session ended.
                .assert(Assertion::FileExists("stop_ran.flag"))
                .assert(Assertion::FileContains {
                    path: "stop_ran.flag",
                    needle: "stop hook ran",
                }),
        )
}

/// All D7 hook-fixture scenarios, in a stable order.
pub fn all() -> Vec<Scenario> {
    vec![pre_hook_blocks_write(), stop_hook_leaves_artifact()]
}
