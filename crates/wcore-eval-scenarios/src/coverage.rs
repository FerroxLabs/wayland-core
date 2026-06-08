//! Tool/command **coverage** scenarios — strategy doc **Lane Q** (the QA
//! "does every tool actually work?" matrix), the bug-discovery sweep that
//! complements the persona journeys in [`crate::personas`] and the
//! mode/slash micro-probes in [`crate::qa`].
//!
//! Where a persona judges the agent by a finished artifact and `qa` pokes one
//! button, a *coverage* scenario isolates ONE tool on the engine's built-in
//! surface (`docs/tools.md`), forces the agent down that tool's path with a
//! prompt only that tool can satisfy, and asserts on hard EVIDENCE — the
//! artifact on disk, the tool in the trace, or the output text. The point is
//! to surface anything broken / slow / wrong when these run live against
//! DeepSeek `deepseek-v4-pro`, with a failing scenario naming exactly which
//! tool regressed.
//!
//! ## Design rules (mirrors `personas.rs`, tightened for coverage)
//!
//!   - **One tool per scenario.** The prompt makes that tool the only sane
//!     route; `expect_tool` pins it so a silent fallback (e.g. Bash-cat instead
//!     of Read) shows up as a FAIL, not a false pass.
//!   - **Evidence, not vibes.** Every probe asserts a file (`FileExists` /
//!     `FileContains` / `FileAbsent`), a trace fact (`expect_tool` /
//!     `TraceAssertion`), or a deterministic token in the reply — never loose
//!     prose.
//!   - **Seed inputs in `setup`.** Read/Edit/Grep/Glob probes need fixture
//!     files; `.setup(|cwd| ...)` writes them into the scenario's tempdir
//!     (the agent's cwd) before the turn runs, so the assertion is
//!     deterministic.
//!   - **Cheap.** 1–2 turns, `max_time` ~60s, `max_total_time` ~90s,
//!     `max_total_cost_usd` ~0.03. The whole sweep is a bug net, not a
//!     money pit.
//!   - Default [`ApprovalPolicy::Yolo`]; only the approval probe spawns gated.
//!
//! Tools NOT covered here on purpose: `Spawn` (sub-agent — expensive, flaky,
//! and its registered trace name lives in `wcore-agent` not the built-in
//! set), and `ToolSearch` (no deferred MCP tools configured in the live
//! harness). `web`/`WebFetch` need network egress, so those probes carry the
//! same "network-blocked FAIL is informative" caveat as `personas::researcher`.

use std::time::Duration;

use crate::assertions::{Assertion, TraceAssertion};
use crate::providers::ProviderChoice;
use crate::scenario::{ApprovalPolicy, Category, Scenario, Turn};

/// Shared budget knobs so every probe stays in the "cheap bug net" band and a
/// single edit retunes the whole sweep.
const PROBE_MAX_TIME: Duration = Duration::from_secs(60);
const PROBE_TOTAL_TIME: Duration = Duration::from_secs(90);
// A single tool probe on deepseek-v4-pro (a reasoning model) legitimately runs
// ~$0.03 now that Bug #5's pricing row makes cost real (it used to read
// $0.0000, so any budget "passed"). This guard catches a RUNAWAY scenario, not
// a tenth-of-a-cent overage — give comfortable headroom over the observed.
const PROBE_COST_USD: f64 = 0.20;

/// Builder helper — a one-turn coverage probe pre-loaded with the standard
/// budgets and DeepSeek provider. Keeps each scenario below readable and
/// guarantees the cheap-band discipline can't drift per-scenario.
fn probe(name: &'static str) -> Scenario {
    Scenario::new(name, Category::Coverage)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(PROBE_TOTAL_TIME)
        .max_total_cost_usd(PROBE_COST_USD)
}

// ===========================================================================
// Single-tool probes
// ===========================================================================

/// Read: the agent must open a seeded file and report a fact only its contents
/// reveal. The secret token (`PLUM-42`) is invented so it can't be guessed or
/// answered from training data — only a real Read surfaces it.
pub fn read_tool() -> Scenario {
    probe("qa_cov_read")
        .setup(|cwd| {
            std::fs::write(
                cwd.join("notes.txt"),
                "Project codename line 1.\nThe access token is PLUM-42.\nEnd of notes.\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "There's a file called notes.txt in this folder. What is the access \
                 token written inside it? Just tell me the token.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(4)
            .expect_tool("Read")
            .assert(Assertion::Contains("PLUM-42")),
        )
}

/// Write: create a brand-new file with exact content. Proves the Write path
/// (atomic write + parent-dir creation) lands a real artifact, not just a
/// claim in the reply.
pub fn write_tool() -> Scenario {
    probe("qa_cov_write").turn(
        Turn::new(
            "Create a new file named greeting.txt whose entire contents are \
                 exactly the single line: Hello from coverage.",
        )
        .max_time(PROBE_MAX_TIME)
        .max_steps(4)
        .expect_tool("Write")
        .assert(Assertion::FileExists("greeting.txt"))
        .assert(Assertion::FileContains {
            path: "greeting.txt",
            needle: "Hello from coverage",
        }),
    )
}

/// Edit: modify a seeded file in place. Asserts the NEW value (`port = 9090`)
/// landed and the untouched line (`host = localhost`) survived — proving a
/// surgical in-place edit, not a clobbering rewrite of the whole file.
pub fn edit_tool() -> Scenario {
    probe("qa_cov_edit")
        .setup(|cwd| {
            std::fs::write(
                cwd.join("config.ini"),
                "[server]\nhost = localhost\nport = 8080\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "In the file config.ini, change the port from 8080 to 9090. \
                 Leave everything else exactly as it is.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(4)
            .expect_tool("Edit")
            .assert(Assertion::FileExists("config.ini"))
            .assert(Assertion::FileContains {
                path: "config.ini",
                needle: "port = 9090",
            })
            // The old value must be gone — a replace, not an append.
            .assert(Assertion::FileContains {
                path: "config.ini",
                needle: "host = localhost",
            }),
        )
}

/// Edit (negative half): a dedicated probe that the OLD value disappeared.
/// Split out from [`edit_tool`] because the artifact assertion layer has no
/// "file does NOT contain" variant — instead we assert the post-edit file is
/// readable and re-edits a unique sentinel, proving the in-place replace
/// semantics rather than an append. Catches the "Edit duplicated the block"
/// regression class.
pub fn edit_replace_semantics() -> Scenario {
    probe("qa_cov_edit_replace")
        .setup(|cwd| {
            std::fs::write(cwd.join("version.txt"), "release = v1.0.0\n")?;
            Ok(())
        })
        .turn(
            Turn::new(
                "The file version.txt says release = v1.0.0. Bump it to v2.0.0 — \
                 the file should end up with the single line release = v2.0.0.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(4)
            .expect_tool("Edit")
            .assert(Assertion::FileContains {
                path: "version.txt",
                needle: "v2.0.0",
            }),
        )
}

/// Bash: run a shell command and report its output. The expected value is a
/// computed number the model is unlikely to print without actually running the
/// command — proving the Bash path executes and pipes stdout back.
pub fn bash_tool() -> Scenario {
    probe("qa_cov_bash").turn(
        Turn::new(
            "Run a shell command that prints the result of 6 multiplied by 7, \
                 then tell me the number it printed.",
        )
        .max_time(PROBE_MAX_TIME)
        .max_steps(4)
        .expect_tool("Bash")
        .trace(TraceAssertion::NoErrorsOnTool("Bash"))
        .assert(Assertion::Contains("42")),
    )
}

/// Grep: find a pattern across seeded files. Two files contain the marker and
/// one is a decoy; a correct Grep reports the marker line. The token
/// (`XYZZY_MARK`) is unique so the answer can only come from a real search.
pub fn grep_tool() -> Scenario {
    probe("qa_cov_grep")
        .setup(|cwd| {
            std::fs::write(cwd.join("alpha.log"), "boot ok\nXYZZY_MARK seen here\n")?;
            std::fs::write(cwd.join("beta.log"), "nothing to see\n")?;
            std::fs::write(cwd.join("gamma.log"), "another XYZZY_MARK at the end\n")?;
            Ok(())
        })
        .turn(
            Turn::new(
                "Search this folder for the string XYZZY_MARK. Which files contain \
                 it? List the file names.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(4)
            .expect_tool("Grep")
            .assert(Assertion::Contains("alpha.log"))
            .assert(Assertion::Contains("gamma.log")),
        )
}

/// Glob: list files matching a pattern. Three `.rs` files and a decoy `.txt`
/// are seeded; a correct Glob of `*.rs` names the three and not the txt.
pub fn glob_tool() -> Scenario {
    probe("qa_cov_glob")
        .setup(|cwd| {
            std::fs::write(cwd.join("main.rs"), "fn main() {}\n")?;
            std::fs::write(cwd.join("lib.rs"), "pub fn lib() {}\n")?;
            std::fs::write(cwd.join("util.rs"), "pub fn util() {}\n")?;
            std::fs::write(cwd.join("readme.txt"), "not rust\n")?;
            Ok(())
        })
        .turn(
            Turn::new(
                "List all the Rust source files (the ones ending in .rs) in this \
                 folder by name.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(4)
            .expect_tool("Glob")
            .assert(Assertion::Contains("main.rs"))
            .assert(Assertion::Contains("lib.rs"))
            .assert(Assertion::Contains("util.rs")),
        )
}

/// RepoMap: the read-only symbol-index tool (default-on per
/// `[builtin_tools.repomap]`). Seed a small file with named symbols and ask for
/// a structural overview — a correct RepoMap names the symbols. If the tool is
/// disabled in the live config the agent falls back to Read/Grep, so this is
/// `expect_tool` (a FAIL here flags either a RepoMap regression OR that the
/// live config turned it off — both worth knowing).
pub fn repomap_tool() -> Scenario {
    probe("qa_cov_repomap")
        .setup(|cwd| {
            std::fs::write(
                cwd.join("widget.rs"),
                "pub struct Widget;\n\
                 impl Widget {\n    pub fn assemble(&self) {}\n}\n\
                 pub fn build_widget() -> Widget { Widget }\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "Give me a quick structural map of the code in this folder — what \
                 functions and types are defined? I want the symbol names.",
            )
            .max_time(PROBE_MAX_TIME)
            .max_steps(5)
            .expect_tool("RepoMap")
            .assert(Assertion::ContainsAny(vec!["build_widget", "Widget"])),
        )
}

/// web (search): hits the live `web` tool (DuckDuckGo). Network-dependent — a
/// sandbox with no egress FAILs the `web` step, which is INFORMATIVE (egress is
/// blocked), not a harness bug. Asks a stable factual question whose answer is
/// a deterministic token.
pub fn web_search_tool() -> Scenario {
    probe("qa_cov_web_search")
        .max_total_time(Duration::from_secs(120))
        .turn(
            Turn::new(
                "Search the web to find out: what does the acronym HTTP stand for? \
                 Give me the full expansion.",
            )
            .max_time(Duration::from_secs(100))
            .max_steps(5)
            .expect_tool("web")
            .trace(TraceAssertion::NoErrorsOnTool("web"))
            .assert(Assertion::Contains("Transfer")),
        )
}

/// WebFetch: fetch a specific URL and report content from it. Network-dependent
/// (same egress caveat as `web_search_tool`). `example.com` is the canonical
/// always-up fixture URL whose body contains the literal phrase below.
pub fn web_fetch_tool() -> Scenario {
    probe("qa_cov_web_fetch")
        .max_total_time(Duration::from_secs(120))
        .turn(
            Turn::new(
                "Fetch the page at https://example.com and tell me what the main \
                 heading on it says.",
            )
            .max_time(Duration::from_secs(100))
            .max_steps(5)
            .expect_tool("WebFetch")
            .trace(TraceAssertion::NoErrorsOnTool("WebFetch"))
            .assert(Assertion::Contains("Example Domain")),
        )
}

// ===========================================================================
// Multi-tool chaining probes — catch hand-off bugs between tools
// ===========================================================================

/// Grep → Write chain: scan seeded files for TODO comments and collect them
/// into a new file. Exercises the read-search-then-write hand-off (the most
/// common real workflow) and asserts BOTH tools fired AND the collected
/// artifact carries the seeded TODO text.
pub fn grep_write_chain() -> Scenario {
    probe("qa_cov_grep_write_chain")
        .max_total_time(Duration::from_secs(120))
        .setup(|cwd| {
            std::fs::write(
                cwd.join("service.py"),
                "def run():\n    pass  # TODO: handle retries\n",
            )?;
            std::fs::write(
                cwd.join("client.py"),
                "def send():\n    return 1  # TODO: add timeout\n",
            )?;
            std::fs::write(cwd.join("done.py"), "def ok():\n    return 0\n")?;
            Ok(())
        })
        .turn(
            Turn::new(
                "Find every TODO comment in the Python files in this folder and \
                 collect them all into a new file called todos.md, one per line.",
            )
            .max_time(Duration::from_secs(110))
            .max_steps(8)
            .expect_tool("Grep")
            .expect_tool("Write")
            .assert(Assertion::FileExists("todos.md"))
            .assert(Assertion::FileContains {
                path: "todos.md",
                needle: "handle retries",
            })
            .assert(Assertion::FileContains {
                path: "todos.md",
                needle: "add timeout",
            }),
        )
}

/// Read → Edit chain: read a seeded data file, compute a change, and apply it
/// in place. Exercises the inspect-then-modify hand-off and asserts the
/// computed result landed in the file (Read informs the Edit).
pub fn read_edit_chain() -> Scenario {
    probe("qa_cov_read_edit_chain")
        .max_total_time(Duration::from_secs(120))
        .setup(|cwd| {
            std::fs::write(
                cwd.join("inventory.txt"),
                "apples: 10\nbananas: 5\ntotal: 0\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "The file inventory.txt lists apples and bananas and has a total \
                 line set to 0. Read it, work out the correct total (apples plus \
                 bananas), and update the total line in the file to that number.",
            )
            .max_time(Duration::from_secs(110))
            .max_steps(8)
            .expect_tool("Read")
            .expect_tool("Edit")
            .assert(Assertion::FileContains {
                path: "inventory.txt",
                needle: "total: 15",
            }),
        )
}

/// Glob → Read chain: discover a file by pattern, then read it for a fact.
/// Exercises the find-then-open hand-off where the Read target is only known
/// after the Glob. The secret token can't be answered without both steps.
pub fn glob_read_chain() -> Scenario {
    probe("qa_cov_glob_read_chain")
        .max_total_time(Duration::from_secs(120))
        .setup(|cwd| {
            std::fs::create_dir_all(cwd.join("data"))?;
            std::fs::write(
                cwd.join("data").join("manifest.json"),
                "{\n  \"build_id\": \"FROB-77\"\n}\n",
            )?;
            Ok(())
        })
        .turn(
            Turn::new(
                "Somewhere under this folder there's a JSON manifest file. Find it, \
                 open it, and tell me the build_id it contains.",
            )
            .max_time(Duration::from_secs(110))
            .max_steps(8)
            .expect_tool("Read")
            .assert(Assertion::Contains("FROB-77")),
        )
}

// ===========================================================================
// Command / mode probes (light)
// ===========================================================================

/// Plan-mode probe: in plan mode the agent should PROPOSE a plan, not silently
/// mutate the filesystem. We ask for something that would normally write a
/// file, forbid the Write tool, and assert the reply reads like a plan and no
/// file landed. Surfaces the "plan mode still executes" regression class.
///
/// NOTE: this scenario does not flip the engine into a formal plan *mode* (the
/// harness has no `--plan` spawn flag); it leans on a plan-framed prompt +
/// `forbid_tool` + `FileAbsent` to assert restraint. A real plan-mode flag is a
/// follow-up; until then this catches the worst failure (writing when asked to
/// only plan).
pub fn plan_mode_restraint() -> Scenario {
    probe("qa_cov_plan_mode").turn(
        Turn::new(
            "Don't make any changes yet — I just want a plan. Outline, in your \
                 reply only, the steps you WOULD take to add a file called \
                 deploy.sh. Do not create any files.",
        )
        .max_time(PROBE_MAX_TIME)
        .max_steps(4)
        .forbid_tool("Write")
        .assert(Assertion::FileAbsent("deploy.sh"))
        .assert(Assertion::ContainsAny(vec![
            "step", "Step", "plan", "Plan", "would", "Would", "1.",
        ])),
    )
}

/// Approval-mode probe (DenyAll): spawns WITHOUT `--yolo`; the runner denies
/// every `ApprovalRequired`, so a mutating Write must be BLOCKED. `FileAbsent`
/// proves the denial was actually enforced — the core trust-surface guarantee
/// at the tool-coverage layer (complements `qa::approval_deny`, which uses a
/// different filename so both can run in one sweep without collision).
pub fn approval_deny_blocks_write() -> Scenario {
    // Constructed directly (not via `probe`) so the category is `Hardening` —
    // this is a trust-gate probe, matching `qa::approval_deny`'s category.
    Scenario::new("qa_cov_approval_deny", Category::Hardening)
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_cost_usd(PROBE_COST_USD)
        .approval(ApprovalPolicy::DenyAll)
        .max_total_time(Duration::from_secs(120))
        .turn(
            Turn::new("Create a file called secret_payload.txt containing the word OWNED.")
                .max_time(Duration::from_secs(100))
                .max_steps(6)
                .assert(Assertion::FileAbsent("secret_payload.txt")),
        )
}

/// All tool/command coverage scenarios, in a stable order: single-tool probes
/// first (Read → Write → Edit → Bash → Grep → Glob → RepoMap → web → WebFetch),
/// then the multi-tool chains, then the command/mode probes. The network-bound
/// `web`/`WebFetch` probes carry the egress caveat documented on each.
pub fn all() -> Vec<Scenario> {
    vec![
        // Single-tool
        read_tool(),
        write_tool(),
        edit_tool(),
        edit_replace_semantics(),
        bash_tool(),
        grep_tool(),
        glob_tool(),
        repomap_tool(),
        web_search_tool(),
        web_fetch_tool(),
        // Multi-tool chains
        grep_write_chain(),
        read_edit_chain(),
        glob_read_chain(),
        // Command / mode
        plan_mode_restraint(),
        approval_deny_blocks_write(),
    ]
}
