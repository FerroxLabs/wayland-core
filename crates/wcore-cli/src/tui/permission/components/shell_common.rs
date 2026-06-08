//! Shared shell-command helpers for the Bash + PowerShell permission
//! components (SPEC §2 Bash / PowerShell, Wave 3 scaffold).
//!
//! Three pure functions, no I/O, no state — each is unit-tested on its
//! return value:
//!
//!  * [`infer_shell_prefix`] — the editable always-allow prefix prefilled
//!    when the user presses `a` on a shell card. The first command token
//!    plus its subcommand, ending in a space so the W0 `AlwaysPrefix`
//!    matcher scopes auto-approval by head only (`cargo test --lib` →
//!    `cargo test `).
//!  * [`destructive_warning`] — the real guard against a reflexive Enter
//!    (AGENTS §0 #3): flags known-dangerous commands so the Bash/PowerShell
//!    body can paint a `theme.error`-bold warning line above the keys.
//!  * [`is_sed_edit`] — detects `sed -i … <file>` so the Bash component can
//!    re-route to the FileEdit body (show the change as a diff).

use std::path::PathBuf;

/// Infer the always-allow prefix from a shell command. The first token
/// plus its subcommand, ending in a space so the W0 matcher scopes by
/// head (`cargo test --lib` -> `cargo test `). Single-token commands
/// return just the token + space (`ls` -> `ls `). An empty command
/// returns the empty string.
pub fn infer_shell_prefix(command: &str) -> String {
    let mut it = command.split_whitespace();
    let Some(cmd) = it.next() else {
        return String::new();
    };
    match it.next() {
        // A subcommand that is not a flag becomes part of the prefix.
        Some(sub) if !sub.starts_with('-') => format!("{cmd} {sub} "),
        _ => format!("{cmd} "),
    }
}

/// Flag destructive shell commands. Returns a human warning when the
/// command matches a known-dangerous pattern, else `None`. This is the
/// real guard against a reflexive Enter (AGENTS §0 #3).
///
/// Covers POSIX-shell hazards (`rm -rf`, `git push --force`, raw `dd`,
/// `mkfs`, the classic fork bomb, raw block-device writes, recursive
/// world-writable chmod) and the PowerShell equivalents the PowerShell
/// component shares (`Remove-Item -Recurse -Force`, `Format-Volume`).
pub fn destructive_warning(command: &str) -> Option<String> {
    let lc = command.to_lowercase();
    let danger = lc.contains("rm -rf")
        || lc.contains("rm -fr")
        || lc.contains("git push --force")
        || lc.contains("git push -f")
        || lc.contains(" dd ")
        || lc.starts_with("dd ")
        || lc.contains("mkfs")
        || lc.contains(":(){") // fork bomb
        || lc.contains("> /dev/sd")
        || lc.contains("chmod -r 777")
        || lc.contains("remove-item -recurse -force")
        || lc.contains("format-volume");
    danger.then(|| "destructive command — review carefully before approving".to_string())
}

/// Detect `sed -i ... <file>` so the Bash component can re-route to the
/// FileEdit body (show the edit as a diff). Returns the target file when
/// the command is an in-place `sed`, else `None`.
///
/// The `-i` flag may be bare (`-i`) or carry a backup-suffix
/// (`-i.bak`, GNU/BSD style), so we match both `== "-i"` and the
/// `-i`-prefixed form. The target file is the last non-flag token.
pub fn is_sed_edit(command: &str) -> Option<PathBuf> {
    let toks: Vec<&str> = command.split_whitespace().collect();
    if toks.first() != Some(&"sed") {
        return None;
    }
    if !toks.iter().any(|t| *t == "-i" || t.starts_with("-i")) {
        return None;
    }
    // Last non-flag token is the target file.
    toks.iter()
        .rev()
        .find(|t| !t.starts_with('-'))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_command_plus_subcommand() {
        assert_eq!(infer_shell_prefix("cargo test --lib"), "cargo test ");
        assert_eq!(infer_shell_prefix("npm run build"), "npm run ");
        assert_eq!(infer_shell_prefix("ls -la"), "ls ");
        assert_eq!(infer_shell_prefix("ls"), "ls ");
        assert_eq!(infer_shell_prefix(""), "");
        // Leading/trailing whitespace is ignored (split_whitespace).
        assert_eq!(infer_shell_prefix("  git   status  "), "git status ");
    }

    #[test]
    fn flags_destructive_commands_only() {
        assert!(destructive_warning("rm -rf /tmp/x").is_some());
        assert!(destructive_warning("rm -fr /tmp/x").is_some());
        assert!(destructive_warning("git push --force origin main").is_some());
        assert!(destructive_warning("git push -f").is_some());
        assert!(destructive_warning("dd if=/dev/zero of=/dev/sda").is_some());
        assert!(destructive_warning("mkfs.ext4 /dev/sdb1").is_some());
        assert!(destructive_warning(":(){ :|:& };:").is_some());
        // PowerShell hazards (shared with the PowerShell component).
        assert!(destructive_warning("Remove-Item -Recurse -Force C:\\tmp").is_some());
        assert!(destructive_warning("Format-Volume -DriveLetter D").is_some());
        // Safe commands are not flagged.
        assert!(destructive_warning("cargo test").is_none());
        assert!(destructive_warning("ls -la").is_none());
        assert!(destructive_warning("git push origin main").is_none());
    }

    #[test]
    fn detects_sed_inplace_edit() {
        assert_eq!(
            is_sed_edit("sed -i s/a/b/ file.txt"),
            Some(PathBuf::from("file.txt"))
        );
        // Backup-suffix form of -i still counts as in-place.
        assert_eq!(
            is_sed_edit("sed -i.bak s/a/b/ file.txt"),
            Some(PathBuf::from("file.txt"))
        );
        assert_eq!(is_sed_edit("sed s/a/b/ file.txt"), None); // no -i
        assert_eq!(is_sed_edit("grep foo file.txt"), None); // not sed
        assert_eq!(is_sed_edit(""), None);
    }
}
