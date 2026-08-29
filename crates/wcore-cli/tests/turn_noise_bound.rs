//! #147 — what a headless run repeats on the terminal, per turn.
//!
//! # What was measured, and why a per-turn count is the only honest assertion
//!
//! A trivial 9-turn `--no-tui` run on a host with no usable OS keyring emitted
//! **573 bytes of stderr per turn**, of which **448** were one paragraph
//! reprinted verbatim on every turn. The condition it reports —
//! `wcore_config::config::replay_protection_unavailable()` — is resolved
//! once at startup and cannot change while the process lives, so every repeat
//! after the first carried no information at all.
//!
//! The operator had *already* been told, on the same stderr, by
//! `wcore_config::config`'s `warn_replay_protection_unavailable_once`, whose
//! own doc comment says the point is that they hear it "ONCE, at a moment that
//! is about configuration — **not repeatedly, attached to a message they were
//! trying to answer**". The engine's per-turn announcement re-introduced
//! exactly what that `Once` guard exists to prevent, one layer up.
//!
//! What the paragraph SAYS has since changed — ADR 0003's third decision let a
//! keyless host journal, so the notice now reports that crash replay is off
//! rather than that the turn goes unrecorded, and the record moved with it.
//! The repeat is what this file measures, not the wording; both matchers below
//! were re-pointed at the current text and neither was widened while doing it.
//!
//! A single-turn run cannot tell a startup notice from a per-turn notice, so
//! every test here runs THREE turns. That is also why the assertions are
//! `==` counts rather than `contains`: `contains` is satisfied by 1 and by 3
//! alike, and 3 is the defect.
//!
//! # The three legs, and what each would let through on its own
//!
//! 1. **The fact is announced once, in one wording.** The startup notice is
//!    counted `== 1` and the engine's per-turn restatement `== 0`. Either
//!    count alone is passed by a wrong fix — `== 0` by deleting both, `== 1`
//!    by restoring the duplicate — so both are asserted.
//! 2. **The per-turn RECORD survives, three times, in the diagnostics log.**
//!    This is the leg that refuses "quieted by deletion". Quieting the terminal
//!    is only legitimate because the forensic record moved to a durable,
//!    size-bounded file (`wcore_cli::log_rotate`) instead of a terminal that
//!    scrolls; a gateway operator asking "which of last week's messages could
//!    not have been replayed" reads that file.
//! 3. **A known-repeating line still repeats three times.** The positive
//!    control. Without it, leg 1 is equally consistent with the run having
//!    executed one turn, or none — and a count of 1 proves nothing about
//!    suppression if nothing was ever counted twice.
//!
//! The per-turn frame a `--json-stream` host receives is deliberately NOT
//! changed and is asserted elsewhere, by
//! `f14_sigkill_recovery::without_secure_store_the_default_journals_every_effect_but_seals_nothing`,
//! which drives two turns for the same reason this file drives three. A host
//! consumes `ProtocolEvent::Info` by machine and correlates it to a `msg_id`;
//! a human at a terminal does neither.
//!
//! Linux-only for the same reason the f14 degraded leg is: the degrade is
//! forced by pointing `DBUS_SESSION_BUS_ADDRESS` at a socket that does not
//! exist, which is how you deny a Secret Service keyring on Linux and nothing
//! at all anywhere else. The product behaviour under test is platform-neutral;
//! only the way to construct the precondition is not.
#![cfg(target_os = "linux")]

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::tempenv::{self, TempEnv};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

const FIXTURE_MODEL: &str = "fixture-chat-v1";
const FIXTURE_KEY: &str = "fixture-local-token";
const TURNS: usize = 3;

/// The immutable host fact, as the ENGINE's per-turn notice states it. Matched
/// on a distinctive clause rather than the whole paragraph so a wording edit
/// cannot silently turn the count into 0 and paint the defect green — it turns
/// it into 0 loudly, against an `== 1`, which is how this const was last found
/// to be stale.
///
/// It must NOT be the paragraph's opening words. `warn_replay_protection_unavailable_once`
/// prints its own startup notice to the same stderr, and that notice opens with
/// the same "crash replay protection is OFF for this run" clause; counting on
/// that prefix would score two surfaces as one and read a correct run as a
/// repeat. `This turn IS being recorded` appears only in the engine's per-turn
/// text — the startup notice speaks of the run, not of a turn.
const DEGRADE_NOTICE: &str = "This turn IS being recorded";

/// The startup notice `warn_replay_protection_unavailable_once` prints at
/// config resolution, matched on a clause that appears ONLY there — the engine
/// speaks of a turn, this speaks of the run, and only this one lists what is
/// still on.
///
/// This is the notice the operator is meant to read. It is asserted here
/// because leg 1 below now expects ZERO copies of the engine's wording, and a
/// `== 0` on its own is equally satisfied by a run that told the operator
/// nothing at all.
const STARTUP_NOTICE: &str = "Durable sessions stay ON";

/// The same fact as it reaches the size-bounded diagnostics log, once per turn.
/// A separate string from [`DEGRADE_NOTICE`] on purpose: the record and the
/// notice are written by two different calls and only one of them is allowed to
/// repeat, so a matcher that could satisfy leg 2 with leg 1's text would let a
/// deleted record hide behind a surviving notice.
const DEGRADE_RECORD: &str = "this turn cannot be replayed if it is interrupted";

/// A line the engine emits once per completed turn, unrelated to this change.
/// The positive control for the counting method itself.
const PER_TURN_CONTROL: &str = "[turns:";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// Run `TURNS` REPL turns against a loopback fixture in a home where no
/// credential backend can work, and return `(stderr, diagnostics log)`.
async fn run_degraded_repl() -> (String, String) {
    let fixture =
        OpenAiFixtureScript::new((0..TURNS).map(|i| OpenAiStep::text(format!("NOISE-REPLY-{i}"))))
            .start()
            .await
            .expect("start loopback fixture");

    let provider = ProviderConfig::new(ProviderId::OpenAI, FIXTURE_MODEL)
        .with_api_key(FIXTURE_KEY)
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    let env: TempEnv = tempenv::build(&provider).expect("build hermetic Core environment");

    let mut child = OwnedTree::new(
        Command::new(binary())
            .arg("--no-tui")
            .arg("--provider")
            .arg("openai")
            .arg("--model")
            .arg(FIXTURE_MODEL)
            .arg("--base-url")
            .arg(fixture.base_url())
            .current_dir(env.path())
            .env("HOME", env.path())
            .env("WAYLAND_HOME", env.home())
            .env("OPENAI_API_KEY", FIXTURE_KEY)
            .env("NO_COLOR", "1")
            // `log_to_file` is `will_enter_tui || !rust_log_set`. Stdout is a pipe
            // so the TUI half is already false; inheriting a developer's or CI's
            // RUST_LOG would route the record to stderr instead of the file and
            // make leg 2 assert on an empty log.
            .env_remove("RUST_LOG")
            // Deny every credential backend, which is what forces the degrade.
            .env_remove("WAYLAND_VAULT_PASSPHRASE")
            .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!(
                    "unix:path={}",
                    env.path().join("missing-secret-service-bus").display()
                ),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn wayland-core"),
    );

    let mut stdin = child.stdin.take().expect("Core stdin pipe");
    let mut script = String::new();
    for i in 0..TURNS {
        script.push_str(&format!("noise probe turn {i}\n"));
    }
    // The REPL treats an empty line as `/quit`.
    script.push('\n');
    stdin
        .write_all(script.as_bytes())
        .await
        .expect("write REPL script");
    stdin.flush().await.expect("flush REPL script");
    drop(stdin);

    let out = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        child.wait_with_output(),
    )
    .await
    .expect("the REPL must terminate")
    .expect("collect Core output");

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let log = std::fs::read_to_string(env.home().join("logs").join("wayland-core.log"))
        .unwrap_or_default();
    (stderr, log)
}

#[tokio::test]
async fn the_degrade_notice_is_announced_once_while_its_per_turn_record_survives() {
    let (stderr, log) = run_degraded_repl().await;

    // POSITIVE CONTROL, first, because every other count in this test is
    // meaningless if the run did not actually execute three turns — and a
    // suppression assertion that passes because nothing happened is the
    // exact false green this file exists to refuse.
    assert_eq!(
        count(&stderr, PER_TURN_CONTROL),
        TURNS,
        "the run did not execute {TURNS} turns, so no count below measures \
         suppression. stderr was:\n{stderr}"
    );

    // LEG 1 — the human hears the immutable host fact ONCE PER RUN, in one
    // wording. The engine's per-turn wording is not that wording: config
    // resolution already printed the fuller startup notice to this same
    // stderr, so a single copy of the engine's paragraph is still the same
    // fact told twice — measured at 1,333 of a trivial run's 2,019 stderr
    // bytes, 66% of it. The two counts are asserted together because either
    // alone is passed by the wrong fix: `== 0` alone by deleting both, `== 1`
    // alone by restoring the duplicate.
    assert_eq!(
        count(&stderr, STARTUP_NOTICE),
        1,
        "the operator must be told, once, that crash replay is off. The \
         startup notice appeared {} times.\nstderr was:\n{stderr}",
        count(&stderr, STARTUP_NOTICE)
    );
    assert_eq!(
        count(&stderr, DEGRADE_NOTICE),
        0,
        "the engine's per-turn wording appeared {} times on a terminal that \
         had already read the startup notice. It reports a startup-resolved \
         host fact that cannot change mid-process; restating it in different \
         words is not new information.\nstderr was:\n{stderr}",
        count(&stderr, DEGRADE_NOTICE)
    );

    // LEG 2 — and it was quieted by MOVING the record, not by deleting it.
    // Without this, leg 1 is satisfied by dropping the announcement entirely,
    // which would leave a degraded gateway with no per-turn evidence anywhere.
    assert_eq!(
        count(&log, DEGRADE_RECORD),
        TURNS,
        "the diagnostics log holds {} per-turn unreplayable-turn records for \
         {TURNS} unreplayable turns. Quieting the terminal is only legitimate \
         while the record survives in the size-bounded log.\nlog was:\n{log}",
        count(&log, DEGRADE_RECORD)
    );
}

/// The other half of #147: a per-turn log line that reported nothing.
///
/// `fire_auto_memorize` runs at the end of every turn and logged
/// "auto-memorize skipped this session; no facts saved … candidates=0" at INFO
/// each time — measured as the ONLY per-turn line in the diagnostics log, on a
/// healthy host as well as a degraded one, 9 of them in a 9-turn run. Nothing
/// was extracted and nothing was rejected, so there was no decision to report.
///
/// The #664 requirement it was added for — surface WHY an operator's facts were
/// not saved — only has content when there were facts, so INFO is now
/// conditioned on that and the empty case dropped to DEBUG. This asserts the
/// empty case is silent at the default level; `RUST_LOG=debug` still shows it.
#[tokio::test]
async fn an_empty_auto_memorize_pass_says_nothing_at_the_default_level() {
    let (_stderr, log) = run_degraded_repl().await;

    let skips = count(&log, "auto-memorize skipped this session");
    assert_eq!(
        skips, 0,
        "auto-memorize logged {skips} skip lines at INFO across {TURNS} turns \
         in which it extracted no candidates at all. A per-turn line reporting \
         that nothing happened is the whole of the finding.\nlog was:\n{log}"
    );

    // Control: the log is not empty, so the count above measures silence
    // rather than a log that was never written or never read.
    assert!(
        !log.trim().is_empty(),
        "the diagnostics log is empty, so a zero count proves nothing about \
         auto-memorize"
    );
}
