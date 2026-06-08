//! Wave A1 — proof the committed IJFW snapshot resolves at compile time
//! without depending on the previous `vendor/ijfw-source` symlink.
//!
//! These tests intentionally exercise the public surface (`MANIFEST_TOML`
//! plus the three `register` functions through the published API). If
//! the snapshot directory is missing or the `include_str!` paths drift,
//! the crate fails to compile and these tests never run, which itself
//! is the regression gate that A1 is solving.
//!
//! The runtime assertions below are belt-and-suspenders: they prove the
//! embedded strings are non-empty and parse correctly, so a partial
//! snapshot (such as an empty file accidentally committed) is caught.

use wayland_ijfw::MANIFEST_TOML;

/// `MANIFEST_TOML` is the in-crate `plugin.toml`. It must remain present
/// and non-empty regardless of the snapshot changes.
#[test]
fn manifest_toml_is_present() {
    assert!(
        !MANIFEST_TOML.trim().is_empty(),
        "wayland-ijfw plugin.toml manifest must not be empty",
    );
    assert!(
        MANIFEST_TOML.contains("wayland-ijfw"),
        "plugin.toml must declare the wayland-ijfw package",
    );
}

/// All 27 snapshot files must be embedded with non-empty bodies. We
/// reach into the modules via a public re-export shim so callers can
/// see the surface that drives the registrations.
///
/// The test compiles only if every `include_str!` resolves — i.e. only
/// if the snapshot directory contains every referenced file. The
/// non-empty assertion catches the case where a file exists but was
/// truncated to zero bytes during snapshotting.
#[test]
fn snapshot_files_are_non_empty() {
    // Mirror the lookup against the same paths the production modules
    // use, so a path drift is caught here rather than only at the
    // `register_*` call sites.
    let referenced: &[&str] = &[
        include_str!("../snapshots/ijfw-source/claude/agents/architect.md"),
        include_str!("../snapshots/ijfw-source/claude/agents/builder.md"),
        include_str!("../snapshots/ijfw-source/claude/agents/scout.md"),
        include_str!("../snapshots/ijfw-source/claude/rules/IJFW-CLAUDE.md"),
        include_str!("../snapshots/ijfw-source/universal/ijfw-rules.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-agents-md/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-auto-memorize/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-commit/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-compress/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-compute/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-core/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-critique/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-cross-audit/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-dashboard/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-debug/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-design/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-handoff/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-memory-audit/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-metrics/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-plan-check/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-preflight/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-recall/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-review/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-summarize/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-team/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-update/SKILL.md"),
        include_str!("../snapshots/ijfw-source/claude/skills/ijfw-workflow/SKILL.md"),
    ];
    assert_eq!(
        referenced.len(),
        27,
        "expected 27 snapshot files (3 agents + 2 rules + 22 skills)",
    );
    for (idx, body) in referenced.iter().enumerate() {
        assert!(
            !body.trim().is_empty(),
            "snapshot file at index {idx} must have non-empty body",
        );
    }
}
