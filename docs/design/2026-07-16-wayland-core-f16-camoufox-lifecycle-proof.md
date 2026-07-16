# F16 Camoufox lifecycle proof

## Scope

This increment connects the production `wayland-browser` adapter to the
existing browser supervisor. It does not install Camoufox and it does not
claim that release packaging already carries the Node sidecar.

The runtime contract is:

1. Probe the configured `/health` endpoint under a 500 ms request deadline.
2. Reuse a healthy externally managed service without taking ownership.
3. Otherwise start only the installed `camofox-browser` command, or the
   executable explicitly named by `WAYLAND_CAMOUFOX_BIN`.
4. Never invoke npm, npx, a shell, or a network installer from a browser tool
   call.
5. Wait at most 15 seconds for health, report early process exit, and return an
   actionable installation error when the command is absent.
6. Own the launched process tree through the centralized
   `wcore-sandbox::backends::process_tree` primitive: a Unix process group or a
   kill-on-close Windows Job Object.
7. Reap the owned tree when startup fails or the supervisor drops. Never kill a
   recovered numeric PID on drop because it may have been reused.
8. Send `Authorization: Bearer ...` when `CAMOFOX_ACCESS_KEY` protects the
   sidecar. The header value is marked sensitive.

## Upstream contract checked

The command, health endpoint, access-key behavior, and Node requirement were
checked against `jo-inc/camofox-browser` version `1.11.2` at upstream commit
`ce3a3b085aacba73eb8de6c51733c19fb13bfae4`.

## Authoritative evidence

The staged tree was copied to the `wcore-f16` Hetzner slot before every Cargo
run.

- `cargo test -p wcore-browser`: PASS, 116 tests total (81 library, 3 op enum,
  27 policy, 1 provider trait, 4 timeout/cancel).
- `cargo clippy -p wcore-browser --all-targets --all-features -- -D warnings`:
  PASS.
- `cargo check --target x86_64-pc-windows-gnu -p wcore-browser`: BLOCKED before
  `wcore-browser` compilation because the worker lacks
  `x86_64-w64-mingw32-gcc`; `sqlite-vec` stopped in its build script. This is
  not a Windows pass.
- Local `cargo fmt --all` and `git diff --check`: PASS. No Cargo compile or test
  ran on the Mac.

The regression coverage proves healthy external reuse, missing-command
diagnostics, tool-level readiness before session creation, access-key auth,
real child tracking, explicit session cleanup, and supervisor-drop cleanup.

## Remaining release boundary

Core now has a truthful lifecycle once the sidecar is installed or bundled.
The npm/Desktop release lane still needs a pinned, provenance-checked delivery
decision for `@askjo/camofox-browser` and its approximately 300 MB browser
payload. Runtime auto-download is intentionally not used as a substitute for
that release work.
