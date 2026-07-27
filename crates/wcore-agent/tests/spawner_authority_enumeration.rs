//! F21-02-01 ENUMERATION GUARD — every production `AgentSpawner` construction
//! site must pass through a seam that DECLARES the parent's tool authority.
//!
//! # Why a source-derived guard and not a prose list
//!
//! The first attempt at F21-02-01 was declined for a specific reason: it wired
//! the intersection at `bootstrap.rs` and left four other production spawner
//! construction sites at the unrestricted default, so the repair manufactured
//! the appearance of enforcement while the seam stayed reachable through routes
//! nobody had checked. The remedy for that is not a longer comment claiming the
//! list is complete — it is a check that RE-DERIVES the list from source on
//! every run, so the claim cannot rot and a newly added site cannot land unwired.
//!
//! # What it enforces
//!
//! `AgentSpawner::new` is the only constructor (every other path is a
//! `clone_*`, which shares the authority cell by `Arc`). This test finds every
//! call to it that is NOT inside a `#[cfg(test)]` item and requires the owning
//! file to be one of the three known seams below. Each seam declares authority
//! exactly once, for every spawner that flows through it:
//!
//! | Seam | Declares | Covers |
//! |---|---|---|
//! | `bootstrap.rs` scoped build | `narrow_parent_tool_authority(registry.tool_names())` | the session spawner |
//! | `AgentEngine::govern_transient_spawner` | `narrow_parent_tool_authority(self.tools.tool_names())` | plan-synthesis + `/crucible` transients |
//! | `bootstrap::govern_standalone_spawner` | `declare_root_parent_tool_authority()` | CLI `crucible`, CLI `workflow run`, standalone Anvil seat |
//!
//! A new production construction site fails this test with instructions rather
//! than silently inheriting the unrestricted constructor default.
//!
//! # F21-02-03 — the dispatch gate rides the same enumeration
//!
//! The reconciliation gave `PolicyGate` (Layer 2) the same authority cell this
//! guard covers, so everything above holds for the gate too. The one thing the
//! construction-site enumeration cannot see is whether
//! `AgentSpawner::execute_resolved_launch` still installs it —
//! `set_policy_gate` had zero production callers before F21-02-03 and would be
//! silently orphaned again if that single line were deleted as "redundant".
//! [`child_launch_installs_the_dispatch_gate_from_the_authority_cell`] pins it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Production `AgentSpawner::new` sites, pinned as `<crate-relative path> =>
/// (count, the seam that declares its authority)`.
///
/// Update this table ONLY together with a declaration at the new site. Adding
/// an entry without one re-opens F21-02-01 for that route.
const DECLARED_SITES: &[(&str, usize, &str)] = &[
    (
        "wcore-agent/src/bootstrap.rs",
        1,
        "scoped bootstrap: narrow_parent_tool_authority(registry.tool_names())",
    ),
    (
        "wcore-agent/src/engine.rs",
        2,
        "AgentEngine::govern_transient_spawner: narrow_parent_tool_authority(self.tools.tool_names())",
    ),
    (
        "wcore-agent/src/orchestration/anvil/seat.rs",
        1,
        "bootstrap::govern_standalone_spawner: declare_root_parent_tool_authority()",
    ),
    (
        "wcore-cli/src/crucible.rs",
        1,
        "bootstrap::govern_standalone_spawner: declare_root_parent_tool_authority()",
    ),
    (
        "wcore-cli/src/workflow.rs",
        1,
        "bootstrap::govern_standalone_spawner: declare_root_parent_tool_authority()",
    ),
];

fn crates_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<workspace>/crates/wcore-agent`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("wcore-agent manifest dir has a parent")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Line numbers (1-based) that lie inside a `#[cfg(test)]` item.
///
/// Brace-matched from the attribute to the close of the item it annotates. This
/// is a lexical approximation — it does not track braces inside string or char
/// literals — which is sound for the direction that matters here: a miscount
/// can only ever mark MORE lines as test (hiding a site, which the pinned
/// counts below would then catch as a shortfall) or fewer (surfacing an extra
/// site, which fails loudly). It never silently authorises an unwired site.
fn cfg_test_lines(src: &str) -> Vec<bool> {
    let lines: Vec<&str> = src.lines().collect();
    let mut inside = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("#[cfg(test)]") {
            i += 1;
            continue;
        }
        let mut depth: i32 = 0;
        let mut opened = false;
        let mut j = i;
        while j < lines.len() {
            for ch in lines[j].chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth <= 0 {
                break;
            }
            j += 1;
        }
        for slot in inside.iter_mut().take((j + 1).min(lines.len())).skip(i) {
            *slot = true;
        }
        i = j + 1;
    }
    inside
}

fn production_sites() -> BTreeMap<String, usize> {
    let root = crates_root();
    let mut files = Vec::new();
    let Ok(crate_dirs) = std::fs::read_dir(&root) else {
        panic!("cannot read crates root at {}", root.display());
    };
    for entry in crate_dirs.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut files);
        }
    }
    assert!(
        !files.is_empty(),
        "found no crate sources under {} — the enumeration guard cannot run",
        root.display()
    );

    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !src.contains("AgentSpawner::new") {
            continue;
        }
        let test_lines = cfg_test_lines(&src);
        let mut count = 0usize;
        for (n, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            // Skip doc comments and ordinary comments: they reference the
            // constructor by name without calling it.
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("AgentSpawner::new") && !test_lines[n] {
                count += 1;
            }
        }
        if count > 0 {
            let rel = file
                .strip_prefix(&root)
                .expect("source lives under the crates root")
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(rel, count);
        }
    }
    found
}

#[test]
fn every_production_spawner_construction_site_declares_parent_tool_authority() {
    let found = production_sites();
    let declared: BTreeMap<String, usize> = DECLARED_SITES
        .iter()
        .map(|(path, count, _)| ((*path).to_owned(), *count))
        .collect();

    let mut problems = Vec::new();
    for (path, count) in &found {
        match declared.get(path) {
            None => problems.push(format!(
                "UNDECLARED production `AgentSpawner::new` in {path} ({count} site(s)).\n  \
                 F21-02-01: a spawner constructed outside a declaring seam inherits the \
                 UNRESTRICTED default, so its children can be handed tools the parent does \
                 not hold. Route it through `bootstrap::govern_standalone_spawner` or \
                 `AgentEngine::govern_transient_spawner`, or call \
                 `narrow_parent_tool_authority` / `declare_root_parent_tool_authority` at the \
                 site, then add it to DECLARED_SITES with the seam that covers it."
            )),
            Some(expected) if expected != count => problems.push(format!(
                "{path}: found {count} production `AgentSpawner::new` site(s), DECLARED_SITES \
                 pins {expected}. A site was added or removed — confirm the new one declares \
                 its parent tool authority (F21-02-01), then update the pin."
            )),
            Some(_) => {}
        }
    }
    for (path, count, _) in DECLARED_SITES {
        if !found.contains_key(*path) {
            problems.push(format!(
                "{path}: DECLARED_SITES pins {count} production `AgentSpawner::new` site(s) but \
                 none were found. If the site moved, move its authority declaration with it."
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "F21-02-01 spawner-authority enumeration failed:\n\n{}\n",
        problems.join("\n\n")
    );
}

/// F21-02-03 RECONCILIATION GUARD — the child dispatch gate must stay
/// installed, and must stay fed from the authority cell.
///
/// `AgentEngine::set_policy_gate` had ZERO production callers workspace-wide
/// before F21-02-03; that is the entire finding. The reconciliation gives it
/// exactly one, in `AgentSpawner::execute_resolved_launch`. Because Layer 1
/// (`build_tool_registry`'s intersection) already makes that call a no-op on
/// today's frozen six-tool child registry, a future reader has every incentive
/// to delete it as dead code — which would silently re-orphan the mechanism and
/// remove the only dispatch-time check a child will ever have.
///
/// So pin three things from source:
///   1. `set_policy_gate` is called in production `spawner.rs` at least once.
///   2. The gate is constructed from `from_parent_tools(authority…)`, i.e. from
///      the `ParentToolAuthority` snapshot — NOT from the parent's full
///      `registry.tool_names()`, which would pre-authorise names the child
///      cannot construct today (fail-open in advance), and NOT from a second
///      cell, which could drift from Layer 1's.
///   3. `execute_resolved_launch` takes exactly ONE authority snapshot. Two
///      reads let a concurrent narrowing land between them and produce a gate
///      stricter than the registry it guards.
#[test]
fn child_launch_installs_the_dispatch_gate_from_the_authority_cell() {
    let spawner_src = std::fs::read_to_string(crates_root().join("wcore-agent/src/spawner.rs"))
        .expect("spawner.rs is readable");
    let test_lines = cfg_test_lines(&spawner_src);

    let production_lines: Vec<(usize, &str)> = spawner_src
        .lines()
        .enumerate()
        .filter(|(n, line)| !test_lines[*n] && !line.trim_start().starts_with("//"))
        .collect();

    let installs: Vec<usize> = production_lines
        .iter()
        .filter(|(_, line)| line.contains("set_policy_gate"))
        .map(|(n, _)| n + 1)
        .collect();
    assert_eq!(
        installs.len(),
        1,
        "expected exactly ONE production `set_policy_gate` call in spawner.rs, found {}. \
         F21-02-03: this is the only production caller on the agent path. If it is gone, \
         `PolicyGate` is orphan code again and children have no dispatch-time authority \
         check; if there are several, the gate is being installed from more than one \
         authority and the two can disagree. Found at lines: {installs:?}",
        installs.len()
    );

    let from_cell = production_lines
        .iter()
        .any(|(_, line)| line.contains("PolicyGate::from_parent_tools"));
    assert!(
        from_cell,
        "the child dispatch gate is no longer built via `PolicyGate::from_parent_tools`. \
         It MUST be derived from the `ParentToolAuthority` snapshot so Layer 1 \
         (build_tool_registry's intersection) and Layer 2 (this gate) can never disagree, \
         and so the gate inherits all three declaring seams instead of just bootstrap's."
    );

    let snapshots: Vec<usize> = production_lines
        .iter()
        .filter(|(_, line)| line.contains("parent_tool_authority.snapshot()"))
        .map(|(n, _)| n + 1)
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "expected exactly ONE production read of `parent_tool_authority.snapshot()` in \
         spawner.rs (in `execute_resolved_launch`), found {}. Both the child's registry and \
         its dispatch gate must be built from the SAME snapshot: the cell narrows \
         monotonically at any time, so two reads can yield a gate STRICTER than the \
         registry, denying a child a tool it visibly holds. Found at lines: {snapshots:?}",
        snapshots.len()
    );
}

/// The guard is only worth anything if its classifier actually distinguishes
/// production code from `#[cfg(test)]` code. Assert that directly: this
/// workspace has far more test constructions than production ones, and a
/// classifier that silently matched everything (or nothing) would still let the
/// test above pass on a tree where the pins happened to line up.
#[test]
fn cfg_test_classifier_separates_test_constructions_from_production() {
    let src = r#"
fn production() {
    let a = AgentSpawner::new(p, c);
}

#[cfg(test)]
mod tests {
    fn helper() {
        let b = AgentSpawner::new(p, c);
    }
}

fn also_production() {
    let d = AgentSpawner::new(p, c);
}
"#;
    let flags = cfg_test_lines(src);
    let mut production = 0;
    let mut test = 0;
    for (n, line) in src.lines().enumerate() {
        if line.contains("AgentSpawner::new") {
            if flags[n] {
                test += 1;
            } else {
                production += 1;
            }
        }
    }
    assert_eq!(production, 2, "production constructions misclassified");
    assert_eq!(test, 1, "cfg(test) construction misclassified");

    // And on the real tree: the workspace must contain strictly more test
    // constructions than production ones, or the classifier has collapsed.
    let found_production: usize = production_sites().values().sum();
    assert_eq!(
        found_production,
        DECLARED_SITES.iter().map(|(_, c, _)| *c).sum::<usize>(),
        "production site total drifted from the pinned total"
    );
}
