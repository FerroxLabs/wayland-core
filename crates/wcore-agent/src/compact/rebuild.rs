//! Post-compaction state rebuild block — the EXECUTION FRONTIER, rendered
//! from typed host state.
//!
//! What a compaction actually costs a user is not the goal. It is the
//! frontier: what has already been read, run and edited. codex#36664 measured
//! 74 compactions in one 5.9-hour session, 95% of them followed within two
//! minutes by re-reading a file or re-running a test already done that
//! session — and the re-fetch is what refills the window, so the loss feeds
//! itself.
//!
//! Every fact here is read from live host state and rendered deterministically.
//! NONE of it is asked of the summariser: a summariser can hallucinate a cwd,
//! a branch, or a file it never saw; typed state cannot. That is the whole
//! reason this block exists as a separate render step rather than as another
//! section of `prompt.rs`'s summarisation instructions — codex#19111 tried the
//! "instruct the model to preserve this verbatim" route and reported "to no
//! avail".
//!
//! The file ledgers cost roughly 50 bytes per path against the tens of
//! thousands of characters a needless re-read costs, and they carry no file
//! CONTENT — naming what was read is the cheap half of the value.

use std::collections::BTreeSet;
use std::path::PathBuf;

use wcore_types::file_state::Provenance;

/// Header the block is rendered under. Also the string the engine's own tests
/// and the conformance probes look for, so it is a constant rather than an
/// inline literal.
pub const REBUILD_HEADER: &str =
    "## Session state after compaction (recorded by the host, not summarised)";

/// Maximum tool names listed before the block truncates with a count.
/// A large MCP inventory can run to hundreds of tools; the point of the line
/// is "these are available", not a second copy of the tool schema the request
/// already carries in `tools[]`.
const MAX_TOOLS_LISTED: usize = 40;

/// Maximum paths listed per ledger before truncating with a count. At ~50
/// bytes a row this caps each ledger near 2 KB, which is the budget at which
/// the ledger is unambiguously cheaper than one avoided re-read.
const MAX_PATHS_LISTED: usize = 40;

/// Live host facts as of the moment of the fold. Plain data with no borrows so
/// the engine can assemble it while it still holds `&self` and render after it
/// has started mutating the buffer.
#[derive(Debug, Default, Clone)]
pub struct HostState {
    /// Process working directory.
    pub cwd: Option<String>,
    /// Current git branch, when the cwd is a work tree.
    pub git_branch: Option<String>,
    /// Whether plan mode is active (no mutations permitted).
    pub plan_mode: bool,
    /// Rendered approval policy, e.g. `"auto-approve"` / `"ask"`.
    pub approval_policy: String,
    /// Whether the tool registry is in read-only mode.
    pub read_only: bool,
    /// Tool names currently registered.
    pub tools: Vec<String>,
    /// `(path, provenance, cached bytes)` for every file the model has touched,
    /// most-recently-used first.
    pub files: Vec<(PathBuf, Provenance, usize)>,
}

/// Render the block, or `None` when there is nothing worth saying.
///
/// `None` matters: the block is spliced into every fold, and a fold on a
/// session that has touched nothing should not spend tokens announcing that.
/// (Codex's world-state renderers return `None` on "nothing changed" for the
/// same reason — `world_state/permissions.rs:87-121`.)
pub fn render(state: &HostState) -> Option<String> {
    let read: Vec<&(PathBuf, Provenance, usize)> = state
        .files
        .iter()
        .filter(|(_, p, _)| matches!(p, Provenance::ReadResult))
        .collect();
    let edited: Vec<&(PathBuf, Provenance, usize)> = state
        .files
        .iter()
        .filter(|(_, p, _)| matches!(p, Provenance::WriteEcho))
        .collect();

    if state.cwd.is_none() && state.tools.is_empty() && read.is_empty() && edited.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(1024);
    out.push_str(REBUILD_HEADER);
    out.push_str(
        "\nThese facts are read from the running session, so they are true even where the \
         summary above is incomplete.\n",
    );

    if let Some(cwd) = &state.cwd {
        out.push_str(&format!("\n- Working directory: {cwd}"));
        if let Some(branch) = &state.git_branch {
            out.push_str(&format!(" (git branch: {branch})"));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "- Plan mode: {}. Tool approval: {}. Read-only tools: {}.\n",
        if state.plan_mode { "ACTIVE" } else { "off" },
        state.approval_policy,
        if state.read_only { "yes" } else { "no" },
    ));

    if !state.tools.is_empty() {
        // Sorted + deduped: the block is rendered on every fold and an
        // unstable ordering would rewrite these bytes each time for no reason.
        let names: BTreeSet<&str> = state.tools.iter().map(String::as_str).collect();
        out.push_str(&format!("- Tools available ({}): ", names.len()));
        out.push_str(&join_capped(names.into_iter(), MAX_TOOLS_LISTED, ", "));
        out.push('\n');
    }

    render_ledger(
        &mut out,
        "Files ALREADY READ this session — do not re-read these unless you \
         have reason to believe they changed",
        &read,
    );
    render_ledger(
        &mut out,
        "Files ALREADY WRITTEN OR EDITED this session — the edit landed; do \
         not redo it",
        &edited,
    );

    Some(out)
}

fn render_ledger(out: &mut String, title: &str, rows: &[&(PathBuf, Provenance, usize)]) {
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!("\n{title} ({}):\n", rows.len()));
    for (path, _, bytes) in rows.iter().take(MAX_PATHS_LISTED) {
        out.push_str(&format!("  - {} ({bytes} bytes)\n", path.display()));
    }
    if rows.len() > MAX_PATHS_LISTED {
        out.push_str(&format!(
            "  - … and {} more\n",
            rows.len() - MAX_PATHS_LISTED
        ));
    }
}

fn join_capped<'a, I: Iterator<Item = &'a str>>(items: I, cap: usize, sep: &str) -> String {
    let all: Vec<&str> = items.collect();
    let head = all.iter().take(cap).copied().collect::<Vec<_>>().join(sep);
    if all.len() > cap {
        format!("{head}{sep}… and {} more", all.len() - cap)
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_files(files: Vec<(&str, Provenance, usize)>) -> HostState {
        HostState {
            cwd: Some("/work".into()),
            approval_policy: "ask".into(),
            files: files
                .into_iter()
                .map(|(p, prov, n)| (PathBuf::from(p), prov, n))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_state_renders_nothing() {
        assert!(render(&HostState::default()).is_none());
    }

    #[test]
    fn read_and_write_ledgers_are_separated_by_provenance() {
        let s = state_with_files(vec![
            ("/work/read_me.rs", Provenance::ReadResult, 120),
            ("/work/wrote_me.rs", Provenance::WriteEcho, 300),
        ]);
        let out = render(&s).expect("state is non-empty");
        let read_section = out.split("ALREADY WRITTEN").next().unwrap();
        let write_section = out.split("ALREADY WRITTEN").nth(1).unwrap();
        assert!(read_section.contains("read_me.rs"), "{out}");
        assert!(!read_section.contains("wrote_me.rs"), "{out}");
        assert!(write_section.contains("wrote_me.rs"), "{out}");
        assert!(!write_section.contains("read_me.rs"), "{out}");
    }

    #[test]
    fn ledger_carries_paths_but_never_content() {
        let s = state_with_files(vec![("/work/secret.rs", Provenance::ReadResult, 9)]);
        let out = render(&s).unwrap();
        assert!(out.contains("/work/secret.rs"));
        assert!(out.contains("9 bytes"));
    }

    #[test]
    fn oversized_ledger_truncates_with_a_count_instead_of_growing() {
        let files: Vec<(String, Provenance, usize)> = (0..MAX_PATHS_LISTED + 7)
            .map(|i| (format!("/work/f{i}.rs"), Provenance::ReadResult, 10))
            .collect();
        let s = HostState {
            cwd: Some("/work".into()),
            approval_policy: "ask".into(),
            files: files
                .into_iter()
                .map(|(p, prov, n)| (PathBuf::from(p), prov, n))
                .collect(),
            ..Default::default()
        };
        let out = render(&s).unwrap();
        assert!(out.contains("and 7 more"), "{out}");
        assert_eq!(
            out.matches("/work/f").count(),
            MAX_PATHS_LISTED,
            "exactly the cap should be listed"
        );
    }

    #[test]
    fn plan_mode_and_permission_tier_are_stated_explicitly() {
        let s = HostState {
            cwd: Some("/w".into()),
            plan_mode: true,
            approval_policy: "auto-approve".into(),
            read_only: true,
            ..Default::default()
        };
        let out = render(&s).unwrap();
        assert!(out.contains("Plan mode: ACTIVE"), "{out}");
        assert!(out.contains("Tool approval: auto-approve"), "{out}");
        assert!(out.contains("Read-only tools: yes"), "{out}");
    }

    #[test]
    fn tool_list_is_stable_across_renders_regardless_of_registry_order() {
        let a = HostState {
            cwd: Some("/w".into()),
            tools: vec!["Read".into(), "Bash".into(), "Write".into()],
            ..Default::default()
        };
        let b = HostState {
            tools: vec!["Write".into(), "Read".into(), "Bash".into()],
            ..a.clone()
        };
        assert_eq!(render(&a).unwrap(), render(&b).unwrap());
    }

    #[test]
    fn git_branch_is_rendered_beside_the_cwd_when_known() {
        let s = HostState {
            cwd: Some("/w".into()),
            git_branch: Some("fix/compaction-frontier".into()),
            ..Default::default()
        };
        assert!(
            render(&s)
                .unwrap()
                .contains("/w (git branch: fix/compaction-frontier)")
        );
    }
}
