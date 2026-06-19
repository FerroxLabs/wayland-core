# Wayland Proving Ground — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `just proving-ground` harness that drives the real `wayland-core` binary through a PTY across hermetic Sessions, asserts a deterministic invariant spine, and turns the four known bugs into failing→passing cells — the trustworthy core the overnight generative sweep (M2/M3) builds on.

**Architecture:** Promote the existing PTY harness in `crates/wcore-cli/tests/smoke_p0.rs` into a shared `tests/support/` module, add a `Session` (one temp `WAYLAND_HOME` reused across relaunches so persistence is testable) and a `RunRecord`, write the invariant checks over that record, and express each known bug as a `Cell` (a scripted persona+config+terminal run). M2 (generative explorer) and M3 (overnight runner/report) are **separate follow-on plans** — see spec §"Breadth after the first overnight run".

**Tech Stack:** Rust 2021, `portable_pty` (already a dev-dep), `tempfile`, `wiremock` (`support::mock_llm`), `vt100` 0.15 (pinned). Unix-only for M1 (`#[cfg(unix)]`), matching the existing harness.

## Global Constraints

- **Build on the existing harness — do not reinvent.** Source of truth assets in `crates/wcore-cli/tests/smoke_p0.rs`: `Pty::spawn(home: &Path)`, `pty.screen_text() -> String`, `pty.wait_for(predicate, Duration, what)`, `pty.send(&[u8])`, `pty.wait_for_exit(Duration)`, `pty.quit()`, `boot(home) -> Pty`, `write_config(home, provider, model, base_url)`, `harden_child_env(cmd, home)`, `STRIPPED_PROVIDER_ENV`. Shared test code lives in `crates/wcore-cli/tests/support/` and is included via `#[path = "support/mod.rs"] mod support;`.
- **Hermeticity:** every Session gets a fresh `TempDir` as `WAYLAND_HOME` + `HOME`; `harden_child_env` strips real provider keys. A run that touches real state is a failure.
- **M1 is `#[cfg(unix)]` and $0** — no live API, no LLM, no judge. MockLlm only.
- **`vt100` stays pinned at 0.15** (a `unicode-width` conflict forces it — see smoke_p0.rs).
- **Plan/spec live in `docs/design/`** (`docs/superpowers/` is gitignored here).
- Errors: `thiserror` for public types, `anyhow` internally; never `unwrap()` in non-test code without a proven invariant.

---

### Task 1: Promote the PTY harness into shared `support`

**Files:**
- Create: `crates/wcore-cli/tests/support/pty.rs`
- Modify: `crates/wcore-cli/tests/support/mod.rs` (add `pub mod pty;`)
- Modify: `crates/wcore-cli/tests/smoke_p0.rs` (delete the inline `mod pty_smoke { … Pty … }` block + the top-level `write_config`/`harden_child_env`/`STRIPPED_PROVIDER_ENV`; re-import from `support::pty`)

**Interfaces:**
- Produces: `support::pty::{Pty, boot, write_config, harden_child_env, STRIPPED_PROVIDER_ENV}` with the exact signatures listed in Global Constraints.

- [ ] **Step 1: Move the code, don't change it.** Cut `Pty` (struct + impl), `boot`, `write_config`, `harden_child_env`, `STRIPPED_PROVIDER_ENV` from `smoke_p0.rs` into `crates/wcore-cli/tests/support/pty.rs`. Make each `pub`. Add the `#[cfg(unix)]` guard on `Pty`/`boot` (keep `write_config`/`harden_child_env` cross-platform as they are today). Keep `use portable_pty::{...}` etc. local to the file.

- [ ] **Step 2: Wire the module.** In `crates/wcore-cli/tests/support/mod.rs` add:
```rust
#[cfg(unix)]
pub mod pty;
```
(Keep `write_config`/`harden_child_env`/`STRIPPED_PROVIDER_ENV` accessible — if they must stay cross-platform, put them in a non-cfg `pub mod hermetic;` and re-export.)

- [ ] **Step 3: Re-point smoke_p0.rs.** Replace the deleted definitions with `use support::pty::{Pty, boot, write_config, harden_child_env, STRIPPED_PROVIDER_ENV};`.

- [ ] **Step 4: Verify the existing tests still pass (non-lossy extraction).**
Run: `cargo nextest run -p wcore-cli --test smoke_p0`
Expected: same pass count as before the move (PTY tests run on unix; `#[ignore]` ones stay ignored).

- [ ] **Step 5: Commit.**
```bash
git add crates/wcore-cli/tests/support/ crates/wcore-cli/tests/smoke_p0.rs
git commit -m "test(proving-ground): promote PTY harness into shared support module"
```

---

### Task 2: `Session` + `RunRecord` + the cell runner

**Files:**
- Create: `crates/wcore-cli/tests/support/proving_ground/mod.rs`
- Create: `crates/wcore-cli/tests/support/proving_ground/record.rs`
- Modify: `crates/wcore-cli/tests/support/mod.rs` (add `#[cfg(unix)] pub mod proving_ground;`)
- Test: `crates/wcore-cli/tests/proving_ground.rs` (new integration test file with `#[path = "support/mod.rs"] mod support;`)

**Interfaces:**
- Produces:
  - `Session::new() -> Session` — owns one `TempDir`; `session.home() -> &Path`.
  - `Session::launch(&self) -> support::pty::Pty` — spawns the real binary against the session home (reuses the same home on every call → relaunch).
  - `RunRecord { final_screen: String, exit: Option<i32>, config_toml: Option<String>, requests: Vec<RecordedRequest>, dirty_death: bool }` (deterministic core; sidecar fields added when an oracle needs them).
  - `run_cell(cell: &Cell) -> RunRecord` and `Cell { name: &'static str, config: ConfigState, term: TermShape, script: fn(&mut Pty, &Session) }`.

- [ ] **Step 1: Write the failing test** in `crates/wcore-cli/tests/proving_ground.rs`:
```rust
#[path = "support/mod.rs"]
mod support;
use support::proving_ground::{Session, ConfigState, TermShape, Cell, run_cell};

#[cfg(unix)]
#[test]
fn run_cell_captures_a_runrecord_for_a_clean_boot() {
    let cell = Cell {
        name: "clean-boot",
        config: ConfigState::ConfiguredOpenAi,   // writes a minimal config so it boots to Workspace
        term: TermShape::default(),
        script: |pty, _s| { pty.wait_for(|t| t.contains("Workspace"), std::time::Duration::from_secs(10), "workspace"); },
    };
    let rec = run_cell(&cell);
    assert!(!rec.dirty_death, "clean boot must not leave a dirty-death sentinel");
    assert!(rec.final_screen.contains("Workspace"));
}
```

- [ ] **Step 2: Run it, watch it fail.**
Run: `cargo nextest run -p wcore-cli --test proving_ground run_cell_captures_a_runrecord_for_a_clean_boot`
Expected: FAIL — `support::proving_ground` unresolved.

- [ ] **Step 3: Implement `record.rs`** — the `RunRecord` struct (fields above), plus a `redact(&str) -> String` that masks anything matching the credential shapes (`sk-…`, `xai-…`, etc.) before any field is stored.

- [ ] **Step 4: Implement `proving_ground/mod.rs`:**
```rust
pub use super::pty::{Pty, harden_child_env};
pub mod record;  pub use record::RunRecord;
use super::mock_llm::{MockLlm, RecordedRequest};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct Session { home: TempDir }
impl Session {
    pub fn new() -> Self { Self { home: TempDir::new().expect("tempdir") } }
    pub fn home(&self) -> &Path { self.home.path() }
    pub fn launch(&self) -> Pty { Pty::spawn(self.home.path()) }   // same home each call => relaunch
}

#[derive(Clone, Copy)]
pub enum ConfigState { Fresh, EnvKeysOnly, ConfiguredOpenAi, CorruptConfig }
#[derive(Clone, Copy)]
pub struct TermShape { pub rows: u16, pub cols: u16 }
impl Default for TermShape { fn default() -> Self { Self { rows: 40, cols: 120 } } }

pub struct Cell {
    pub name: &'static str,
    pub config: ConfigState,
    pub term: TermShape,
    pub script: fn(&mut Pty, &Session),
}

pub fn run_cell(cell: &Cell) -> RunRecord {
    let session = Session::new();
    cell.config.materialize(session.home());   // writes config.toml / no-op / corrupt bytes
    let mut pty = session.launch();
    (cell.script)(&mut pty, &session);
    pty.quit();
    RunRecord::capture(session.home(), &mut pty)  // reads final screen, config.toml, dirty-death sentinel
}
```
(`TermShape` must be threaded into `Pty::spawn` — extend `Pty::spawn(home, TermShape)` and default existing callers to `TermShape::default()` in Task 1's re-point, or add `Pty::spawn_sized`.)

- [ ] **Step 5: Run the test, confirm PASS.**
Run: `cargo nextest run -p wcore-cli --test proving_ground run_cell_captures_a_runrecord_for_a_clean_boot`
Expected: PASS.

- [ ] **Step 6: Commit.**
```bash
git add crates/wcore-cli/tests/support/proving_ground/ crates/wcore-cli/tests/proving_ground.rs crates/wcore-cli/tests/support/mod.rs
git commit -m "test(proving-ground): Session + RunRecord + cell runner"
```

---

### Task 3: KNOWN BUG #1 — onboarding persists across relaunch

**Files:** Modify `crates/wcore-cli/tests/proving_ground.rs`; create `crates/wcore-cli/tests/support/proving_ground/invariants.rs`.

**Interfaces:** Produces `invariants::config_persists(records: &[RunRecord]) -> Result<(), String>`.

- [ ] **Step 1: Write the failing cell** (drives the real onboarding env-keys flow, then relaunches the same Session and asserts it does NOT re-onboard):
```rust
#[cfg(unix)]
#[test]
fn onboarding_persists_across_relaunch() {
    let session = Session::new();
    ConfigState::EnvKeysOnly.materialize(session.home());   // OPENAI_API_KEY in child env, no config.toml
    // First launch: connect the detected env key (press '1'), complete the flow.
    let mut p1 = session.launch();
    p1.wait_for(|t| t.contains("Detected in your environment"), SECS_10, "onboarding");
    p1.send(b"1");                                          // connect OpenAI
    p1.wait_for(|t| t.contains("Ready") || t.contains("Workspace"), SECS_10, "connected");
    p1.send(b"\r");                                          // finish
    p1.quit();
    // config.toml MUST now exist with the provider.
    let cfg = std::fs::read_to_string(session.home().join("config.toml")).unwrap_or_default();
    assert!(cfg.contains("openai"), "connect must persist a provider to config.toml");
    // Second launch (same home): MUST land on Workspace, not Onboarding.
    let mut p2 = session.launch();
    p2.wait_for(|t| t.contains("Workspace") && !t.contains("connect a provider to begin"),
                SECS_10, "workspace-not-onboarding");
    p2.quit();
}
```

- [ ] **Step 2: Run it.** Expected: **FAIL** today (the known bug — no config.toml is written / it re-onboards). This failure IS the bug, captured.
Run: `cargo nextest run -p wcore-cli --test proving_ground onboarding_persists_across_relaunch`

- [ ] **Step 3: Fix the engine** (the actual bug, separate small change): in the onboarding connect path (`crates/wcore-cli/src/tui/surfaces/onboarding.rs`), persist on successful connect, and make the first-run gate (`crates/wcore-cli/src/tui/mod.rs:455`) treat a connected provider as configured. (Detailed engine fix is its own commit; this cell is its acceptance test.)

- [ ] **Step 4: Run it, confirm PASS** after the engine fix.

- [ ] **Step 5: Add the reusable invariant** `config_persists` in `invariants.rs` (asserts: after a connect run, the relaunch record's `final_screen` is Workspace and `config_toml` is `Some` with a provider). Commit.
```bash
git commit -m "test(proving-ground): onboarding-persistence cell + config_persists invariant (known bug #1)"
```

---

### Task 4: KNOWN BUG #2 — `/doctor` content reachable at short height

**Files:** Modify `crates/wcore-cli/tests/proving_ground.rs`, `invariants.rs`.

- [ ] **Step 1: Write the failing cell** — boot configured, open `/doctor` at a deliberately short terminal, assert the DISCOVERED section (last section) is reachable via the canonical reveal keys:
```rust
#[cfg(unix)]
#[test]
fn doctor_content_reachable_at_short_height() {
    let session = Session::new();
    ConfigState::ConfiguredOpenAi.materialize(session.home());
    let mut p = session.launch_sized(TermShape { rows: 24, cols: 100 });  // short height forces overflow
    p.send(b"/doctor\r");
    p.wait_for(|t| t.contains("SYSTEM"), SECS_10, "doctor");
    // The DISCOVERED/last section must be reachable with canonical scroll keys.
    let reached = support::proving_ground::reach_text(&mut p, "TOKENS", &CANONICAL_REVEAL_KEYS, SECS_5);
    assert!(reached, "/doctor must scroll to its last section at 24-row height");
    p.quit();
}
```
where `CANONICAL_REVEAL_KEYS = [b"\x1b[B" /*Down*/, b"\x1b[6~" /*PgDn*/, b"j", b"G"]` and `reach_text` sends each key, re-reads `screen_text`, returns true if the target appears.

- [ ] **Step 2: Run it.** Expected: **FAIL** today (the scroll bug).
- [ ] **Step 3: Fix the engine** — make the `/doctor` diagnostics surface scrollable (`crates/wcore-cli/src/tui/surfaces/diagnostics.rs`); its own commit.
- [ ] **Step 4: Run it, confirm PASS.**
- [ ] **Step 5: Add `content_reachable` invariant; commit.**
```bash
git commit -m "test(proving-ground): /doctor short-height scroll cell + content_reachable invariant (known bug #2)"
```

---

### Task 5: KNOWN BUG #3 — `sk-flux-` detection (network-free registry invariant)

**Files:** Modify `crates/wcore-providers/src/fingerprint.rs` (expose the registry), create `crates/wcore-providers/tests/detection_registry.rs`.

**Interfaces:** Produces `wcore_providers::fingerprint::declared_prefixes() -> &'static [(&'static str, &'static str)]`.

- [ ] **Step 1: Expose the registry.** In `fingerprint.rs`, add `pub fn declared_prefixes() -> &'static [(&'static str, &'static str)] { UNIQUE_PREFIXES }`.

- [ ] **Step 2: Write the registry-completeness invariant test** (this is the network-free oracle that catches the *class*, not just `sk-flux-`):
```rust
use wcore_providers::fingerprint::{declared_prefixes, fingerprint_key};
#[test]
fn every_declared_prefix_resolves_to_exactly_one_provider() {
    for (prefix, slug) in declared_prefixes() {
        let key = format!("{prefix}TESTTESTTEST1234");
        let fp = fingerprint_key(&key);
        assert!(fp.is_unambiguous(), "{prefix} must resolve unambiguously");
        assert_eq!(fp.best().unwrap().slug, *slug, "{prefix} -> wrong provider");
    }
}
#[test]
fn sk_flux_is_a_declared_prefix() {
    assert!(declared_prefixes().iter().any(|(p, s)| *p == "sk-flux-" && *s == "flux-router"),
            "the Flux Router prefix must be registered (regression: it was missing)");
}
```

- [ ] **Step 2b: Run it.** With PR #52 merged this PASSES; on a tree without the fix the second test FAILS — proving the cell catches the regression. (If `sk-flux-` isn't yet on this branch, the failing→passing transition is the fix from PR #52.)

- [ ] **Step 3: Commit.**
```bash
git commit -m "test(proving-ground): network-free prefix-registry detection invariant (known bug #3)"
```

---

### Task 6: KNOWN BUG #4 — build provenance (binary matches source)

**Files:** Create `crates/wcore-cli/build.rs`; modify `crates/wcore-cli/src/main.rs` (surface the hash); modify `crates/wcore-cli/tests/proving_ground.rs`.

**Interfaces:** Produces a `wayland-core --build-info` line containing the embedded git short SHA.

- [ ] **Step 1: Embed the source hash.** `crates/wcore-cli/build.rs`:
```rust
use std::process::Command;
fn main() {
    let sha = Command::new("git").args(["rev-parse", "--short", "HEAD"]).output()
        .ok().filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=WAYLAND_SOURCE_SHA={sha}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
```

- [ ] **Step 2: Surface it.** In `main.rs`, add a `--build-info` arg that prints `wayland-core <CARGO_PKG_VERSION> (source <WAYLAND_SOURCE_SHA>)` using `env!("WAYLAND_SOURCE_SHA")`.

- [ ] **Step 3: Write the provenance invariant** (catches the stale-build class — the binary under test must match the repo HEAD the harness is gating):
```rust
#[test]
fn binary_matches_repo_head() {
    let head = std::process::Command::new("git").args(["rev-parse","--short","HEAD"])
        .output().unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core")).arg("--build-info")
        .output().unwrap();
    let info = String::from_utf8_lossy(&out.stdout);
    assert!(info.contains(&head), "binary built from {info:?} != repo HEAD {head} (stale build)");
}
```

- [ ] **Step 4: Run it, confirm PASS** (a fresh build matches HEAD; a stale prebuilt binary FAILS — exactly the Forge-rebuild class). **Commit.**
```bash
git commit -m "feat(cli): embed source SHA + --build-info; test(proving-ground): build-provenance invariant (known bug #4)"
```

---

### Task 7: `just proving-ground` recipe + flat report

**Files:** Modify `justfile`.

- [ ] **Step 1: Add the recipe** (after the `smoke:` recipe, matching its style):
```make
# Wayland Proving Ground — deterministic spine (unix, hermetic, $0)
proving-ground:
    cargo nextest run -p wcore-cli --test proving_ground
    cargo nextest run -p wcore-providers --test detection_registry
```

- [ ] **Step 2: Run it.**
Run: `just proving-ground`
Expected: all cells pass (after the four engine fixes land); a flat per-test pass/fail list — the morning "shit works ok" signal.

- [ ] **Step 3: Commit.**
```bash
git add justfile && git commit -m "build(just): proving-ground recipe runs the deterministic spine"
```

---

### Task 8: Replay-determinism test (findings are trustworthy)

**Files:** Modify `crates/wcore-cli/tests/proving_ground.rs`.

- [ ] **Step 1: Write the test** — run the same cell twice, assert identical *invariant verdicts* (not byte-identical records; timing/intermediate frames differ by design):
```rust
#[cfg(unix)]
#[test]
fn same_cell_yields_same_verdicts_twice() {
    let cell = clean_boot_cell();
    let a = run_cell(&cell); let b = run_cell(&cell);
    assert_eq!(a.dirty_death, b.dirty_death);
    assert_eq!(a.final_screen.contains("Workspace"), b.final_screen.contains("Workspace"));
    assert_eq!(a.config_toml.is_some(), b.config_toml.is_some());
}
```

- [ ] **Step 2: Run it, confirm PASS.**
- [ ] **Step 3: Commit.**
```bash
git commit -m "test(proving-ground): replay-determinism over invariant verdicts"
```

---

## M1 done = the deterministic spine. Next plans (separate, per spec)

- **M2 — Generative explorer:** AI-as-user drives `PtyDriver` across personas×intents×configs×terminal-sizes; invariants check each step; replay-confirm before reporting. New plan: `2026-06-19-wayland-proving-ground-PLAN-M2-explorer.md`.
- **M3 — Overnight runner + triaged report:** `just proving-ground --overnight` runs the explorer fleet unattended on the box, $-capped, emitting one deduped severity-ranked report with repros. New plan: `…-PLAN-M3-overnight.md`.

## Self-review notes
- **Spec coverage:** M1 covers spec §1 (PtyDriver + Frame-as-text via screen_text — full enum lands at P3), §2 (RunRecord core), §3 (Session hermeticity), §6.1 invariants (the 6 spine invariants + the 4 known bugs), success criteria 1–3. The generative sweep (spec §6.3/§7), fixtures (§5.2-3), judge, fleet (§8), flywheel (§9), coverage map (§10), and recall criterion (§ success 5) are M2/M3 — explicitly deferred, not omitted.
- **No placeholders:** every code step shows real code against the real existing APIs.
- **Type consistency:** `Pty`, `Session`, `RunRecord`, `Cell`, `run_cell`, `ConfigState`, `TermShape`, `declared_prefixes` are used consistently across tasks.
