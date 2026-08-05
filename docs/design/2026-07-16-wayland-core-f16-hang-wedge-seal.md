# F16 live hang and wedge seal

Status: implementation-complete on the staged candidate based on
`73eb0ee297db756cd49fa1f1677418f78e49f46a`. Linux exact-candidate proof is
green. The candidate is not pushed or merged, and current-candidate macOS and
Windows native execution remain release gates rather than claimed passes.

## Requirement matrix

| Failure class | Root fix | Current proof | Disposition |
|---|---|---|---|
| Bash output capture and runaway descendants | Missing optional bwrap mounts use `--ro-bind-try`/`--bind-try`; all sandbox execution owns a process tree and bounded stdout/stderr drain | `d434dd519775c99799a56af744449fc42aa96b57`; exact-HEAD `wcore-sandbox` and Bash-filtered `wcore-tools` suites | Implemented; Linux passed |
| Provider request/stream stalls | Provider connect/between-byte deadlines remain the live-consumer bound; dropping the engine receiver now terminates every spawned provider stream worker | `af87c293f90dbd58468577a15afd4136eb89112b`; 942-provider-suite proof recorded in the slice receipt | Implemented; Linux passed |
| Browser registration and Camoufox sidecar | Plugin spec reifies into a real `BrowserTool`; the real Camoufox REST contract is used; health/startup are bounded; owned sidecars are process-tree contained and reaped | `d2ee49da8417780b9d7dd21771a5a0768e121bf0`, `73eb0ee297db756cd49fa1f1677418f78e49f46a`; 116 browser tests and strict clippy | Implemented; Linux passed |
| Flux fork finish-length wedge | Missing upstream tool-call ids are synthesized once and survive assistant-call/result replay, preventing blind repeat turns | `108792ef56056c74251d697820de9da3717bf529`; 939-provider-suite proof recorded in the slice receipt | Implemented; Linux passed |
| Context-ceiling hard death | Oversized tool output is spilled; conversation-heavy history is truncated/dropped without orphaning tool pairs; irreducible history does not overwrite the last recoverable session | `33a9bdd9ee28ab8b8dcb9e06e2a7928f0b6c8a20`, `bb7e6912a467e6afd330a5a51bc83d44cb021c5d`, `4a652150206952888d9b7290045cdbd65d7c397a`; exact-HEAD compact-engine suite | Implemented; Linux passed |
| Windows/WSL process and path behavior | Blocking canonicalization is off the async reactor; the AppContainer real-spawn probe is hard-bounded and single-flight; process trees use Windows Job Objects; centralized argv/PATHEXT helpers own portable spawn | `af69bdc046bef94671426a20a8a1fb7327c91d30`, `2ead3d5d49103ed3e8db196b7d49e25cd0512707`; PR 207 had green x86_64/aarch64 Windows and macOS builds | Implemented historically; current candidate native rerun required |

## Exact-HEAD Linux evidence

The committed candidate was copied to the `wcore-f16` Hetzner proof slot for
each Cargo run. Cargo was not run on the Mac except formatting.

- `cargo test -p wcore-sandbox`: PASS. The suite covers output limits,
  cancellation, background-descendant reaping, missing optional bwrap sources,
  and AppContainer probe concurrency/fail-closed behavior.
- `cargo test -p wcore-tools bash -- --nocapture`: PASS. 59 Bash-related unit
  tests passed with two intentional ignores; the credential-exfiltration,
  cancellation, script, and tool-cancel integration regressions also passed.
- `cargo test -p wcore-agent --test engine_compact_test`: PASS, 15/15. This
  includes live-turn and resumed-session tool-output shedding, text truncation,
  drop-oldest degradation, and bounded emergency behavior.
- `cargo test -p wcore-agent --lib
  engine::retry_wedge_protection_tests::ceiling_abort_does_not_persist_unrecoverable_session
  -- --exact --nocapture`: PASS, proving an irreducible over-ceiling turn leaves
  the last recoverable journal/session state intact.
- `cargo test -p wcore-browser`: PASS, 116 tests.
- `cargo clippy -p wcore-browser --all-targets --all-features -- -D warnings`:
  PASS.
- `cargo build --release -p wcore-cli`: PASS. This exposed and then proved the
  protocol bridge's explicit no-view handling for `ProviderFailoverReceipt`.
- `cargo test -p wcore-cli --test release_binary_smoke
  release_binary_ready_event_advertises_plugin_capabilities -- --exact
  --nocapture`: PASS, 1/1, against the optimized binary.
- `cargo test -p wcore-cli --test registry_inventory_snapshot
  bootstrap_registers_browser_tool -- --exact --nocapture`: PASS, 1/1,
  proving browser registration through the shipped CLI bootstrap path.

## Honest boundaries

1. The current candidate has not been exercised on a native Windows or macOS
   runner. A MinGW cross-check was blocked before `wcore-browser` compilation
   because the proof host lacks `x86_64-w64-mingw32-gcc`; this is not a Windows
   pass.
2. Core never downloads executable browser code at tool-call time. A release
   must bundle or otherwise install a pinned Camoufox sidecar; absent that,
   browser calls now fail quickly with actionable diagnostics instead of
   hanging or silently colliding.
3. GitHub issues remain open for release coordination. This seal does not close
   them and does not claim that older released binaries contain this candidate.
