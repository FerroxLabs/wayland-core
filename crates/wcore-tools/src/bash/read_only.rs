//! Static proof that one shell command could not have mutated anything.
//!
//! `Bash` is opaque by default and that is the correct default: a shell can
//! reach any state on the host and nothing can photograph the result
//! afterwards. But the opaque classification is applied to the whole tool,
//! and the majority of what an agent actually sends it — `ls`, `cat`,
//! `wc -l`, `grep -rn foo src` — cannot mutate anything at all. Every one of
//! those, interrupted by a crash, became a question for a human.
//!
//! This module answers only the cases it can PROVE, and refuses everything
//! else. The proof has three parts and all three must hold:
//!
//! 1. **Character allowlist.** Every byte of the command must come from a set
//!    with no meaning to the shell beyond word splitting. That is an allowlist,
//!    not a denylist, so an unmodelled construct fails toward opaque rather
//!    than sneaking through: `;`, `&`, `|`, `<`, `>`, `` ` ``, `$`, `(`, `)`,
//!    `{`, `}`, `[`, `]`, `*`, `?`, `~`, `!`, `#`, `\`, `=`, both quote marks
//!    and every newline are simply not in it. So a chained, redirected,
//!    substituted, globbed, quoted or `VAR=`-prefixed command is never
//!    classified here, and neither is anything the de-obfuscation pass in
//!    [`super::policy::deobfuscate`] exists to unpick.
//!
//! 2. **Program allowlist.** The first word must be one of a small set chosen
//!    against two rules: the program has no write mode reachable from its
//!    command line, AND it cannot dispatch to a user-configured external
//!    helper. The second rule is why `git` is absent even though `git status`
//!    is the single most common read-only call an agent makes: git runs a
//!    pager, `diff.external`, textconv filters and `core.fsmonitor` out of
//!    repository and user configuration, so no analysis of the ARGUMENTS can
//!    bound what a `git` invocation executes. `sort` (`-o`), `uniq` (its
//!    second positional argument), `sed` (`-i`), `awk` (`print >`, `system()`),
//!    `find` (`-delete`, `-exec`) and `env` (which execs its trailing words)
//!    are absent for the first rule.
//!
//! 3. **Per-program argument rules.** Only one program in the set has a
//!    mutating flag: `date -s` sets the system clock. It is admitted only with
//!    format arguments.
//!
//! What this buys is stated precisely: a command that passes is one whose
//! effect on the world is a read, so an interruption has nothing for an
//! operator to have an opinion about and recovery settles it without asking.
//! A command that fails any part keeps exactly the recovery it had before
//! this module existed.

/// Characters that carry no meaning to the shell beyond separating words.
///
/// ASCII only. A non-ASCII byte in a path is perfectly legitimate and simply
/// is not classified here — silence is this module's safe direction.
fn is_inert_shell_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            ' ' | '\t' | '-' | '_' | '.' | '/' | '+' | ',' | ':' | '@' | '%'
        )
}

/// Programs with no command-line write mode and no dispatch to a
/// user-configured external helper. See the module header for what is
/// deliberately absent and why.
const READ_ONLY_PROGRAMS: &[&str] = &[
    "arch",
    "basename",
    "cat",
    "cksum",
    "cut",
    "date",
    "df",
    "dirname",
    "du",
    "echo",
    "egrep",
    "false",
    "fgrep",
    "file",
    "grep",
    "head",
    "id",
    "ls",
    "md5sum",
    "nproc",
    "printenv",
    "printf",
    "ps",
    "pwd",
    "readlink",
    "realpath",
    "rg",
    "seq",
    "sha1sum",
    "sha256sum",
    "sleep",
    "stat",
    "tail",
    "tr",
    "true",
    "uname",
    "uptime",
    "wc",
    "which",
    "whoami",
];

/// `date` prints when its arguments are formats and SETS THE SYSTEM CLOCK when
/// they are not (`-s`, `--set`). Only the printing form is read-only, and the
/// printing form is exactly "every argument begins with `+`".
fn arguments_are_read_only(program: &str, arguments: &[&str]) -> bool {
    match program {
        "date" => arguments.iter().all(|arg| arg.starts_with('+')),
        _ => true,
    }
}

/// True only when `command` is proven to mutate nothing.
///
/// False is the answer for everything else, including everything this
/// classifier does not model. It is never an assertion that the command DOES
/// mutate something.
pub(super) fn is_provably_read_only(command: &str) -> bool {
    if !command.chars().all(is_inert_shell_char) {
        return false;
    }
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    if !READ_ONLY_PROGRAMS.contains(&program) {
        return false;
    }
    let arguments: Vec<&str> = words.collect();
    arguments_are_read_only(program, &arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ordinary_read_only_calls_an_agent_makes_are_classified() {
        for command in [
            "pwd",
            "ls",
            "ls -la /etc",
            "cat Cargo.toml",
            "wc -l src/main.rs",
            "grep -rn needle src",
            "rg --files-with-matches needle",
            "head -n 20 README.md",
            "tail -n 5 log.txt",
            "stat Cargo.lock",
            "du -sh target",
            "df -h /root",
            "whoami",
            "uname -a",
            "date +%s",
            "sha256sum Cargo.toml",
            "which cargo",
            "sleep 1",
        ] {
            assert!(
                is_provably_read_only(command),
                "should be provably read-only: {command}"
            );
        }
    }

    /// Every shell construct that lets a read-only-looking program line reach
    /// a write. Each of these is a command whose FIRST WORD is on the
    /// allowlist, so only the character rule can refuse it — which is the
    /// point of making that rule an allowlist.
    #[test]
    fn a_shell_construct_that_can_write_is_never_classified() {
        for command in [
            "cat a > b",
            "cat a >> b",
            "cat a | tee b",
            "ls; rm -rf /",
            "ls && rm -rf /",
            "ls || rm -rf /",
            "echo $(rm -rf /)",
            "echo `rm -rf /`",
            "echo ${HOME}",
            "cat <b",
            "ls *",
            "ls ?",
            "ls [ab]",
            "ls ~",
            "echo hi\nrm -rf /",
            "cat 'a b'",
            "cat \"a b\"",
            "cat a\\;rm",
            "FOO=bar ls",
            "ls &",
            "echo hi #; rm -rf /",
            "ls !!",
        ] {
            assert!(
                !is_provably_read_only(command),
                "a shell construct must never be classified read-only: {command}"
            );
        }
    }

    /// The programs deliberately left out, and the write each one reaches.
    #[test]
    fn a_program_with_a_write_mode_or_a_configurable_helper_is_never_classified() {
        for command in [
            "sort -o out in",
            "uniq in out",
            "sed -i s/a/b/ f",
            "awk {print} f",
            "find . -delete",
            "env FOO=bar rm -rf /",
            "git status",
            "git diff",
            "tee out",
            "cp a b",
            "mv a b",
            "rm a",
            "touch a",
            "mkdir a",
            "chmod 777 a",
            "curl https://example.com",
            "python script.py",
            "sh script.sh",
            "bash script.sh",
        ] {
            assert!(
                !is_provably_read_only(command),
                "must not be classified read-only: {command}"
            );
        }
    }

    /// `date` is the one admitted program with a mutating flag.
    #[test]
    fn date_is_admitted_only_in_its_printing_form() {
        assert!(is_provably_read_only("date"));
        assert!(is_provably_read_only("date +%Y-%m-%d"));
        assert!(
            !is_provably_read_only("date -s 2020-01-01"),
            "`date -s` sets the system clock"
        );
        assert!(!is_provably_read_only("date --set 2020-01-01"));
        assert!(
            !is_provably_read_only("date -u"),
            "an unmodelled flag is refused, not guessed at"
        );
    }

    /// A path to the same program is not the program: `./ls` is whatever the
    /// working directory happens to hold.
    #[test]
    fn a_program_named_by_path_is_not_the_allowlisted_program() {
        assert!(!is_provably_read_only("./ls"));
        assert!(!is_provably_read_only("/bin/ls"));
        assert!(!is_provably_read_only("bin/ls"));
        assert!(!is_provably_read_only(""));
        assert!(!is_provably_read_only("   "));
    }
}
