#!/usr/bin/env python3
"""Revert all SEVEN lane/win-engine changes, one at a time or all at once.

The lane's own earlier mutant covered five of the seven changes and was only
ever executed on Linux, where every `cfg(windows)` branch is dead code and the
suite therefore came back fully green — a mutant that cannot go red proves
nothing. This script exists so the mutation can be applied ON WINDOWS, where
the reverted code is the code that actually runs, and so the two changes the
old mutant missed are covered too.

Usage:
    win-engine-mutate.py list
    win-engine-mutate.py apply <name>|all
    win-engine-mutate.py revert            # restore every file from .orig

Each mutation is an exact string replacement; a mutation whose search text is
not found is a hard error, so a rebased or edited tree can never silently
produce an "all green" mutant run.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# name -> (relative path, exact text to replace, replacement)
MUTATIONS = {
    # 1. The production Windows implementation of the observation primitive.
    #    Reverting it restores `ErrorKind::Unsupported`, i.e. the state in
    #    which the whole durable-receipt / crash-reconciliation / rollback
    #    path was inert on Windows.
    "vfs-windows-observe": (
        "crates/wcore-tools/src/vfs.rs",
        """    #[cfg(windows)]
    {
        observe_real_file_windows(path)
    }""",
        """    #[cfg(windows)]
    {
        // MUTATION PROOF ONLY
        let _ = (path, observe_real_file_windows);
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "identity-aware file observation is unavailable on this platform",
        )))
    }""",
    ),
    # 2. Production. #661 error discipline: without the existence pre-check the
    #    answer depends on whether the installed backend exits 2 for an absent
    #    target, which `findstr` does not.
    "grep-target-exists": (
        "crates/wcore-tools/src/grep.rs",
        """    match tokio::fs::try_exists(&resolved).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("grep error: no such file or directory: {path}"),
                is_error: true,
            };
        }
        Err(error) => {
            return ToolResult {
                content: format!("grep error: cannot access {path}: {error}"),
                is_error: true,
            };
        }
    }""",
        """    let _ = &resolved; // MUTATION PROOF ONLY""",
    ),
    # 3. Production. Windows ancestor walk mapping ENOENT to NotFound rather
    #    than to a component-open failure that names the PARENT.
    "media-intake-notfound": (
        "crates/wcore-tools/src/media_intake.rs",
        """                if e.kind() == std::io::ErrorKind::NotFound {
                    return IntakeError::NotFound(path.to_path_buf());
                }
""",
        "",
    ),
    # 4. Fixture. Candidate checkout root must be absolute on Windows too.
    "gate-candidate-root": (
        "crates/wcore-agent/src/child_transaction/gate_executor.rs",
        """            root: Ok(if cfg!(windows) {
                PathBuf::from(r"C:\\srv\\wayland\\candidate\\checkout")
            } else {
                PathBuf::from("/srv/wayland/candidate/checkout")
            }),""",
        """            root: Ok(PathBuf::from("/srv/wayland/candidate/checkout")),""",
    ),
    # 5. Fixture. NOT covered by the lane's original mutant: the private
    #    WRITABLE root, one line below the candidate root and refused by a
    #    different guard.
    "gate-writable-root": (
        "crates/wcore-agent/src/child_transaction/gate_executor.rs",
        """    fn private_scratch_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\\srv\\wayland\\private\\scratch")
        } else {
            PathBuf::from("/srv/wayland/private/scratch")
        }
    }""",
        """    fn private_scratch_root() -> PathBuf {
        PathBuf::from("/srv/wayland/private/scratch")
    }""",
    ),
    # 6. Fixture. Durable-receipt workspace path.
    "journal-workspace-file": (
        "crates/wcore-agent/src/journal_effects.rs",
        """        if cfg!(windows) {
            r"C:\\workspace\\file.txt"
        } else {
            "/workspace/file.txt"
        }""",
        """        "/workspace/file.txt\"""",
    ),
    # 7. Fixture. NOT covered by the lane's original mutant: both halves of the
    #    transcription refusal case — the absent-but-valid path, and comparing
    #    the refused path in its JSON-encoded form.
    "transcription-missing-path": (
        "crates/wcore-tools/src/transcription_tools.rs",
        """        let missing = if cfg!(windows) {
            r"C:\\nonexistent\\path\\audio.mp3"
        } else {
            "/nonexistent/path/audio.mp3"
        };""",
        """        let missing = "/nonexistent/path/audio.mp3";""",
    ),
    "transcription-json-encoded-compare": (
        "crates/wcore-tools/src/transcription_tools.rs",
        """        let named = serde_json::to_string(missing).expect("encode refused path");
        assert!(
            r.content.contains(named.trim_matches('"')),""",
        """        assert!(
            r.content.contains(missing),""",
    ),
}


def backup_path(relative: str) -> Path:
    return ROOT / (relative + ".mutorig")


def apply(name: str) -> None:
    relative, old, new = MUTATIONS[name]
    target = ROOT / relative
    backup = backup_path(relative)
    if not backup.exists():
        backup.write_text(target.read_text(encoding="utf-8"), encoding="utf-8")
    source = target.read_text(encoding="utf-8")
    if old not in source:
        raise SystemExit(
            f"MUTATION {name}: search text not found in {relative} — the tree "
            f"has moved and this mutant would have been silently vacuous"
        )
    target.write_text(source.replace(old, new, 1), encoding="utf-8")
    print(f"applied {name} -> {relative}")


def revert() -> None:
    for relative, _, _ in MUTATIONS.values():
        backup = backup_path(relative)
        if backup.exists():
            (ROOT / relative).write_text(
                backup.read_text(encoding="utf-8"), encoding="utf-8"
            )
            backup.unlink()
            print(f"reverted {relative}")


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    command = sys.argv[1]
    if command == "list":
        for name, (relative, _, _) in MUTATIONS.items():
            print(f"{name}\t{relative}")
    elif command == "apply":
        which = sys.argv[2]
        if which == "all":
            for name in MUTATIONS:
                apply(name)
        else:
            apply(which)
    elif command == "revert":
        revert()
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
