• This is an audit task — read-only investigation of specific files. The user wants precise file:line evidence. I should delegate exploration but I also need precise quotes. The task involves reading several specific files:

  1. crates/wcore-agent/tests/transactional_delegated_mutation_test.rs:757 — capacity probe PowerShell command
  2. crates/wcore-sandbox/tests/live_fs_acl.rs:217, 387
  3. crates/wcore-sandbox/src — sandbox script building (cmd/type)
  4. crates/wcore-swarm/tests/dispatch_smoke.rs
  5. hard_process_containment_windows.rs (job object)

  I could do this myself with Grep/Read, but there's a lot. Let me do targeted reads myself since precision matters and the files are known. Let me start with parallel Grep/Read.

  Let me search for the capacity probe code.

• I'll verify against the source directly. Starting with the exact failure sites.

• Interesting — the test file already quotes the path in `type_and_hold`: `type "{}"`. But `type_file` (line 147-157) does NOT quote: `"type".into(), file.display().to_string()` — that's argv mode, so quoting doesn't apply (each arg separate). Wait, in type_file the args are separate argv entries: cmd.exe /c type <path>. When Rust's Command passes multiple args to cmd.exe, they're joined into a command line. Rust's Command on Windows quotes args with spaces automatically. So the file path with no spaces under C:\Users\Public should be fine.

  Now the key question: does the error message "The filename, directory name, or volume label syntax is incorrect." relate to `\\?\` prefix? Where would a `\\?\` prefix come from? The test's seed_file uses `C:\Users\Public\...` — no `\\?\` prefix. Unless `dir.clone()` in fs_read_allow is fine, but the path passed to `type`... Hmm.

  Wait — where would `\\?\` appear? In failure #1, the temp path is `\?\C:\Windows\ServiceProfiles\NetworkService\AppData\Local\Temp\.tmpXXXX\checkouts`. The capacity probe passes `self.swarm_root.to_string_lossy()` as `$args[0]`. PowerShell `-Command ... $args[0]` — the swarm_root contains a `\\?\` prefix. How? Something canonicalizes the path. `std::fs::canonicalize` on Windows returns `\\?\` prefixed paths. tempfile's TempDir doesn't canonicalize by default, but... the swarm_root may have been canonicalized somewhere.

  The PowerShell failure: `Unexpected token '\?\C:\...\checkouts' in expression or statement` — the script is passed with `-Command SCRIPT root`. With `-Command`, PowerShell parses everything after -Command as a command line. If SCRIPT doesn't consume args via $args... actually with `-Command`, trailing args ARE available via $args. But the issue: PowerShell with `-Command` re-parses the entire command line; when a `\\?\C:\...` path follows, PowerShell tries to parse it as part of the command! Actually with `-Command`, everything after it is treated as the command text, and remaining tokens become arguments only if... Let me recall: `powershell.exe -Command "script" arg1` — the args after the command string are available in $args. But there's a known quirk: `-Command` consumes all remaining arguments and re-tokenizes them. A path starting with `\\?\` gets re-parsed by PowerShell and can break. The error "Unexpected token '\\?\C:\...\checkouts' in expression or statement" indicates PowerShell parsed the path as part of the script expression, not as an argument. Known PowerShell behavior: with `-Command`, if the command is followed by arguments, PowerShell may concatenate them. The standard fix is `-File` or use `--%` stop-parsing, or embed the argument differently.

  Actually the known behavior: `powershell -Command <script> <args>` — PowerShell docs say: with -Command, all remaining arguments are interpreted as part of the command to execute. So the path is re-tokenized as PowerShell script text. `\\?\C:\...` is not valid PowerShell syntax → ParserError. So yes, hypothesis confirmed for #1 in mechanism, though the trigger is the `\\?\` prefix in swarm_root (which comes from canonicalization somewhere).

  Wait but would it fail without `\\?\`? `C:\Windows\...\checkouts` as re-tokenized PowerShell: `C:\Windows\...` — a bare token with colon and backslashes... PowerShell would try to interpret `C:\...` as a command/expression. Hmm, actually `$drive.AvailableFreeSpace)"` ends the script; then `C:\path` would be a separate statement → "Unexpected token" too? Actually a path like `C:\foo\bar` alone is treated as a command name (native command invocation attempt) — that would give "CommandNotFound" not ParserError. The `\\?\` prefix gives ParserError because `\\?\` is invalid syntax. So the observed ParserError specifically confirms `\\?\` in swarm_root.

  Where does `\\?\` come from? Look at where swarm_root is set — likely `std::fs::canonicalize` in worktree manager setup or in the test fixture. The temp dir path `C:\Windows\ServiceProfiles\NetworkService\AppData\Local\Temp\.tmpXXXX` — tempfile. And `checkouts` subdir. Something canonicalized it (e.g., `.canonicalize()` or dunce missing).

  Also #2: the sandbox `type` failure — "The filename, directory name, or volume label syntax is incorrect." That's cmd's error when given a `\\?\` path. But the test builds path from `%PUBLIC%` + join — no canonicalize there. Unless... the AppContainer backend itself canonicalizes argv or the manifest paths? Or the file display... `file.display()` where file = dir.join("granted.txt"), dir from env PUBLIC. No `\\?\`.

  Hmm, but maybe the AppContainer backend canonicalizes fs_read_allow paths and then... no, the type command gets `file.display()` directly from the test. So where does the syntax error come from? Could be the cmd line: `type C:\Users\Public\wcore-r61-PID-granted\granted.txt` — under AppContainer, %SystemRoot% etc. Actually the error "The filename, directory name, or volume label syntax is incorrect" (ERROR_INVALID_NAME 123) is different from "Access is denied" (ERROR_ACCESS_DENIED 5). If the ACL grant failed, `type` would print "Access is denied." Getting ERROR_INVALID_NAME suggests something else — maybe the path contains something weird. PID in dir name is fine.

  Wait — maybe the backend wraps the command in its own script. I need to read the AppContainer backend source to see how it builds the sandbox command. Maybe it writes a batch file with the path interpolated, and the batch file path or working directory has issues. Or maybe the backend prepends `\\?\` when canonicalizing manifest paths and passes the canonicalized path into the command. Let me read the appcontainer backend.

  Also need: hard_process_containment_windows.rs:157,329; dispatch_smoke.rs. And where swarm_root gets canonicalized.

  This is a lot of reading — good case for parallel explore agents, but I need precise quotes. Let me do targeted reads myself since I've been efficient so far. Read the AppContainer backend.

• The test file's story is more nuanced than the hypothesis. Now the AppContainer backend and the job-object test.appcontainer
appcontainer.rs
bwrap_landlock.rs
bwrap_seccomp.rs
bwrap.rs
docker_tests.rs
docker.rs
mod.rs
no_sandbox.rs
process_tree.rs
sandbox_exec.rs
backends
bin
directory_authority_archive.rs
directory_authority_file.rs
directory_authority_tests.rs
directory_authority_windows_tests.rs
directory_authority_windows.rs
directory_authority.rs
error.rs
lib.rs
manifest.rs
process_capture.rs


• The command.rs doesn't build the command line from argv — that's in process.rs probably. Let me look at how the command line is built and how the spawned process gets argv, and where `quote_arg` is used. Also need to check how the AppContainer backend handles the fs_read_allow paths (acl_lease canonicalize at acl_lease.rs:436 returns canonical path — used for ACL granting, fine).

  The question for #2: why does `type C:\Users\Public\...\granted.txt` fail with "The filename, directory name, or volume label syntax is incorrect"? Note — the seeded file path has no `\\?\`. Unless... wait, does the sandbox rewrite argv? Or does cmd under AppContainer have a different working directory issue? Or — `type` in cmd with an unquoted path containing no spaces should work.

  Hmm, but maybe the command line building uses quote_arg incorrectly, or joins argv with the script. Let me check process.rs for command-line construction.

  Also key: the hypothesis says cmd/type rejects `\\?\` paths. That's true — cmd.exe does not support `\\?\` prefix ("The filename, directory name, or volume label syntax is incorrect" is exactly the cmd error for `type \\?\C:\...`). But where would `\\?\` enter in live_fs_acl.rs? The test passes plain paths. Unless the backend canonicalizes the fs_read_allow paths and REWRITES argv? Let me check process.rs / mod.rs of appcontainer.

• Now the process-spawn path where argv becomes a command line.crates/wcore-sandbox/src/backends/appcontainer/:
acl_lease
acl_lease.rs
windows_impl

crates/wcore-sandbox/src/backends/appcontainer/windows_impl/:
command.rs
handles.rs
process.rs
tests.rs


• The command line is built with `quote_arg` and joined. argv[0] in the command line is the raw argv[0] (e.g. "cmd.exe") but lpApplicationName is the resolved program. Note: for `type_file`, argv = ["cmd.exe", "/c", "type", "C:\Users\Public\wcore-r61-PID-granted\granted.txt"]. No `\\?\` prefix there — so hypothesis #2's mechanism (cmd/type rejects \\?\) requires the file path to have a `\\?\` prefix. It doesn't... unless the cwd handling or something else. Wait — where is the path `file.display()`? The test seed uses %PUBLIC%. So no verbatim prefix.

  Hmm, but wait — the test was reported as failing with stderr "The filename, directory name, or volume label syntax is incorrect." Could that come from `type` being given a malformed command line? Let's think: `cmd.exe /c type C:\Users\Public\wcore-r61-1234-granted\granted.txt`. Nothing wrong syntactically. ERROR_INVALID_NAME from `type` occurs when path contains invalid chars... Actually, hold on — when Rust test's `std::process::Command` runs the backend... no, the backend builds cmdline itself.

  Alternative cause: under AppContainer with Low-IL restricted token, `cmd /c type <path>` — `type` opens the file; if access denied, cmd prints "Access is denied." ERROR_INVALID_NAME (123) text is odd. Could it come from cmd interpreting the path oddly? Actually there's a known behavior: `type` with a path containing no invalid chars but the file being a reserved name... no.

  Hmm — but wait. Maybe the real problem: `cmd.cwd` — cwd_w. In type_file, cwd: None. Fine.

  Let me reconsider: maybe the actual failure is earlier in the pipeline. The runner is NT AUTHORITY\NetworkService. LOCALAPPDATA = C:\Windows\ServiceProfiles\NetworkService\AppData\Local. AppContainer profile creation under NetworkService... The test failing is at line 217 (granted read exit 0 assertion) with stderr "The filename...". So spawn worked, cmd ran, `type` failed with ERROR_INVALID_NAME.

  Why would `type` return ERROR_INVALID_NAME on `C:\Users\Public\wcore-r61-<pid>-granted\granted.txt`? Hmm, one possibility: the command-line the child sees is not what we think. quote_arg only quotes when whitespace — the path has no spaces, so unquoted. Fine.

  Wait — is it possible the path got mangled by Rust's `PathBuf` when the runner sets PUBLIC to something odd? Under NetworkService, %PUBLIC% should still be C:\Users\Public. 

  Hmm, but what about `type_and_hold`: `type "{path}" && (...)`. In `cmd_script`, argv = ["cmd.exe","/d","/s","/c", script]. quote_arg quotes the whole script because it contains spaces. So cmdline becomes: `cmd.exe /d /s /c "type "C:\...\granted.txt" && (...)"`. Note quote_arg's escaping: inner quotes get backslash-escaped as `\"` per CRT rules, but cmd.exe does NOT understand `\"` — cmd uses `""` or caret. This is the classic mismatch! The script `type "C:\foo" && (...)` contains embedded `"` chars. quote_arg produces `"type \"C:\foo\" && (...)"`. When CreateProcess gives this to cmd.exe, cmd's own parser handles the command line after /c: cmd strips... Actually cmd.exe parses its command line specially; the CRT in cmd.exe would first parse argv via CommandLineToArgvW: argv would be ["cmd.exe", "/d", "/s", "/c", "type \"C:...\" && (...)"]. Wait no — CommandLineToArgvW on `"type \"C:\foo\" && (...)"` — MSVC rules: `\"` is a literal quote, so the arg becomes `type "C:\foo" && (...)` — correct round-trip! Because cmd.exe uses the CRT parsing to reconstruct argv, and then for /c it rejoins argv[1..]. Actually cmd.exe does NOT use CRT argv parsing for its command line — cmd parses the raw command line itself looking for /c. Hmm, this is subtle. Known guidance: cmd /c with embedded quotes needs the whole thing wrapped: cmd /c ""script"" or /s. With /s, cmd strips first and last quote and executes the rest literally. 

  Hmm wait — but this test reportedly passed on previous runs? Failure #3 mentions the exit code being "choice's selection index" — the test comment at lines 118-130 says they already fixed that with `exit /b 0`. The current failure says A's read exit != 0, "not choice's selection index" — meaning exit code was 1 (choice's index), meaning the `&&` block never ran because `type` failed. Same for #2: stderr "The filename, directory name, or volume label syntax is incorrect."

  So both #2 and #3: `type` fails with ERROR_INVALID_NAME. That's cmd's `type` complaining about the path syntax. Under AppContainer... what path does cmd see? Actually — wait. Does `type` in cmd print that message for access-denied under AppContainer? No, access denied prints "Access is denied."

  Could %TEMP% remapping matter? No.

  Hmm, what about the COMSPEC/env: the env block includes only the listed vars plus manifest.env. OK.

  Key insight candidate: `type` with quoted path — cmd's `type` internal command doesn't strip quotes? Actually `type "path"` works fine in cmd.

  Let me reconsider ERROR_INVALID_NAME 123. When does `type` emit that? When the filename contains characters invalid per DOS parsing, e.g. `?`, `*`, `:` in wrong position... OR when the path starts with `\\?\`! `type \\?\C:\foo` → "The filename, directory name, or volume label syntax is incorrect." That's the classic cmd error for extended-length paths — cmd.exe does not accept the `\\?\` prefix.

  So where would `\\?\` come from in live_fs_acl? The test builds from %PUBLIC%... unless PUBLIC env var under the test runner... no.

  Hmm wait, actually let me re-check: in `type_file` the path is `file.display()`. `file = dir.join("granted.txt")`, `dir = PathBuf::from(public).join(...)`. public = env var "PUBLIC" or fallback. No canonicalize. So the path given to type is plain `C:\Users\Public\...`.

  Unless — under the AppContainer, the *granted path* ACL works, but cmd itself... Actually! Maybe the issue is different: ERROR_INVALID_NAME is what cmd's `type` reports when CreateFile fails with ERROR_INVALID_NAME, which can happen when... the AppContainer named-object isolation? No.

  Hold on, maybe I should check: does the backend REWRITE the argv paths, e.g., canonicalizing fs paths in the command? Let me grep for where fs_read_allow is used and whether argv is touched. Also `acl_lease.rs:436` canonicalizes grant paths — for icacls? Let me check how the ACE is applied (icacls subprocess? SetNamedSecurityInfo?). acl_lease/storage.rs:789 builds `\\?\{spelling}` — interesting! Line 789: `let extended_path = PathBuf::from(format!(r"\\?\{spelling}"));` — something creates a `\\?\`-prefixed path. Where is that used? If that extended path leaks into the spawned command or the icacls grant... Let me read that.

  Also for #2, maybe the granted path's ACE was applied to `\\?\C:\...` spelling via icacls, and icacls failed, so no ACE → access denied. But error message is INVALID_NAME not ACCESS_DENIED...

  Actually wait — reread failure #2: stderr = "The filename, directory name, or volume label syntax is incorrect." Whose stderr? The sandboxed cmd's stderr. `type` printed it. So `type` rejected the path string it received. The path string came from the test's `file.display()`. For `type` to reject it with INVALID_NAME, the path as seen by cmd must be malformed — e.g., containing `\?` or quotes mismatch. In type_and_hold (used by #2's read task — line 203 uses type_and_hold!), the script is `type "C:\Users\Public\wcore-r61-PID-granted\granted.txt" && (%SystemRoot%\System32\choice.exe /T 3 /D Y >nul & exit /b 0)`.

  quote_arg on this script: needs_quote=true (spaces). Then it backslash-escapes embedded quotes → `type \"C:\...\" && ...` wrapped in outer quotes. cmd.exe does NOT process `\"` as an escaped quote! cmd's tokenizer treats `"` specially but backslash has no escape meaning in cmd. After CRT argv parsing by cmd.exe's own CRT... does cmd.exe use CRT argv parsing? cmd.exe is a C program; its main() receives argv parsed by the MSVC CRT. So CommandLineToArgvW/CRT parsing: `"type \"C:\...\" && (...)"` → single arg `type "C:\..." && (...)`. Then cmd's /c handling: with /s, cmd takes the remainder of the RAW command line after /c, strips the outer quotes... Actually /s handling: cmd strips the first and last quote characters of the remainder and executes the rest VERBATIM from the raw command line (not the CRT-parsed argv!). The raw remainder is `"type \"C:\...\" && (...)"` → strip outer quotes → `type \"C:\...\" && (...)`. The `\"` stays literally! So cmd executes `type \"C:\...\"` → path starts with `\"C:` → INVALID_NAME! 

  That's a very plausible mechanism: quote_arg uses CRT escaping (`\"`) but cmd.exe with /s reads the raw command line, not the CRT-parsed argv, so the backslash-escaped quotes leak into the executed command. This would make every `type_and_hold` (and echo_temp_and_hold uses no quotes — no embedded quotes there) fail with exactly "The filename, directory name, or volume label syntax is incorrect."

  Wait but echo_temp_and_hold has no embedded quotes. type_and_hold has `type "{path}"`. So the fix: remove inner quotes (path has no spaces under C:\Users\Public — but PID-based names have no spaces), or rely on /s outer-strip and don't quote inside. Actually with /s the proper form is: cmd /s /c ""type C:\path" && (...)""? The classic rule: cmd /s /c ""inner"" keeps inner quotes. Hmm, but with quote_arg CRT-escaping, can't produce `""` easily... Actually quote_arg only escapes `"` as `\"`. There's no way to emit `""`. So minimal test-side fix: drop the quotes around the path in type_and_hold since the seeded path is space-free: `type {path} && (...)`. But is that robust? The dir name `wcore-r61-{pid}-{tag}` has no spaces. Fine.

  But wait — was this failing before? The comment says they previously fixed the exit-code issue (choice index) — at 20-32 it FAILED with exit code 1, then they added `exit /b 0`. Now failing differently. Maybe previously `type` also failed (choice index 1 via && short-circuit... no wait, previously without exit /b 0, even success gave exit 1). Now with exit /b 0, the type failure surfaces as nonzero exit.

  Hmm, but actually let me double check the /s semantics. cmd /s /c "string": /s modifies treatment of string after /c — strips first and last quote, executes remainder. The remainder is the RAW command line text. Yes. So embedded `\"` remains as literal `\"`. cmd then parses `type \"C:\Users\...\" && ...`: cmd's parser sees `\"` — backslash, then quote starts quoted region... the filename token becomes `\"C:\Users\Public\...\granted.txt\"`? Actually cmd strips the quotes during tokenization: token = `\C:\Users\...\granted.txt\`? Something like that — a path with a leading backslash-quote... The error INVALID_NAME fits.

  Hmm, but wait: does CreateProcessAsUserW pass cmdline_w as-is? Yes, lpCommandLine. cmd.exe parses the raw command line for /c handling. So yes.

  Actually, let me verify what cmd does more carefully. There's a well-known cmd quirk documented by Microsoft: cmd /c behavior — if /s not specified and the string meets certain conditions (exactly two quote chars etc.), quotes are stripped. With /s, always strips outer quotes. The raw text is used. So `type \"...\"` — the executed text contains backslashes before quotes. cmd's tokenizer: backslash is a normal filename char. So filename = `\"C:\...\granted.txt\"`→ after quote removal: `\C:\Users\Public\...\granted.txt\`... hmm trailing. Either way invalid → ERROR_INVALID_NAME 123. 

  But hold on — is this consistent with #2 failing on the REAL runner but presumably these tests were run before (comments reference runs at "20-32")? Maybe previous runs used different path without quotes... Can't verify history; doesn't matter. Actually — wait. I should double check git history for type_and_hold. Quick `git log -p` might be too heavy. Let me not; the mechanism analysis stands but I should mark confidence. Actually, I CAN test the quote_arg behavior on the actual string locally in Rust to show what cmdline is produced. That's verifiable here on macOS: compute quote_arg output. The cmd parsing part I can't test on macOS. Let me at least compute the produced command line.

  Let me also check: does anything else in the sandbox path mangle? For #3, same type_and_hold → same root cause as #2. So #2 and #3 same root cause: embedded-quote + CRT escaping in quote_arg vs cmd raw-cmdline parsing. And NOT the `\\?\` hypothesis (path never has the prefix in live_fs_acl). Actually wait — I should double-check whether `\\?\` could appear via `dir` canonicalization in the manifest fs_read_allow → that's only for ACL granting, not the type command. And granted ACL might be applied to `\\?\`-spelled path via acl_lease/storage.rs:789 — need to check what that does; if icacls is invoked with `\\?\` path it may fail → grant never applied → but then error would be Access denied, not invalid name. But ALSO the grant application might itself fail. Let me read acl_lease/storage.rs around 789 and the grant path.

  For #1: PowerShell -Command with trailing arg. The swarm_root has `\\?\` prefix. Where does swarm_root get canonicalized? Let me find. Also verify PowerShell behavior: `powershell -NoProfile -NonInteractive -Command <script> <arg>` — PowerShell docs: "With -Command, all remaining command-line arguments are considered part of the command." Actually the precise rule: with -Command, the args after the command string are passed in $args ONLY when... From MS docs: "-Command: ... the results of the script are... Any arguments after the command are available via $args"? Let me recall real behavior: `powershell -Command 'write-host $args[0]' hello` prints hello. Yes, trailing args land in $args. BUT there's the known parsing quirk: PowerShell re-parses the entire remaining command line; an argument like `\\?\C:\...` gets tokenized as PowerShell tokens and breaks with "Unexpected token". This is documented behavior — with -Command, PowerShell concatenates all remaining args and reparses them as script. A value with backslashes usually survives, but `\\?\` (starts with `\\` — interpreted as... hmm) produces ParserError. The observed error confirms: "Unexpected token '\\?\C:\...\checkouts' in expression or statement. ParserError/UnexpectedToken". So yes — the path reached PowerShell re-parsing and broke.

  Where does `\\?\` in swarm_root come from? Need to find where swarm_root is set in worktree_manager or its constructor. grep canonicalize in wcore-swarm.

  Fix for #1: pass the path via environment variable or `-File` with args, or strip the `\\?\` prefix (dunce), or better: embed arg safely — e.g., use `--%` stop parsing? Simplest robust: put the path in an env var for the child (`env` field? capture_bounded_process may not support env). Alternative: use `-Command` with the path already inside the script as a single-quoted PowerShell literal — but quoting must escape single quotes. Or change argv to `["-NoProfile","-NonInteractive","-Command", SCRIPT, "--", root]`? The `--` doesn't stop reparse. The real fix: PowerShell -Command reparse breaks on special chars; standard workaround is `-File` or env var. Or strip the verbatim prefix from swarm_root before passing: since GetPathRoot/DriveInfo don't need `\\?\`, dunce::simplified or manual strip of `\\?\` prefix for drive-letter paths. Both: strip prefix AND/OR use env var. Minimal: strip `\\?\` prefix when path is VerbatimDisk (root cause is the canonicalized path). But also path with spaces would break too (reparse splits on spaces... actually $args handles spaces? `powershell -Command $args[0] "a b"` — arg with spaces quoted by Rust Command quoting → PowerShell reparse sees quotes → keeps as one token → fine). `\\?\` is the breaker.

  #4 job-object: hard_process_containment_windows.rs:157 breakaway_is_denied "timed out waiting for both detached breakaway candidates running before job close" and :329 active_process_cap_is_enforced. Independent of paths. Need to read that test and the job-object code. Note AppContainer path sets JOB_OBJECT_LIMIT_BREAKAWAY... which flags? From imports: BREAKAWAY_OK, SILENT_BREAKAWAY_OK both imported — need to see which are set. The test expects breakaway denied. If the sandbox sets neither, breakaway is denied by default... but "timed out waiting for detached breakaway candidates running" — the test spawns detached processes (CREATE_BREAKAWAY_FROM_JOB | DETACHED) expecting them to fail to start or be killed? "timed out waiting for both detached breakaway candidates running before job close" — the test waits for candidates to be running, then closes the job expecting KILL_ON_JOB_CLOSE to reap them. If breakaway is allowed (SILENT_BREAKAWAY_OK set?), children escape the job, but they'd still be "running"... timeout waiting for them running means they never started. Hmm, detached candidates may fail under Low-IL token? Or under NetworkService, conhost issues? CREATE_NO_WINDOW note says console-host init fails 0xC0000142 without console... detached process creation (DETACHED_PROCESS) under the sandbox may fail. Need to read the test file. Is it in wcore-sandbox? "hard_process_containment_windows.rs" — find it.

  #5 dispatch_smoke.rs Os code 2 NotFound — likely powershell.exe not found? No... Or git? Under NetworkService, maybe the temp/profile. Need to read dispatch_smoke.rs and see what it spawns. Could share root cause with #1 (swarm_root verbatim path → git or mkdir failures). Os{code:2} is a Rust io error NotFound — maybe spawning a program that doesn't exist, or creating a dir under a `\\?\` path... Actually `\\?\` paths work with Rust fs APIs. code 2 could be from `git` not on PATH for NetworkService? But #1's error shows git operations proceeded to capacity probe. Hmm, dispatch_smoke might be in wcore-swarm tests — let me read it.

  Also missed things (#d): the `\\?\` path may also break other things: the unix `df` variant is fine; but git operations with verbatim paths? git for Windows historically had issues with `\\?\` paths. Also worktree add with verbatim path. Also NATIVE_ACCEPTANCE_CASES=11 vs actual test count. Also `choice.exe` under NetworkService? Also `has_appcontainer_ace` checks "wcore-" — profile name. Also LOCALAPPDATA for NetworkService exists. Also icacls availability fine.

  Let me now: read acl_lease/storage.rs:760-820, find swarm_root canonicalization, find hard_process_containment_windows.rs, read dispatch_smoke.rs. Parallel.

• Now let me trace where `swarm_root` picks up a `\\?\` prefix, the ACL lease storage, and the two remaining failure sites.

• Key confirmations so far:

  - `worktree_manager.rs:9`: `let repo_root = std::fs::canonicalize(repo_root)?;` and line 13: `let swarm_root = std::fs::canonicalize(&swarm_root)?;` → on Windows, `swarm_root` IS `\\?\C:\...` verbatim. Confirmed source of the `\\?\` prefix for the capacity probe.

  - capacity probe passes that verbatim path as a trailing arg after `-Command SCRIPT` → PowerShell reparse → ParserError. Confirmed mechanism (the observed error matches).

  - dispatch_smoke tests: `Swarm::new(tmp.path())` → worktree_manager canonicalizes → every dispatch calls workspace_capacity → capacity probe fails → DispatchAdmission error → `swarm.dispatch(brief, 4).await.unwrap()` panics. That explains multiple failures in dispatch_smoke with the capacity-probe error. But the user also reported `Os{code:2, NotFound}`. Where from? Possibly from `shell_command_argv("powershell.exe", ...)` — spawning powershell.exe: if the runner's PATH for NetworkService lacks System32\WindowsPowerShell\v1.0? No — spawning "powershell.exe" via CreateProcess searches PATH; System32 is always in PATH even for NetworkService... Actually powershell.exe lives in System32\WindowsPowerShell\v1.0 which IS on default PATH. Hmm.

  Os{code:2} could come from somewhere else. Note tests use `std::env::current_exe()` — fine. `git` spawn — git must be on PATH; it evidently is (other git ops run... actually do they? init_repo runs git — if git were missing, everything fails with code 2 at init. The capacity-probe failure happens at dispatch, meaning init_repo succeeded → git exists).

  Hmm — `Os{code:2, NotFound}`: maybe from reading `.swarm-worktrees` after dispatch failed (transaction_entries unwrap on read_dir → dir exists). Or from `swarm.cleanup()`. Or spawning `powershell.exe` works but... Let me not speculate too much; flag it as needing the actual log, but likely candidates: (1) cleanup failing on verbatim paths, (2) something spawning `bash` (the required_live_windows test uses `bash -c` — bash may not exist on the runner → code 2, and that test is designed to FAIL when backend can't bind, but spawn NotFound would give a different failure).

  Actually wait — `required_live_windows_public_dispatch_bash_confines_parent_and_descendants` dispatches `["bash", "-c", script]`. On a Windows runner, bash may exist (git-bash). The AppContainer sandbox rejects bash (BareShell::Unsupported). But this test enters through Swarm::dispatch which would fail earlier at the capacity probe. So its failure = same root cause as #1.

  Os{code:2} — could be from `Swarm::new` → `ensure_real_directory` etc.? Or from tempfile creation under NetworkService TEMP? If TEMP is fine... The user didn't give precise context. I should say: not enough info; list candidates.

  Actually — one strong candidate: `capture_bounded_process(shell_command_argv("powershell.exe", ...))`. If Windows PowerShell is not installed on Server Core-ish runners... but it's Windows 11, powershell.exe is inbox. Hmm.

  Another: `wcore-swarm` heartbeat/worker spawn uses `std::env::current_exe()` — the test binary path — fine.

  Or `std::fs::read_dir(repo.join(".swarm-worktrees")).unwrap()` in transaction_entries — if dispatch failed before creating... but worktree_manager constructor creates swarm_root via ensure_real_directory, so it exists.

  I'll be candid: I can't pin Os{code:2} without the log; likely candidates X, Y.

  Now #4: read hard_process_containment_windows.rs around 157 and 329, plus the job-object setup in the sandbox (which job limits are set). Note in the AppContainer backend they set JOB_OBJECT_LIMIT_ACTIVE_PROCESS etc. Let me read the test file and the job creation code.

• The `\\?\` source for #1 is confirmed (`worktree_manager.rs:9,13` canonicalize). Now the job-object test and the job-limit setup.

• Failure #4: breakaway_is_denied "timed out waiting for both detached breakaway candidates running before job close" — the wait_until at line 382 (tagged_cmd_count >= 2 within 20s) timed out. So the detached grandchildren never became visible to the host-side CIM query — or the parent script failed entirely. Possible causes:

  1. The parent cmd script failed → no grandchildren. The script contains embedded quotes: `start "" /b %ComSpec% /c "rem tag & hold" & ...`. Again quote_arg CRT-escaping: the script arg contains `"` chars → quote_arg produces `\"` → cmd /s strips outer quotes, executes raw text containing `\"` → the `start` command sees `\""` etc. → breakage. So the same quote-mangling root cause as #2/#3! The whole `cmd_script` helper path is broken whenever the script contains embedded quotes.

  Wait — but #2's failing test `granted_path_is_readable_then_revoked` uses type_and_hold which has `type "path"` — embedded quotes. And `one_execution_grant_never_leaks_to_another_identity` also uses type_and_hold. And ungranted_path_is_denied uses type_file (no script) which PASSED presumably (only 2 failing in live_fs_acl mentioned: :217 and :387 — those are the two type_and_hold-based granted-read tests). type_file argv form: cmd.exe /c type C:\path — quote_arg leaves everything unquoted (no spaces) → works. cmd_script form with embedded `"` → broken. That's consistent!

  And in hard_process_containment: contained_detached_child_exit uses `start "" /b choice...` — contains `""` embedded quotes → quote_arg escapes → broken? That test presumably passed? User only listed :157 breakaway (line 157 is the panic in wait_until, message "both detached breakaway candidates running before job close" → called at line 382-386) and :329 active_process_cap (line 329 assertion... wait :329 is `peak > 0` assertion? Line 329-332: `assert!(peak > 0, "fan-out never admitted any descendant")`. User said ":329 active_process_cap_is_enforced fails" — matches peak == 0, i.e. no descendants observed.

  Hmm, but active_process_cap script: `for /L %i in (1,1,{attempts}) do @start "" /b choice... & hold & exit /b 0` — contains `""` → mangled → for-loop/start fails → no descendants → peak == 0. Same root cause!

  contained_detached_child_exit also has `start "" /b` — did it pass? The user only reported breakaway_is_denied and active_process_cap. If quote-mangling were the cause, contained_detached_child_exit would fail too ("contained execution must return" or elapsed bound). Hmm. Unless... wait, maybe the mangling isn't happening the way I think. Let me carefully recompute.

  quote_arg applied to the script `start "" /b %SystemRoot%\System32\choice.exe /T 45 /D Y >nul & exit 0`:
  - needs_quote: contains spaces → yes.
  - For each `"`: emit backslashes*2+1 then `"`. backslashes before a `"` here is 0 → emits `\"`.
  - Result: `"start \"\" /b %SystemRoot%\System32\choice.exe /T 45 /D Y >nul & exit 0"`.

  Full cmdline: `cmd.exe /d /s /c "start \"\" /b ... & exit 0"`.

  cmd.exe startup: cmd's C runtime parses argv → argv[4] = `start "" /b ... & exit 0` (CRT unescapes `\"`). BUT cmd does NOT use argv for /c — cmd scans the RAW command line for /c and takes the rest. With /s: cmd strips the leading and trailing quote chars of the remainder and executes the rest AS-IS from raw text: `start \"\" /b ... & exit 0`.

  So cmd executes: `start \"\" /b ...`. `start` with title argument: first quoted arg is title. `\"\"` — cmd tokenizer: starts with `\`, then `"` opens a quoted region → title = `\"`? Actually start's title parsing: the first quoted string is the title. The raw text `\""` — hmm complex, but likely the start line breaks or title becomes weird, possibly `start` opens a new window or errors. 

  Actually wait — is that really what cmd does? Known behavior of cmd /c parsing: From Microsoft docs: "If /C or /K is specified, the remainder of the command line is processed as an immediate command... If /S is specified, the first and last quote are stripped and everything else is executed." Without /S, cmd strips quotes only under specific conditions (exactly two quotes, no special chars between, etc.). With /S, always strips first+last quote.

  So yes: executed text = raw remainder minus first and last `"` = `start \"\" /b %SystemRoot%\System32\choice.exe /T 45 /D Y >nul & exit 0`.

  Hmm wait — the last char of the cmdline is `"` (quote_arg wrapped). First char `"`. Strip both → `start \"\" /b ... exit 0`. Then cmd parses `start \"\" /b ...`: `start` sees arguments: `\"\"` — the first quoted-ish token. cmd's parser on `\"\"`: backslash literal, `"` toggles quote state... `\""` → quoted empty with backslash before? This might actually work by accident or might fail.

  This is getting speculative. The empirical evidence from the user's run: tests with type_and_hold failed with INVALID_NAME (consistent with `type \"path\"`), tests with embedded quotes in containment failed. Tests without embedded quotes (type_file, ungranted) presumably passed. contained_detached_child_exit — user said "5 of 6 targets failed"; within windows-job-object they mentioned breakaway + active_process_cap. Maybe contained_detached_child_exit and job_close_reaps passed? job_close_reaps also has embedded quotes (`%ComSpec% /c "rem tag & hold"`). Hmm, if quote-mangling broke all of them, more would fail.

  Wait — maybe I have the /s semantics wrong. Let me reconsider: maybe cmd DOES use CRT-parsed argv. Actually empirical knowledge: `cmd /s /c "echo "hello""` → prints "hello"? Hmm.

  Let me think again about what actually happens. There's a well-known fact: Rust's std::process::Command on Windows has special handling for cmd.exe: it appends... no wait, that's for `.bat` files. When Rust's Command builds a command line for cmd.exe with args ["/c", "script with \"quotes\""], Rust quotes with `\"` and people have hit this exact bug: cmd mangling. The common workaround: pass the script pre-quoted as a single arg and Rust... Actually the KNOWN issue (rust-lang/rust#29494, and the famous "cmd.exe /c argument quoting" problem): Rust's standard quoting breaks `cmd /c` when the command contains quotes. The recommended fix is to use `raw_arg` (`.raw_arg()` via CommandExt) or pre-quote the entire string. This is a real, well-documented footgun. And this codebase reimplements Rust's quoting (quote_arg, Colascione algorithm) — same CRT escaping — so it inherits the same cmd mismatch.

  But then why would contained_detached_child_exit and job_close_reaps pass while breakaway fails? Let me look again at the scripts:

  - contained_detached_child_exit: `start "" /b %SystemRoot%\System32\choice.exe /T 45 /D Y >nul & exit 0` → mangled: `start \"\" /b ...`. 
  - job_close_reaps: `start "" /b %ComSpec% /c "rem tag & hold" & hold8 & exit /b 0`.
  - breakaway: two `start "" /b %ComSpec% /c "rem tag & hold"`.
  - active_process_cap: `for /L %i in (1,1,544) do @start "" /b hold90 & hold25 & exit /b 0`.

  Hmm wait, maybe the mangling is NOT happening — maybe cmd actually handles it fine. Then what else could explain breakaway timing out on "candidates running"? 

  Alternative explanation for #4: `tagged_cmd_count` uses `powershell` — works. The detached grandchildren are `cmd /c "rem tag & choice"` — started via `start /b`. Under the AppContainer Job with active process limit 512, fine. But under the sandbox's restricted token, `start /b %ComSpec% /c ...` — the new cmd also needs to load — worked elsewhere presumably.

  OR: the host-side query `tagged_cmd_count` runs `powershell` UNSANDBOXED from the test process. Get-CimInstance Win32_Process — under NetworkService, querying Win32_Process CommandLine for processes owned by... the sandbox children run as the same user (NetworkService) with a restricted token. CIM/WMI access for NetworkService — querying Win32_Process requires WMI access; CommandLine property requires privilege but same-user processes are visible. NetworkService should be able to see its own processes' command lines. Hmm, but Get-CimInstance on Win32_Process can be slow (>20s under load?) — 20s deadline.

  Alternatively — the grandchildren cmd processes under AppContainer: `start /b` within cmd — the child cmd is spawned by the sandboxed cmd with the restricted token. Fine.

  Hmm, but wait — what if the problem is actually the same quote mangling but selective? For breakaway/active_process_cap the scripts contain `&&`-free sequences... All four containment scripts contain `""`. If mangling killed the whole script, contained_detached_child_exit would fail its exit-code assertion (exit 0/7 expected; cmd error gives exit 1). The user listed only 2 failures for windows-job-object out of... NATIVE_CONTAINMENT_CASES=5. So 3 passed including contained_detached_child_exit and job_close_reaps_detached_descendant. job_close_reaps has the same `%ComSpec% /c "rem tag & hold"` shape as breakaway — and it PASSED (its wait_until(tagged>=1) succeeded!). So tagged grandchildren DO start and ARE visible to the CIM query. breakaway waits for >= 2 within 20s — timing? The two starts are sequential in one script — same as job_close's one. Why would 2 not show up in 20s while 1 shows up?

  Hmm wait, maybe job_close didn't pass — user said "windows-job-object — hard_process_containment_windows.rs:157 breakaway_is_denied ... ; :329 active_process_cap_is_enforced fails". Only 2 failures named. But "5 of 6 targets failed" refers to test targets (cargo test binaries), not individual tests. windows-job-object is one target; it failed (at least one test failing marks the target failed). Other tests within could also have failed; the user only highlighted two signatures.

  OK so I can't fully determine #4's cause from the user's summary alone. Let me think about what's most plausible:

  For active_process_cap: peak == 0 → `image_count("choice.exe")` never saw any choice.exe above baseline during 30s while parent held 25s. The for-loop `for /L %i in (1,1,544) do @start "" /b choice...` — spawning 544 choice.exe processes. If the script ran correctly, even capped at 512, hundreds of choice.exe would appear. peak==0 → fan-out produced NOTHING observable. Either script failed (quote mangling breaking `start ""`), or tasklist query failed, or... If quote mangling broke `start ""`, contained_detached_child_exit would fail too — and maybe it did! The user said the target failed; they listed two signatures but there may be more failures in that target. Same for breakaway.

  Actually for `type_and_hold` — `type \"C:\...\" && (...)` → INVALID_NAME, && short-circuits, cmd exits with errorlevel 1? type's error → exit code 1. #3's observed: exit code was nonzero (assertion failed expecting 0), message says "not choice's selection index" — consistent with exit 1.

  But hold on — #2 stderr shows the INVALID_NAME message, meaning `type \"path\"` executed and failed with that error. That directly confirms the `\"` leak: cmd executed type with a mangled path `\"C:\...\"` → after cmd quote handling the filename token is `\C:\...` or contains `\?`... whatever, invalid. STRONG confirmation that embedded quotes get backslash-mangled. 

  Actually wait, let me double check one more possibility for #2: could `type` print INVALID_NAME for a properly-passed path under AppContainer when access is denied weirdly? No — access denied prints "Access is denied." INVALID_NAME strongly suggests malformed path text. And quote_arg's `\"` is the only mangling in the pipeline. The alternative `\\?\` hypothesis: path under %PUBLIC% never has the prefix, and nothing in the backend rewrites the type path (acl_lease canonicalizes only the GRANT paths for DACL ops, not argv). So hypothesis #2 as stated (cmd/type rejects \\?\) is REFUTED as root cause — real cause is quote_arg CRT-escaping of embedded quotes in cmd /s /c scripts.

  Now, is there ALSO a grant problem? If the ACE were missing, after fixing quotes type would print Access denied. Can't verify from here. But note the wait_until(has_appcontainer_ace) at line 206 PASSED (the failure was at line 217 after that wait) → the ACE was present during the run → grant machinery works. Good — so for #2, grant worked, read command mangled.

  For #3: same type_and_hold → same root cause. Note in #3, B's type_file (no grant, no quotes) correctly failed (assert_ne passed), A's type_and_hold mangled → exit nonzero → assertion at 387 failed. Same root cause as #2. Confirmed.

  Now #4: which containment tests use scripts with embedded quotes → ALL of them via cmd_script... but wait: cmd_script itself is also used by scripts WITHOUT embedded quotes? echo_temp_and_hold (no quotes) is in live_fs_acl and presumably its test passed. The pattern: embedded `"` → broken; no embedded `"` → fine. contained_detached_child_exit has `start "" /b` → broken → would fail. So likely 4 of 5 containment tests failed, matching "target failed". The one possibly passing: the gate marker (native_containment_gate_marker, runs with --ignored... it's #[ignore] but the suite runs --ignored presumably; it just checks availability → passes).

  So unify #2,#3,#4 as ONE root cause: quote_arg (command.rs:76-111) CRT-escapes embedded quotes when building the cmdline (process.rs:613-618), but cmd.exe /s /c re-reads the RAW command line and strips only the outer quotes, so the script executes with literal `\"` sequences → `type \"path\"` fails ERROR_INVALID_NAME, `start \"\" /b` misbehaves/fails.

  Minimal fix for that: in the tests, avoid embedded quotes (paths are space-free under %PUBLIC%; use `%ComSpec% /c rem ... & hold` without inner quotes — but `rem tag & hold` needs grouping... `start "" /b %ComSpec% /c "rem tag & choice"` — the quotes group the rem+choice; without quotes, `start "" /b %ComSpec% /c rem tag & choice` would run choice in the PARENT context after & — wrong. Hmm). Alternative proper fix: in process.rs, when argv[0] resolves to cmd.exe and /c or /k present, apply cmd-compatible quoting: wrap the script arg in `""` without backslash-escaping inner quotes (cmd doubles? no...). The robust approach for cmd: pass `cmd.exe /d /s /c "<script verbatim>"` where the outer quotes are added and inner quotes left as-is (cmd with /s strips outer quotes only and executes the rest verbatim). Since quote_arg would escape inner quotes, the backend should special-case: if the arg is the script following /c or /k for cmd.exe, don't CRT-escape; just wrap in quotes (and if it already starts/ends... keep simple). Rust std solves this with raw_arg; here the backend owns cmdline construction, so: join args, but for the /c payload, append it raw wrapped in double quotes.

  Careful about weakening isolation: this is a quoting-layer fix, no isolation impact, as long as we only change quoting of the /c payload for cmd. The sandbox boundary (job, token, ACLs) untouched.

  Alternatively fix tests to avoid quotes — but production code (BashTool etc.) will hit the same via cmd scripts; fixing the backend quoting is the right layer. Actually wait — is that true? Production callers pass arbitrary argv; any caller doing cmd /c with embedded quotes hits this. Backend fix is right.

  Now #1: capacity probe. Confirmed: worktree_manager.rs:9 canonicalize(repo_root), :13 canonicalize(swarm_root) → verbatim `\\?\` prefix. :417-451 windows available_workspace_bytes passes root as trailing arg after -Command. PowerShell -Command reparses trailing args as script text; `\\?\C:\...` → ParserError "Unexpected token '\\?\C:\...'" — matches observed. Two-layer fix:
  - Minimal: strip the verbatim prefix for drive-letter paths before passing (dunce::simplified or manual), at worktree_manager.rs:420 (or canonicalize site). Actually better at the probe site: `let root = ...strip_prefix(r"\\?\")` when followed by drive letter.
  - More robust: pass the path via env var: but capture_bounded_process signature? It takes a Command — shell_command_argv returns a Command builder; can set .env? Let me check shell_command_argv return type — it's likely tokio Command; can call .env("WCORE_PROBE_ROOT", root) and script reads $env:WCORE_PROBE_ROOT. That avoids reparse entirely. Either is fine; env-var approach is immune to all path weirdness.

  Does PowerShell command quote its path argument? No — it's passed as separate argv entry (argv mode), but -Command semantics reparse it; the observed error proves it. Also note: even without `\\?\`, a path with `&`/`(` would break similarly; env var fixes the class.

  Also unix df variant (line 375-415) passes root as argv to df — argv mode, no reparse, fine.

  #5 dispatch_smoke: Swarm::new → WorktreeManager::new canonicalizes (worktree_manager.rs:9) → dispatch → workspace_capacity → probe fails → DispatchAdmission → `.unwrap()` panics at dispatch (e.g. line 26). Multiple failures = same root cause as #1. The `Os{code:2,NotFound}`: where? Possibly from `swarm.cleanup().await.unwrap()` or from `transaction_entries` read_dir? Or — wait, maybe from spawning `powershell.exe`?? If the probe spawn itself failed with NotFound, capture_error("workspace capacity probe", error) would wrap it as DispatchAdmission too. But the user observed the ParserError message — so powershell ran. Os{code:2} might be a different test in dispatch_smoke: e.g. `required_live_windows_public_dispatch_bash_confines...` uses `bash` — if dispatch gets past admission... no, it fails at admission first. Hmm, but `public_dispatch_owns_git_authority...` asserts then calls cleanup... 

  Another candidate for code 2: heartbeat_symlink test is cfg(unix) — not on Windows. `dispatch_rejects_*_replacement`: replace_repo_container uses std::fs::rename — on Windows renaming `box` while swarm holds handles on box/repo → the comment says it's portable... code 2? NotFound on rename? Eh.

  Actually — is git on the runner? init_repo ran fine for other tests presumably. What about `Swarm::new` → validate_repo_authority → git rev-parse? Fine.

  I can't pin Os{code:2} without the actual logs; I'll say so and list the likely candidates: (a) `git` missing from NetworkService PATH in some spawn context, (b) rename/rmdir on verbatim paths, (c) reading `.swarm-worktrees` before creation in transaction_entries (line 358-360 unwrap on read_dir) — wait, if dispatch fails at admission BEFORE... Swarm::new creates swarm_root though. Hmm, actually does it? worktree_manager.rs:12 ensure_real_directory(&swarm_root) — creates it. So read_dir works.

  (d) strong candidate: the `bash` worker test — Swarm::dispatch might spawn the worker via the sandbox... but admission fails first. Unless workspace_capacity is only called for... hmm, is the capacity probe invoked on every dispatch? The user observed DispatchAdmission for windows-f20-lifecycle (wcore-agent tests) and presumably for swarm tests. If capacity probe is in the dispatch path, all dispatch tests fail with the ParserError, and code:2 comes from a non-dispatch path: `run_git` spawn `git`... 

  I'll flag (d) as unverifiable without logs and note candidates.

  Now (d) missed items — things that will fail next run after fixing these:

  1. `windows-f20-lifecycle` — 8 of 9 tests fail in transactional_delegated_mutation_test.rs. All go through workspace_capacity → same probe. After fixing the probe, next failures may surface (git operations with verbatim swarm_root paths — git for Windows handles `\\?\`? `git -C \\?\C:\...` may fail or produce mixed spellings; worktree add with verbatim paths historically problematic). Flag as watch item.

  2. `capture_bounded_process` with powershell.exe — fine.

  3. The `\\\\?\\` path also flows into `DirectoryAuthority::open`, `validate_swarm_root`, git commands via git_command with current_dir(verbatim path) — Rust sets cwd via CreateProcess lpCurrentDirectory which DOES accept `\\?\` paths? Actually lpCurrentDirectory does NOT support `\\?\` prefix! CreateProcess fails with ERROR_DIRECTORY or invalid parameter when lpCurrentDirectory is a verbatim path? Hmm — actually Rust's std handles this: Rust's Command on Windows strips `\\?\` from current_dir? Yes! Rust std canonicalizes and... I recall Rust's Command::current_dir uses the path verbatim in CreateProcess; CreateProcess lpCurrentDirectory must be a normal path (no `\\?\`)... Actually NT supports it via RtlDosPathNameToNtPathName. I believe `\\?\` cwd works on modern Windows. Not sure. Flag as watch item, can't verify.

  4. `equivalent_windows_spellings_resolve_to_one_file_identity` (storage.rs:756) — an ignored live test that would also run in the suite; probably fine.

  5. `has_appcontainer_ace` checks `acl.contains("wcore-")` — profile name under NetworkService fine.

  6. live_fs_acl type_file tests that pass would keep passing.

  7. After quote fix: `contained_detached_child_exit`'s elapsed bound, `active_process_cap` spawning 544 choice.exe — heavy but capped at 512; runner resources. Also note test counts choice.exe system-wide — parallel test interference: other tests use choice too (live_fs_acl type_and_hold uses choice!). If the suite runs test binaries concurrently or serially? Serial per target probably; but within wcore-sandbox, live_fs_acl and hard_process_containment are separate targets — if CI runs them in parallel, image_count("choice.exe") baseline gets polluted → flaky. Flag.

  8. tagged_cmd_count uses `powershell` (not powershell.exe) — resolves via PATH; fine. But WMI/CIM query under NetworkService: Get-CimInstance Win32_Process CommandLine — NetworkService has limited WMI perms; CommandLine may return null for processes it can't access, but its own children should be readable. Flag as possible failure: if CIM returns nothing due to permissions, breakaway/job_close fail at the "candidates running" wait. This is an alternative #4 explanation I can't rule out! Hmm. Which is right for #4? Evidence discrimination: if CIM query failed, `tagged_cmd_count` returns 0 for ALL → contained_detached_child_exit would still PASS (it doesn't use tagged_cmd_count; it asserts exit code + elapsed). job_close_reaps would fail at its wait_until(tagged>=1). breakaway fails at wait_until(tagged>=2). active_process_cap uses tasklist (image_count) — peak>0 assert fails if fan-out broken OR tasklist broken.

  So two coherent stories for #4:
  - Story A (quote mangling): ALL cmd_script tests with embedded quotes fail: contained_detached_child_exit (exit code wrong — actually would it? `start \"\" /b choice & exit 0` — if start fails, `& exit 0` still runs → exit 0! and elapsed fast → TEST PASSES vacuously!). Interesting — contained_detached_child_exit would PASS even with mangled start, because `& exit {code}` executes regardless and the detached idler never started, so no pipe hold, fast return. So quote-mangling is CONSISTENT with contained_detached_child_exit passing! And job_close_reaps: wait tagged>=1 fails (mangled start → no grandchild) → FAIL. breakaway: FAIL. active_process_cap: `for /L ... do @start \"\" /b ...` — if start line errors per iteration... `& hold25 & exit /b 0` still runs → parent exits 0, peak==0 → FAIL at :329. 

  - Story B (CIM/WMI perms): contained_detached passes, job_close FAILS, breakaway FAILS, active_process_cap — tasklist should still work (tasklist /FI works for NetworkService, its own processes visible) → fan-out would show hundreds → test would PASS. 

  The user reported exactly breakaway + active_process_cap failing. Under story A, job_close should also fail (same wait mechanism). Unless job_close DID fail and user didn't list it. Under story B, active_process_cap should pass. So the observed set (breakaway + cap fail) fits story A better IF job_close also failed (just unlisted), or fits neither perfectly. Actually with story A: which of the 5 containment tests fail? gate marker passes; contained_detached passes vacuously; job_close fails; breakaway fails; cap fails → 3 failures. User listed 2. Plausible they just highlighted 2.

  With story B: gate passes, contained passes, job_close fails, breakaway fails, cap passes → 2 failures but the pair would be job_close+breakaway, not breakaway+cap. User said breakaway+cap. So story A is the better fit.

  Also story A is already PROVEN by #2's stderr (INVALID_NAME from type with mangled path). Same cmd_script machinery in both files. Good — unify #2,#3,#4.

  One more check on #4 story A: does `start \"\" /b` actually fail? `start` parses first quoted arg as title. `\"\"` → cmd tokenizer: `\` literal, `"` opens quote, `\` literal? no—after `\"\"`: chars: backslash, quote(open), backslash, quote(close) → title = `\\`? Then `/b choice...` runs. Hmm — might actually still start choice! But then the tag test: job_close's start line is `%ComSpec% /c \"rem tag & hold\"` → the /c payload becomes `\"rem` → cmd child gets garbage → `rem` line breaks, tag absent, and `& hold`... The inner cmd fails → no tagged cmd → wait fails. For cap test: `start \"\" /b %SystemRoot%\System32\choice.exe /T 90 /D Y >nul` — if title=`\` and the rest runs, choice WOULD start → peak > 0... but reported peak==0. Hmm. Depends on exact cmd tokenization of `\"\"`. Can't fully verify without Windows. I'll present story A as most likely (proven mechanism from #2's stderr) but flag that the exact behavior of `start \"\"` should be verified on the runner; also note `>nul` inside a `for /L ... do @start ...` — the `&` after `do @start ... >nul` — in a for-loop one-liner, `& hold25` is OUTSIDE the loop only if parenthesized... `for /L %i in (1,1,544) do @start "" /b choice & hold25 & exit /b 0` — actually `do @start ...` consumes until end-of-line or `&`? In cmd, `for ... do (command)` — without parens, `do command` where command is the rest up to... `do @start "" /b choice.exe /T 90 /D Y >nul & choice25 & exit /b 0` — the `&` separators: for's do-command is just the start command; then `& hold25 & exit /b 0` are separate commands executed once after the loop. Correct semantics. OK.

  Also for (d) missed: 
  - The `echo_temp_and_hold` helper (no embedded quotes) → would pass. 
  - Check NATIVE_ACCEPTANCE_CASES=11 vs actual test count in live_fs_acl (I see: ungranted, granted_readable, denied_secret, one_exit_does_not_remove, one_execution_grant_never_leaks, concurrent_allow_and_deny + more beyond line 429 — file has 668 lines; if the count mismatches, gate marker fails. Let me count tests quickly.
  - Windows-f20-lifecycle: also uses DispatchAdmission path — after probe fix, lifecycle tests exercise landing/git with verbatim paths — potential next failures in `git worktree` handling `\\?\` paths (worktree.rs). Also note the capacity probe SCRIPT uses [IO.DriveInfo] on `\\?\C:\` — GetPathRoot of verbatim path returns `\\?\C:\` → DriveInfo ctor may throw "The given path's format is not supported"? DriveInfo accepts "C" or "C:\"; `\\?\C:\` would throw. But the parse error happens first.
  - `shell_command_argv("powershell.exe", ...)` relies on PATH; fine on Win11.
  - The unix df path is fine.
  - dispatch.rs:605 canonicalize(&program_path) — worker program canonicalized → verbatim `\\?\C:\...exe` path passed to the sandbox resolve_program → is_unc_or_device_path(program) REJECTS `\\?\` → every worker spawn with a canonicalized program path fails with "UNC or device path rejected"! Let me check dispatch.rs:605 context. This could be a REAL next-run blocker (or even part of #5's failures!). resolve_program rejects anything starting with `\\`. If dispatch canonicalizes the worker program (e.g. current_exe of the test binary) → `\\?\C:\...` → AppContainerBackend rejects spawn → worker fails. That's very likely part of #5! Let me read dispatch.rs around 605.

  Let me also count live_fs_acl test count for NATIVE_ACCEPTANCE_CASES=11, and read dispatch.rs:590-620. Also check what the worker spawn path does with the argv (sandbox? direct?). Also check `Swarm::new`→manager new, and where dispatch invokes sandbox backend on Windows.

• Two more checks: `dispatch.rs:605` canonicalization and the test count vs `NATIVE_ACCEPTANCE_CASES`.

• live_fs_acl has 11 #[ignore] async tests + gate marker asserting NATIVE_ACCEPTANCE_CASES=11. Count async fns with #[ignore]: ungranted(161), granted(186), denied_secret(251), one_exit(304), one_execution(352), concurrent(400), twenty_concurrent(443), timeout(490), unrelated_acl(536), deny_ace(578), normal_sid(630) = 11. OK consistent.

  Note `deny_ace_still_blocks_granted_read` (578) and others likely use type_and_hold too → they'd also fail with quote mangling. The user only listed :217 and :387 for windows-appcontainer-acl and windows-retained-handle — those are different TARGETS (test binaries compiled with feature filters?). "windows-appcontainer-acl" and "windows-retained-handle" are separate CI targets running subsets via --exact test names probably. OK.

  Now check dispatch.rs:605 — canonicalize(&program_path) pushed into fs_read_allow. fs_read_allow goes to the AppContainer ACL lease — canonicalize produces `\\?\` — is that a problem for the ACL grant (icacls / SetNamedSecurityInfo)? acl_lease.rs:436 canonicalizes anyway, so the grant path machinery already deals in canonical paths — designed for it (same_windows_path). Not argv. And the program path passed to resolve_program is `program` itself (argv[0] from worker_command — e.g. `std::env::current_exe()` which is NOT canonicalized, or "cmd"/"bash"). current_exe on Windows returns... `GetModuleFileNameW` — does Rust return `\\?\`? Rust's current_exe returns the path possibly with `\\?\` prefix? I believe std::env::current_exe on Windows returns a path WITHOUT the `\\?\` prefix... Actually Rust docs: current_exe may return a verbatim path? I recall it returns the plain path. Not sure.

  But the dispatch failures happened at capacity probe (before any worker spawn), so dispatch_smoke failures = #1 root cause. After fixing the probe, the next layer: worker spawn through AppContainerBackend with worker_command like current_exe (absolute path to test exe under target\debug\deps) → resolve_program: absolute path, try_exists → OK → spawned. But fs_read_allow includes canonicalize(program_path) = `\\?\...exe` → ACL grant path — lease machinery canonicalizes anyway. Then worker runs the test fixture binary under the AppContainer — the fixture exe loads DLLs... it's a Rust test binary (statically linked CRT?) — Rust msvc binaries link vcruntime dynamically? They link kernel32 etc, should load. Whether a full test-harness exe runs under Low-IL AppContainer is another question — flag as next-run risk.

  Also worker spawn: how does dispatch actually spawn? Presumably via a SandboxBackend selected somewhere (docker vs appcontainer). container_owned_workspace false on Windows native → AppContainer. OK.

  Now, is `git` invoked with verbatim swarm_root paths anywhere via Command::current_dir? git_command at worktree_manager.rs:462 uses current_dir presumably. Rust's Command::current_dir on Windows with a `\\?\` path: CreateProcess lpCurrentDirectory — documentation says it must be a valid path; `\\?\` prefixes ARE accepted by CreateProcess for lpCurrentDirectory? The MS docs say lpCurrentDirectory "The full path to the current directory" — path length limited to MAX_PATH unless... Actually there's a known Rust issue: Command::current_dir with verbatim paths works because Rust uses the wide API and NT handles it. Hmm, I genuinely don't remember. Flag as unverified watch item.

  Git itself with `-C \\?\C:\...` — git for Windows historically errors: "fatal: cannot change to '\\?\C:\...'"? Actually modern git handles it via cygwin/msys path translation... git for Windows (msys2) may mangle `\\?\` paths. Worktree operations in worktree_manager use git commands with paths — potential next-run failures in windows-f20-lifecycle even after the probe fix. The tests were previously run on this harness? The comment in dispatch_smoke about rename panics with code 5 suggests earlier runs got FURTHER (past capacity probe) — meaning the capacity probe used to pass?? Hmm, or those comments came from non-native analysis. Can't verify.

  Wait — actually let me reconsider #1. Did the probe always fail on Windows? The test suite "just ran on a REAL self-hosted Windows runner" — maybe first real run. The probe with `\\?\` root — would it EVER work? PowerShell -Command with trailing `\\?\C:\...` arg → ParserError always. Unless... when swarm_root has no `\\?\` prefix. canonicalize ALWAYS returns verbatim on Windows. So on Windows this always fails. Yes — first real run surfaces it.

  But hold on — verify the PowerShell claim: `powershell.exe -NoProfile -NonInteractive -Command "<script>" "\\?\C:\foo"` — does $args actually receive it or does the parser choke? The observed error message literally shows ParserError at "char:124" with the token `\?\C:\...checkouts`. Script length: the SCRIPT const is 120 chars. char:124 ≈ right after the script → the trailing arg was re-parsed as script text. Confirms: PowerShell -Command concatenates remaining args and reparses. So ANY trailing arg with PowerShell-special syntax breaks; `$args` never gets cleanly populated this way. Actually, known behavior: with -Command, trailing args ARE appended to the command string and reparsed; simple tokens like `C:\foo` parse as... an expression statement → CommandNotFoundException at RUNTIME (script still runs $args? no...). Empirically `powershell -Command 'write $args[0]' hello` prints hello — because `hello` parses as a command invocation, $args[0]... hmm wait does it? Let me think: PS docs say: "With Command, all remaining arguments are considered part of the command to execute" — actually the value of -Command is everything after it. When you pass -Command "script" arg1, PowerShell treats it as if you typed `script arg1` in the console? No — the documented behavior: args after -Command string are available in $args ONLY when using -File. For -Command, `$args` contains args passed after the command... Let me recall the actual doc: "-Command: Executes the specified commands (and any parameters) as though they were typed at the PowerShell command prompt". So `script arg1` typed at prompt → arg1 becomes part of the command line, appended to the script text, reparsed. `hello` alone parses as a command token → runs after script → but $args? Actually in `powershell -Command "$args" hello` prints "hello" — this is a known trick... I believe $args does work because PowerShell binds trailing tokens as args. Hmm, there's nuance: `powershell -c "& { $args[0] }" hello` works. With plain script text, `hello` gets appended to the command text and reparsed. For a bare word it might still bind to $args? The observed behavior here: ParserError on `\\?\...` — reparse of path text. Confirmed enough: the mechanism is the -Command reparse; fix is to not pass the path as trailing arg.

  Fix options for #1:
  - Pass path via env var: script `$root=[IO.Path]::GetPathRoot($env:WCORE_SWARM_PROBE_ROOT); ...` and set env on the Command. Need capture_bounded_process to allow env — check shell::shell_command_argv returns tokio::process::Command or std; likely can call .env before passing. Minimal and robust.
  - Or strip `\\?\` prefix (dunce-style) for VerbatimDisk before passing. Also needed maybe for git paths elsewhere. Check if dunce is a dependency.

  Actually simpler PowerShell-only fix: `-Command SCRIPT -- root`? `--` doesn't help. Or use `-EncodedCommand`? Overkill.

  Let me check whether dunce is in Cargo deps. And check shell_command_argv signature for env support. Also quickly verify whether anything else (git commands) receive verbatim paths — worktree.rs git_command. Let me check git_command current_dir.

• Final verification round: how the probe command is spawned, whether `dunce` is available, and how git commands use the verbatim `swarm_root`.Cargo.lock:334: "dunce",
Cargo.lock:1329: "dunce",
Cargo.lock:2416:name = "dunce"


• I have enough to answer. Let me check where dunce is used (which crates) to note it's available. Cargo.lock lines 334 and 1329 reference dunce as a dep of something. Quick check: probably wcore-config or wcore-sandbox already depends on dunce. Let me grep the workspace Cargo.tomls quickly.
[[package]]
name = "aws-lc-sys"
version = "0.41.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1a2f9779ce85b93ab6170dd940ad0169b5766ff848247aff13bb788b832fe3f4"
dependencies = [
 "cc",
 "cmake",
 "dunce",
 "fs_extra",
]

[[package]]
name = "aws-runtime"
version = "1.7.3"
version = "0.7.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8380ce7721cc895fe8a184c49d615fe755b0c9a3d7986355cee847439fff907f"
dependencies = [
 "async-tungstenite",
 "base64 0.22.1",
 "cfg-if",
 "chromiumoxide_cdp",
 "chromiumoxide_types",
 "dunce",
 "fnv",
 "futures",
 "futures-timer",
 "pin-project-lite",
 "reqwest",
 "serde",
dunce is only a transitive dep (aws-lc-sys, chromiumoxide), not a direct dependency of wcore-swarm/wcore-sandbox. So the fix should either add `dunce` as a direct dep (it's already in the lock tree) or strip the prefix manually (a 5-line helper).

  I have enough to write the report. Be terse, file:line, confirm/refute, root causes, fixes, missed items.

  Summary of findings:

  (a) The `\\?\` hypothesis: HALF confirmed.
  - #1: CONFIRMED in mechanism. Capacity probe: crates/wcore-swarm/src/worktree_manager.rs:417-451. swarm_root is canonicalized at :9 and :13 → always `\\?\C:\...` on Windows. Passed as trailing argv after `-Command` (:422-431). PowerShell -Command reparses trailing text as script → ParserError on `\\?\` token. The path is NOT quoted (argv mode doesn't help because PowerShell itself reparses).
  - #2/#3: REFUTED as stated. The path passed to `type` never has a `\\?\` prefix — seed_file builds from %PUBLIC% (live_fs_acl.rs:49-56), nothing canonicalizes it. Real cause: quote_arg CRT-escaping (command.rs:76-111) applied when building the cmdline (process.rs:613-618). type_and_hold (live_fs_acl.rs:131-134) embeds `"` around the path; quote_arg turns them into `\"`; cmd.exe /s /c strips only the OUTER quotes of the raw command line and executes the rest verbatim → cmd runs `type \"C:\...\"` → ERROR_INVALID_NAME "The filename, directory name, or volume label syntax is incorrect." — exactly the observed stderr. Does cmd/type reject `\\?\`? Yes it does, but it's not what's happening here.
  - Note: cmd DOES reject `\\?\` paths in general — true — but irrelevant here.

  Evidence for #2 grant working: wait_until(has_appcontainer_ace) at :206-210 passed before the read assertion at :217 failed — ACE was present; the read command itself was malformed.

  (b) Per target:
  1. windows-f20-lifecycle (8/9 fail): all go through `manager.workspace_capacity` → `available_workspace_bytes` → the PowerShell probe. Same root cause #A (probe reparse). transactional_delegated_mutation_test.rs:757 calls workspace_capacity.
  2. windows-appcontainer-acl :217: root cause #B (quote_arg/cmd mismatch). The `type "{path}"` in type_and_hold :131.
  3. windows-retained-handle :387: same #B (same helper, A's read command mangled → nonzero exit; comment at :383-385 even anticipated this). B's denial check passed because type_file (:147-157, no embedded quotes) is unaffected.
  4. windows-job-object: most likely #B too. All containment scripts go through cmd_script with embedded `""` (`start "" /b ...`): breakaway (:367-369), job_close (:236-240), active_process_cap (:292-296). `contained_detached_child_exit` (:200) would pass vacuously because `& exit {code}` runs regardless of the mangled `start`. Caveat: cannot fully verify cmd's tokenization of `start \"\"` without Windows; alternative (WMI/CIM visibility under NetworkService) is contradicted by active_process_cap failing (it uses tasklist, not CIM) — tasklist works for NetworkService, so the fan-out itself must have produced nothing → script-level failure → #B.
     - Hmm wait, check: does breakaway failing at "candidates running" fit #B? The inner `%ComSpec% /c \"rem tag & choice\"` — the spawned cmd gets mangled payload → `rem` command broken → but cmd would still run something... `cmd /c \"rem` → cmd receives command `\"rem` → invalid → exits immediately → no tagged cmd → tagged_cmd_count stays 0 → timeout. Fits.
     - Also there's a second possible contributor for active_process_cap: `for /L %i in (1,1,{attempts}) do @start \"\" /b ...` — if `start` line errors, loop completes instantly, parent holds 25s, peak=0 → assertion at :329 (`peak > 0`) fails. Fits.
  5. windows-public-dispatch: Swarm::new → WorktreeManager::new (worktree_manager.rs:8-13) canonicalizes → dispatch → workspace_capacity probe → DispatchAdmission → unwrap panics. Same root cause #A. dispatch_smoke.rs:26, :91, :181 etc. 
     - Os{code:2, NotFound}: cannot pin from source alone. Candidates: (i) spawning `powershell.exe` — no, probe produced ParserError output so powershell ran; (ii) `git` spawn — git clearly ran (init_repo precedes); (iii) `bash` worker in required_live_windows_public_dispatch_bash_confines (:187) — bash may not exist on the runner → but admission fails first, so this test fails with DispatchAdmission regardless; (iv) read_dir on `.swarm-worktrees` in transaction_entries (:358-360) — swarm_root is created in the ctor, so unlikely; (v) std::fs::rename in replace_repo_container (:332-335). Need the actual log to say. Most likely a secondary unwrap after the admission panic in cleanup, or the bash fixture. State explicitly: unverified.
     - ALSO found: dispatch.rs:601-607 — `std::fs::canonicalize(&program_path)` pushed into fs_read_allow → `\\?\`-prefixed grant path. acl_lease.rs:436 canonicalizes grant paths itself, so likely benign — but verify the lease machinery accepts an already-verbatim path (same_windows_path at acl_lease.rs:481 suggests it was designed for it).

  (c) Minimal fixes:
  #A (probe): worktree_manager.rs:417-451. Two options:
    - Strip the verbatim prefix before passing: for VerbatimDisk paths, `root.strip_prefix(r"\\?\")` (manual 4-line helper, or add `dunce` dep — currently only transitive in Cargo.lock, not a direct dep of wcore-swarm). One line at :420. This is the minimal fix.
    - More robust class fix: pass the path via env var instead of a trailing -Command arg: `cmd.env("WCORE_SWARM_PROBE_ROOT", &root)` and script reads `$env:WCORE_SWARM_PROBE_ROOT`. Immune to reparse for ALL weird path chars (spaces, `&`, `(`, quotes). Recommend this; it's ~3 lines changed (script const + argv list + .env call). shell_command_argv returns a tokio Command (shell.rs:265-270) so .env works before handing to capture_bounded_process.
    No isolation impact: read-only probe, path is self-owned.
  #B (quoting): the fix belongs in the backend, not the tests, because any production caller issuing `cmd /c` with embedded quotes hits the same mangling. In process.rs:613-618: when the resolved program is cmd.exe and the arg follows `/c` or `/k`, do NOT CRT-escape it; wrap the script in literal double quotes (cmd /s strips exactly the outer pair and executes the remainder verbatim). Inner quotes must pass through untouched. Alternative minimal-but-incomplete fix: remove inner quotes in the test helpers (live_fs_acl.rs:131-134 drop the `"` around the path — paths under %PUBLIC% have no spaces) — but this leaves the production landmine. Recommend the backend fix + keep tests unchanged. Isolation impact: none — quoting layer only; do NOT relax is_unc_or_device_path or the job/token setup. Caution: don't "fix" it by switching to shell-string interpolation; keep argv discipline.
    - Verify on the runner: `cmd /d /s /c "type "C:\path with space\file" && exit /b 0"` behavior.
  #4 same fix as #B.
  #5 same fix as #A.
  No fix weakens the sandbox boundary. Explicitly warn: do NOT fix #B by adding CREATE_NO_WINDOW or relaxing the Low-IL check; do NOT fix #A by catching the parse error and defaulting capacity to "unlimited" — that would silently defeat the admission budget (DispatchAdmission is a resource-governance control).

  (d) Missed / next-run failures:
  1. After #A is fixed, the verbatim swarm_root flows into every git invocation: git_command current_dir (worktree_manager.rs:1194,1201) and git args carry `\\?\C:\...` paths. Git for Windows (msys2) mangles `\\?\` paths in -C/--git-dir args; `git worktree add` with verbatim paths is a known failure mode. Cannot verify without the runner; expect residual failures in f20-lifecycle/dispatch even after the probe fix. Consider storing swarm_root de-verbatized (dunce::simplified at worktree_manager.rs:9,13) instead of patching each consumer — that fixes #A AND the latent git issue in one place. Also standalone_authority_fixture canonicalizes current_dir (dispatch_smoke.rs:385-386) → verbatim comparisons `child_git.starts_with(&checkout)` — both sides canonicalized so consistent; but WCORE_SWARM_PARENT_GIT env is NOT canonicalized (:387) → assert_ne!(child_git, parent_git) compares verbatim vs plain → passes trivially, fine. But the denied-read loop reads parent paths — fine.
  2. After #B is fixed, `active_process_cap_is_enforced` spawns 544 choice.exe (cap 512) — heavy on a self-hosted runner; also `image_count("choice.exe")` counts SYSTEM-WIDE (no per-user/session filter) — if live_fs_acl tests (which also use choice, live_fs_acl.rs:132) run concurrently with hard_process_containment on the same runner, baseline/delta gets polluted → flaky. Serialize the native targets on the runner or tag-filter.
  3. `tagged_cmd_count` uses bare `powershell` (hard_process_containment_windows.rs:110) and Get-CimInstance Win32_Process CommandLine — under NetworkService, CommandLine visibility for other-session processes may be null; its own children should be visible, but if the runner's WMI is locked down the "candidates running" waits will time out even after #B is fixed. Watch item; can't verify from source.
  4. dispatch.rs:604-607 canonicalize(program_path) → verbatim path in fs_read_allow. The lease machinery canonicalizes anyway (acl_lease.rs:436) so likely fine — but if any consumer string-compares manifest paths against non-canonical paths (e.g. deny-vs-allow overlap checks), verbatim/plain mismatch could cause a miss. Can't fully verify; flag.
  5. Worker spawn of the test-harness exe (fixture_argv, dispatch_smoke.rs:369-380): resolve_program accepts absolute existing exe (command.rs:222-237), but a full Rust test binary under the Low-IL restricted token must load its DLLs from System32 only — likely OK for a Rust exe, but the `--ignored --exact` fixture depends on the exe path being readable + its dir traversable; fs_read_allow gets the canonicalized exe path (file only, not dir) — AppContainer needs traverse/read on the file itself; granting the FILE should suffice for image load? Image load requires read on the file AND traverse on ancestors; %PUBLIC%-style ancestor chain doesn't apply to target\debug\deps under the repo... The checkout lives under Temp for dispatch_smoke (tempfile) — NetworkService temp. Grant covers only checkout+scratch+exe file. The exe's ancestor dirs (repo target\debug\deps) need traverse — AppContainer token without "bypass traverse checking"? Everyone has bypass-traverse by default... under restricted token? SeChangeNotifyPrivilege is typically held; restricted token with DISABLE_MAX_PRIVILEGE keeps privileges. Probably OK but unverified — flag as next-run risk for windows-public-dispatch after #A fix.
  6. The unix df probe branch (worktree_manager.rs:375-415) is unaffected.
  7. echo_temp_and_hold (live_fs_acl.rs:137-145) has no embedded quotes → unaffected by #B.
  8. NATIVE_ACCEPTANCE_CASES=11 matches the 11 #[ignore]d live tests — OK; adding any new live test without bumping the constant fails the gate marker.
  9. PowerShell probe even after quoting: [IO.DriveInfo]::new($root) where $root = GetPathRoot of a verbatim path returns `\\?\C:\` → DriveInfo ctor throws. If you fix #A by only QUOTING the arg (e.g. embedding it in the script as a quoted literal) but keep the `\\?\` prefix, DriveInfo fails at runtime. Stripping the prefix (or env var + DriveInfo handles plain root) is required. Note DriveInfo on `\\?\C:\` — I believe it throws ArgumentException. So prefix-stripping is mandatory either way; combined with env-var transport it's robust.

  Also should double-check claim "powershell -Command reparses trailing args" — I'm confident: docs state everything after -Command is executed as if typed at the prompt; the observed ParserError at char:124 (script is 120 chars) confirms.

  Also for (a): "Does the PowerShell command quote its path argument?" — No quoting in the script; path passed as a separate argv entry (worktree_manager.rs:429), which PowerShell's -Command reparse treats as script text. Quoting would NOT fix it robustly; env-var or prefix-strip will.

  One more check I should make: where does capture_bounded_process live and does the Command passed allow .env? shell_command_argv returns tokio Command (from wcore_config::shell). The caller at worktree_manager.rs:421-431 builds then passes to capture_bounded_process. Adding `.env(...)` before passing — trivial. Fine.

  Now write the report. Terse, file:line. The user asked for precision. Use their structure (a)(b)(c)(d).

• Verification complete. Here's the audit, in your structure.

  ## (a) The `\\?\` hypothesis — half confirmed, half refuted

  **#1 (capacity probe): CONFIRMED in mechanism.**
  - `swarm_root` is canonicalized at `crates/wcore-swarm/src/worktree_manager.rs:9` and again at `:13` — `std::fs::canonicalize` on Windows *always* returns the `\\?\C:\...` verbatim form.
  - The probe is `worktree_manager.rs:417-451`: script const at `:419`, path passed as a trailing argv entry at `:429` (`root.as_str()` after `"-Command", SCRIPT`). The path argument is **not quoted**, and quoting wouldn't save it: `powershell.exe -Command` reparses everything after the command string as PowerShell text (the observed ParserError is at char:124; the script const is ~120 chars — the error lands exactly where the trailing arg begins). `\\?\` is not valid PowerShell syntax → `UnexpectedToken`. `$args[0]` never binds.

  **#2/#3 (sandbox `type`): REFUTED as stated.** The path given to `type` never carries a `\\?\` prefix — `seed_file` builds from `%PUBLIC%` (`crates/wcore-sandbox/tests/live_fs_acl.rs:49-56`) and nothing canonicalizes it. The real mechanism:

  - `type_and_hold` embeds literal quotes: `type "{path}" && (...)` (`live_fs_acl.rs:131-134`).
  - The backend builds the child command line with `quote_arg` (`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:613-618`), which CRT-escapes embedded quotes to `\"` (`command.rs:76-111`).
  - But `cmd.exe /d /s /c` does **not** consume the CRT-parsed argv; with `/s` it strips the first and last quote of the *raw* command line and executes the remainder verbatim. The child therefore runs `type \"C:\Users\Public\...\granted.txt\"` → `ERROR_INVALID_NAME` — the exact stderr you observed. (Yes, `cmd`/`type` also rejects genuine `\\?\` paths with the same message, which is why the signature was misleading.)
  - Corroboration: the ACE-presence wait (`live_fs_acl.rs:206-210`) **passed** before the read failed — the grant machinery works; the read command was malformed. And the unquoted `type_file` helper (`:147-157`) used by the *denial* tests is unaffected, which matches your pass/fail split.

  ## (b) Root causes per target

  | Target | Root cause | Verdict |
  |---|---|---|
  | windows-f20-lifecycle (8/9 via `workspace_capacity`, e.g. `transactional_delegated_mutation_test.rs:757`) | **#A** probe reparse | #A |
  | windows-appcontainer-acl (`live_fs_acl.rs:217`) | **#B** quote_arg/cmd mismatch | #B |
  | windows-retained-handle (`live_fs_acl.rs:387`) | **#B** — same helper; A's `type` fails, `&&` short-circuits, exit≠0. Your "symptom, not its own bug" read is correct, but via #B, not `\\?\` | #B |
  | windows-job-object (`hard_process_containment_windows.rs:382-386` wait, `:329` assert) | **#B** most likely: every containment script has embedded `""` (`start "" /b ...`, `:200/:236/:292/:367`). `contained_detached_child_exit` passes *vacuously* (`& exit {code}` runs regardless). Alternative (NetworkService CIM visibility) is contradicted by `active_process_cap` failing — it uses `tasklist`, which works for NetworkService, so the fan-out itself produced nothing → script-level failure | #B |
  | windows-public-dispatch (`dispatch_smoke.rs:26,91,181`) | **#A** — `Swarm::new` → `WorktreeManager::new` (`worktree_manager.rs:8-13`) canonicalizes; every dispatch hits the probe before any worker spawns | #A |

  So: two root causes, not five. #1+#5 = #A; #2+#3+#4 = #B.

  `Os{code:2, NotFound}` in #5: **cannot pin from source alone.** Ruled out: `powershell.exe` (the probe ran and returned stderr) and `git` (`init_repo` precedes dispatch). Candidates: the `bash` worker at `dispatch_smoke.rs:187` (git-bash may be absent for NetworkService — but admission fails first, so that test fails with #A regardless), or a secondary `.unwrap()` after the admission panic. Need the actual log to say more.

  ## (c) Minimal fixes

  **#A — preferred: strip the verbatim prefix at the source, one place.** Apply `dunce::simplified()` (or a 4-line manual `strip_prefix(r"\\?\")` guarded to VerbatimDisk) at `worktree_manager.rs:9` and `:13`. `dunce` is currently only a transitive dep (aws-lc-sys, chromiumoxide in Cargo.lock), so add it to `wcore-swarm/Cargo.toml` or hand-roll. Fixing at the canonicalize site also defuses the latent git problem in (d1) — patching only the probe leaves `\\?\` flowing into every git invocation. Belt-and-braces for the probe itself: pass the path via env var (`.env("WCORE_SWARM_PROBE_ROOT", &root)` on the `tokio::process::Command` from `shell_command_argv`, `wcore-config/src/shell.rs:265`; script reads `$env:WCORE_SWARM_PROBE_ROOT`) instead of a trailing `-Command` arg — immune

