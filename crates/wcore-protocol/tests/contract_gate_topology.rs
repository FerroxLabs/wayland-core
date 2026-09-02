//! Where the Desktop contract corpus gate is allowed to run (gh#1055 ask #2).
//!
//! The corpus check is platform-independent bookkeeping: it regenerates in
//! memory and byte-compares the result against `contracts/desktop/v1`. Running
//! it on every leg turned one fact - "a `SOURCE_INPUTS` file's hash moved" -
//! into a board where `CI (Array)`, `CI (linux-containerized)` and
//! `CI (macos-latest)` were red simultaneously, which is the most alarming
//! shape a CI board can show and is the reason two PRs lost a night to it.
//!
//! So the gate is deliberately narrowed:
//!
//! - the standalone `wcore-contract check` STEP lives in exactly one job,
//!   `ci-linux`, instead of once per entry of the native matrix;
//! - the one test that compares the checked-in corpus against the generator,
//!   `checked_corpus_matches_real_serializers_byte_for_byte`, is compiled out
//!   on macOS. Linux already covers everything macOS would, and the WINDOWS
//!   leg is kept on purpose: `all_relative_files`' `.replace('\\', "/")` is the
//!   one genuinely platform-sensitive line in the generator, so Windows is the
//!   leg worth having and macOS is the redundant one.
//!
//! None of that is expressible as a compile error, and a wrong `if:` or a
//! dropped `#[cfg]` removes coverage SILENTLY rather than failing loudly -
//! which is exactly the failure mode this file exists to make loud. It asserts
//! the arrangement over the real `.github/workflows/ci.yml` and the real test
//! source, so putting the step back on the matrix, or quietly widening the
//! macOS exclusion to the whole binary, fails here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> always has a workspace root two levels up")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} must be readable to check the contract gate topology: {error}",
            path.display()
        )
    })
}

/// The workflow job owning each line of `ci.yml`, by line index.
///
/// Job keys are the only two-space-indented bare keys under `jobs:`, so this
/// needs no YAML parser and cannot be confused by the step bodies (six spaces)
/// or by the top-level `on:`/`concurrency:` blocks (before `jobs:`).
fn owning_jobs(workflow: &str) -> Vec<Option<String>> {
    let mut owner: Option<String> = None;
    let mut seen_jobs_key = false;
    let mut owners = Vec::new();
    for line in workflow.lines() {
        if line == "jobs:" {
            seen_jobs_key = true;
        } else if seen_jobs_key
            && line.starts_with("  ")
            && !line.starts_with("   ")
            && line.ends_with(':')
        {
            let key = line.trim_end_matches(':').trim();
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                owner = Some(key.to_string());
            }
        }
        owners.push(owner.clone());
    }
    owners
}

/// Line indices where `needle` appears on a line that is not a comment in
/// `comment` syntax.
///
/// Comment lines are excluded deliberately: `ci.yml` carries long prose blocks
/// that name the very commands asserted on here, and a gate that matches a
/// comment proves nothing.
fn executable_hits(text: &str, needle: &str, comment: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with(comment) && line.contains(needle))
        .map(|(index, _)| index)
        .collect()
}

fn is_step_start(line: &str) -> bool {
    line.starts_with("      - ") && !line.starts_with("       ")
}

/// The line index where the step containing `index` begins.
fn step_start(lines: &[&str], index: usize) -> usize {
    (0..=index)
        .rev()
        .find(|&i| is_step_start(lines[i]))
        .unwrap_or_else(|| panic!("line {} is not inside a workflow step", index + 1))
}

/// The `key: value` entries directly under the block opening at `start`.
fn entries(lines: &[&str], start: usize, indent: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in &lines[start + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(indent) {
            break;
        }
        let rest = &line[indent.len()..];
        if rest.starts_with(' ') {
            break;
        }
        if rest.starts_with('#') {
            continue;
        }
        match rest.split_once(':') {
            Some((key, value)) => {
                found.insert(key.trim().to_string(), value.trim().to_string());
            }
            None => break,
        }
    }
    found
}

/// The permissions GHA actually grants `job`'s `GITHUB_TOKEN`.
///
/// Effective, not merged: a job-level `permissions:` block REPLACES the
/// workflow-level one outright, and every scope the winning block omits is set
/// to `none`. That is the whole hazard this models - an omitted scope reads as
/// "unchanged" and behaves as "revoked".
fn effective_permissions(workflow: &str, job: &str) -> BTreeMap<String, String> {
    let lines = workflow.lines().collect::<Vec<_>>();
    let owners = owning_jobs(workflow);
    let job_level = (0..lines.len())
        .find(|&i| owners[i].as_deref() == Some(job) && lines[i] == "    permissions:");
    if let Some(start) = job_level {
        return entries(&lines, start, "      ");
    }
    match (0..lines.len()).find(|&i| lines[i] == "permissions:" && owners[i].is_none()) {
        Some(start) => entries(&lines, start, "  "),
        None => BTreeMap::new(),
    }
}

/// The bare job-level keys (`runs-on`, `if`, ...) declared by `job`.
fn job_level_keys(workflow: &str, job: &str) -> Vec<String> {
    let owners = owning_jobs(workflow);
    workflow
        .lines()
        .enumerate()
        .filter(|(index, line)| {
            owners[*index].as_deref() == Some(job)
                && line.starts_with("    ")
                && !line.starts_with("     ")
                && line.contains(':')
        })
        .filter_map(|(_, line)| line.trim().split_once(':').map(|(key, _)| key.to_string()))
        .collect()
}

/// The lines of the `- name:` step containing `index`.
fn step_body(lines: &[&str], index: usize) -> Vec<String> {
    let start = step_start(lines, index);
    let end = (start + 1..lines.len())
        .find(|&i| is_step_start(lines[i]))
        .unwrap_or(lines.len());
    lines[start..end].iter().map(|l| l.to_string()).collect()
}

#[test]
fn the_corpus_drift_step_runs_in_exactly_one_job_and_that_job_is_linux() {
    let workflow = read(".github/workflows/ci.yml");
    let owners = owning_jobs(&workflow);
    let hits = executable_hits(&workflow, "wcore-contract -- check", "#");
    assert_eq!(
        hits.len(),
        1,
        "the standalone corpus check must appear exactly once in ci.yml; found it on lines {:?}. \
         One fact should not light N boxes (gh#1055 ask 2).",
        hits.iter().map(|i| i + 1).collect::<Vec<_>>()
    );
    assert_eq!(
        owners[hits[0]].as_deref(),
        Some("ci-linux"),
        "the corpus check step must live in the containerized Linux job, not in the native \
         matrix (it is platform-independent bookkeeping); it is on ci.yml:{} in job {:?}",
        hits[0] + 1,
        owners[hits[0]]
    );
}

#[test]
fn the_preflight_hint_runs_on_linux_and_can_never_fail_the_build() {
    let workflow = read(".github/workflows/ci.yml");
    let lines = workflow.lines().collect::<Vec<_>>();
    let owners = owning_jobs(&workflow);
    let hits = executable_hits(&workflow, "wcore-contract -- preflight", "#");
    assert_eq!(
        hits.len(),
        1,
        "the pre-flight hint must appear exactly once in ci.yml; found it on lines {:?}",
        hits.iter().map(|i| i + 1).collect::<Vec<_>>()
    );
    assert_eq!(
        owners[hits[0]].as_deref(),
        Some("ci-linux"),
        "the pre-flight hint belongs on the single Linux job; it is on ci.yml:{} in job {:?}",
        hits[0] + 1,
        owners[hits[0]]
    );
    let body = step_body(&lines, hits[0]);
    assert!(
        body.iter()
            .any(|line| line.trim() == "continue-on-error: true"),
        "the pre-flight step is a HINT and must never gate a PR - it has no \
         `continue-on-error: true`:\n{}",
        body.join("\n")
    );
}

#[test]
fn only_macos_is_dropped_from_the_corpus_drift_test_and_only_that_one_test() {
    let source = read("crates/wcore-protocol/tests/desktop_contract_corpus.rs");
    let lines = source.lines().collect::<Vec<_>>();
    let signature = "fn checked_corpus_matches_real_serializers_byte_for_byte";
    let hits = executable_hits(&source, signature, "//");
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one definition of the drift test; found lines {:?}",
        hits.iter().map(|i| i + 1).collect::<Vec<_>>()
    );

    // Attributes are the contiguous `#[...]` lines immediately above the fn.
    let attributes = (0..hits[0])
        .rev()
        .take_while(|&i| lines[i].trim_start().starts_with("#["))
        .map(|i| lines[i].trim().to_string())
        .collect::<Vec<_>>();
    assert!(
        attributes
            .iter()
            .any(|attribute| attribute.contains("cfg(not(target_os = \"macos\"))")),
        "the on-disk-vs-generator drift test must be compiled out on macOS, so a source-hash \
         rebase cannot redden a third platform for a fact Linux already reported. Attributes \
         found: {attributes:?}"
    );

    // The exclusion is one test, not the binary. `#![cfg(...)]` at the top of
    // this file would silently delete every other protocol assertion it holds
    // on macOS - the opposite of what ask #2 asked for.
    assert!(
        !source.contains("#![cfg("),
        "the macOS exclusion must be scoped to the drift test, never applied to the whole \
         desktop_contract_corpus binary"
    );
}

/// Ask #3's hint reads the PR's changed-file list over the REST API. The
/// workflow-level `permissions:` block grants contents+checks only, and GHA
/// sets every scope a block omits to `none`, so without an explicit
/// pull-requests grant `gh api` 403s. Because the step is `continue-on-error`,
/// that 403 does not redden anything: the notice simply never appears, on any
/// PR, forever. This repo has already paid "multiple weeks of red" for the same
/// omission on the `report` job.
#[test]
fn the_job_carrying_the_preflight_hint_can_actually_read_the_pull_request() {
    let workflow = read(".github/workflows/ci.yml");
    let owners = owning_jobs(&workflow);
    let hits = executable_hits(&workflow, "wcore-contract -- preflight", "#");
    assert_eq!(
        hits.len(),
        1,
        "the pre-flight hint must appear exactly once in ci.yml; found lines {:?}",
        hits.iter().map(|i| i + 1).collect::<Vec<_>>()
    );
    let job = owners[hits[0]]
        .clone()
        .expect("the hint must live inside a job");
    let granted = effective_permissions(&workflow, &job);

    // Known-positive control. `actions/checkout` in this job needs contents:read
    // and has always had it, so if THIS is missing the permission parse above is
    // wrong - and the real assertion below would be passing for the wrong reason
    // or failing for one.
    assert_eq!(
        granted.get("contents").map(String::as_str),
        Some("read"),
        "control failed: contents:read is what actions/checkout in `{job}` needs and has always \
         had, so its absence means this permission parse is broken rather than the workflow. \
         Parsed: {granted:?}"
    );

    assert!(
        matches!(
            granted.get("pull-requests").map(String::as_str),
            Some("read") | Some("write")
        ),
        "job `{job}` runs `gh api repos/.../pulls/N/files` for the corpus pre-flight hint, which \
         needs a pull-requests grant. A `permissions:` block sets every scope it omits to `none`, \
         and the step is `continue-on-error`, so this omission never reddens anything - `gh` just \
         403s and the hint is never emitted at all. Effective permissions: {granted:?}"
    );
}

/// A pre-flight hint that arrives post-flight is not a hint. It first shipped as
/// step 11 of ~15 - after fmt, clippy, the whole nextest suite, the swarm suite
/// and the voice suite - where it announced a failure the reader had already
/// waited through, and where any earlier red aborted the job before it ran at
/// all. Both bounds are asserted: it cannot precede the image it runs inside,
/// and it must precede every step that uses that image.
#[test]
fn the_preflight_hint_runs_before_every_step_it_is_meant_to_pre_empt() {
    let workflow = read(".github/workflows/ci.yml");
    let lines = workflow.lines().collect::<Vec<_>>();
    let owners = owning_jobs(&workflow);
    let hits = executable_hits(&workflow, "wcore-contract -- preflight", "#");
    assert_eq!(hits.len(), 1);
    let job = owners[hits[0]]
        .clone()
        .expect("the hint must live inside a job");
    let hint = step_start(&lines, hits[0]);

    let image = executable_hits(&workflow, "docker build -t \"$CI_IMAGE\"", "#")
        .into_iter()
        .find(|&index| owners[index].as_deref() == Some(job.as_str()))
        .map(|index| step_start(&lines, index))
        .unwrap_or_else(|| {
            panic!("control failed: `{job}` must build the CI image the hint runs inside")
        });
    assert!(
        image < hint,
        "the hint runs the real binary inside $CI_IMAGE, so it cannot precede the image build \
         (ci.yml:{} vs the hint at ci.yml:{}) - moved above it, it dies on a missing image \
         instead of hinting",
        image + 1,
        hint + 1
    );

    let flight = executable_hits(&workflow, "$DOCKER_RUN", "#")
        .into_iter()
        .filter(|&index| owners[index].as_deref() == Some(job.as_str()))
        .map(|index| step_start(&lines, index))
        .filter(|&start| start != hint)
        .collect::<Vec<_>>();
    assert!(
        !flight.is_empty(),
        "control failed: `{job}` runs every compile, lint and test step through $DOCKER_RUN, so \
         finding none means this search is broken, not that the job is empty"
    );
    let first = *flight.iter().min().expect("non-empty");
    assert!(
        hint < first,
        "the pre-flight hint is wired POST-flight: it starts at ci.yml:{} while the first step \
         that compiles, lints or tests anything starts at ci.yml:{}. Its entire value is telling \
         the author BEFORE those minutes are spent, and an earlier gating failure aborts the job \
         so the hint is never emitted at all (gh#1055 ask 3).",
        hint + 1,
        first + 1
    );
}

/// The mirror of `the_preflight_hint_runs_on_linux_and_can_never_fail_the_build`,
/// and the gap that test left open: it asserted `continue-on-error` on the
/// ADVISORY step while saying nothing about the GATING one. Measured before this
/// existed - adding `if: false` + `continue-on-error: true` to the corpus drift
/// step left every test in this file green, which is precisely the silent
/// coverage loss the file's own doctrine says it exists to make loud.
#[test]
fn the_corpus_drift_step_is_a_gate_and_carries_nothing_that_could_silence_it() {
    let workflow = read(".github/workflows/ci.yml");
    let lines = workflow.lines().collect::<Vec<_>>();
    let owners = owning_jobs(&workflow);
    let hits = executable_hits(&workflow, "wcore-contract -- check", "#");
    assert_eq!(hits.len(), 1);
    let body = step_body(&lines, hits[0]);

    // Control: the body really is the drift step's. Without it every assertion
    // below would also hold for an empty or misaligned slice.
    assert!(
        body.first().is_some_and(
            |line| line.contains("- name: Check Desktop protocol contract corpus drift")
        ),
        "control failed: the located step body does not start at the drift step:\n{}",
        body.join("\n")
    );
    for silencer in ["if:", "continue-on-error:"] {
        assert!(
            !body.iter().any(|line| line.trim().starts_with(silencer)),
            "ask #2 narrowed the corpus gate to ONE standalone step; a `{silencer}` on it \
             removes that coverage silently rather than loudly, which is the exact failure mode \
             this file exists to catch. Step body:\n{}",
            body.join("\n")
        );
    }

    // Same silencers one level up: an `if:` or `continue-on-error:` on the JOB
    // kills the gate just as quietly as one on the step.
    let job = owners[hits[0]]
        .clone()
        .expect("the gate must live inside a job");
    let keys = job_level_keys(&workflow, &job);
    assert!(
        keys.iter().any(|key| key == "runs-on"),
        "control failed: every job declares `runs-on`, so parsing none from `{job}` means this \
         key scan is broken: {keys:?}"
    );
    for silencer in ["if", "continue-on-error"] {
        assert!(
            !keys.iter().any(|key| key == silencer),
            "job `{job}` carries a job-level `{silencer}:`, which silences the corpus gate as \
             completely as one on the step itself: {keys:?}"
        );
    }
}

/// The LANE gate carries the corpus check too, and it is not advisory.
///
/// The CI step asserted above is the last line of defence, not the first, and
/// on 2026-08-30 it was the ONLY one. A lane edited `wcore-cli/src/main.rs` —
/// a `SOURCE_INPUTS` file, hashed by path and read from disk at test time —
/// tested `-p wcore-mcp -p wcore-cli`, ran `scripts/preflight.sh`, got 0 from
/// both, and reported the tree green. `-p wcore-protocol` was 100.
///
/// The lane's gate could not have caught it: nothing in `preflight.sh` looked
/// at the corpus, and `contract::preflight`'s hint is advisory by construction
/// — it returns a message, never an error — so a lane can quote `PREFLIGHT=0`
/// as coverage it is not. The remedy is in `preflight.sh` now, as a FAILING
/// gate, and it asks the deciding question directly (is the corpus current
/// with the tree?) rather than the proxy the hint asks (did the diff touch a
/// listed path?), so no diff base and no path spelling can defeat it.
///
/// A shell script cannot fail to contain a line, so this is what makes its
/// removal loud instead of silent.
#[test]
fn the_lane_preflight_gates_on_corpus_currency_rather_than_hinting_at_it() {
    let preflight = read("scripts/preflight.sh");

    // Control on the read: this is the pre-flight, not some other script.
    assert!(
        preflight.contains("PRE-FLIGHT PASSED"),
        "control failed: scripts/preflight.sh does not look like the lane \
         pre-flight, so every assertion below is about the wrong file"
    );

    let hits = executable_hits(&preflight, "wcore-contract -- check", "#");
    assert_eq!(
        hits.len(),
        1,
        "scripts/preflight.sh must run `wcore-contract -- check` exactly once, \
         on a line that is not a comment. Without it a lane that edits a \
         SOURCE_INPUTS file passes its own gate and hands the red to whoever \
         integrates (measured: lane/f13-w2-mcp-transports, 2026-08-30)"
    );

    // ADVISORY IS NOT A GATE. `wcore-contract -- preflight` prints a hint and
    // exits 0 by design; accepting it here would restore exactly the hole this
    // test closes.
    assert!(
        executable_hits(&preflight, "wcore-contract -- preflight", "#").is_empty(),
        "the pre-flight hint exits 0 whatever it finds. It is a hint, not the \
         gate — `wcore-contract -- check` is the gate"
    );

    // And the result must reach the exit code. A gate whose failure is printed
    // and then dropped is the advisory hint wearing a gate's name.
    let line = preflight.lines().nth(hits[0]).expect("hit line exists");
    let assigned = line
        .split_once('=')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.contains(' '))
        .expect("the corpus gate is bound to a shell variable this test can follow");
    assert!(
        preflight.contains(&format!("if out=\"$(${assigned} 2>&1)\"")),
        "the corpus gate is defined as `{assigned}` but is never run into the \
         `fail` accumulator, so a red would print and the script would still \
         exit 0"
    );
    assert!(
        preflight.contains("  fail=1"),
        "control failed: the pre-flight no longer accumulates failures the way \
         this test assumes it does"
    );
}
