# Exit codes

`wayland-core` sets its process exit status from the outcome of the run, so a
script, CI step, `just` recipe or cron job can branch on `$?` without parsing
stdout.

This page is the contract. It is not a description of the code — the test
`crates/wcore-cli/tests/exit_code_contract.rs` PARSES the table below and
fails if any row disagrees with the constants in
`crates/wcore-cli/src/exit_code.rs`, so the two cannot drift apart.

## The codes

<!-- EXIT-CODE-TABLE:BEGIN -->

| Code | Constant | Meaning |
|------|----------|---------|
| 0 | `OK` | The run completed: the model ended its turn and the last tool batch, if there was one, had no unrecovered error. |
| 1 | `FAILURE` | Startup, configuration, provider or transport failure. The run never produced an answer. |
| 2 | — | CLI usage error: an unknown flag, a bad argument. Emitted by the argument parser before anything else runs. |
| 3 | `TOOL_FAILURE` | The run ended on an UNRECOVERED tool failure: the last tool results before the model's final answer contained an error and the model made no further tool call. |
| 4 | `LIMIT` | The engine stopped the run at a limit (`--max-turns`, the token envelope) instead of the model finishing. |
| 129 | `HUNG_UP` | Hung up (SIGHUP). |
| 130 | `INTERRUPTED` | Interrupted (SIGINT / Ctrl-C). |
| 143 | `TERMINATED` | Terminated (SIGTERM). |

<!-- EXIT-CODE-TABLE:END -->

`128 + N` for signal `N` is the shell convention, so `$?` reads the same as it
would for any other interrupted Unix program.

## What the codes deliberately do NOT say

There is no code for "the model's answer was wrong". Nothing in this process
can verify that, and inventing a code for it would be a worse lie than the
silence it replaced. Exit `0` means *the run completed*, not *the task was
achieved*.

A tool failure the model went on to RECOVER from is not code `3`. An agent
that probes for a file, finds it missing, and reads a different one has not
failed; treating that as failure would make almost every real run look broken.
Only the trailing state counts.

Code `4` outranks code `3`. When a run is cut short at a limit, "we ran out of
turns" is the more actionable fact, and the trailing tool state of a truncated
run is not a verdict on the task.

## Platform notes

| Signal | Linux | macOS | Windows |
|--------|-------|-------|---------|
| SIGINT / Ctrl-C | 130 | 130 | 130 |
| SIGTERM | 143 | 143 | not delivered by the OS |
| SIGHUP | 129 | 129 | not delivered by the OS |

Windows has no SIGTERM or SIGHUP; the console Ctrl-C event is the only
shutdown signal, and it maps to `130` like its Unix counterpart. Codes `0`–`4`
are identical on all three platforms.

## Using it

```bash
wayland-core -p openai "run the migration and report"
case $? in
  0)   echo "completed" ;;
  3)   echo "ended on a failed tool — check the transcript" ;;
  4)   echo "hit the turn cap — re-run with a higher --max-turns" ;;
  130) echo "you interrupted it" ;;
  *)   echo "did not start: $?" ;;
esac
```

Other exit paths follow the same rules. `--doctor` exits `1` when a check that
is REQUIRED on the current platform reports missing, and `0` otherwise —
warnings and skipped or manual rows never flip it. Subcommands that fail
during their own work (`plugin`, `swarm`, `workflow`, `mcp-serve`) exit `1`.
