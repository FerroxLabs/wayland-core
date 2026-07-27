//! The automation surface an operator actually types.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! Every case here goes through `wcore_cli::cron::run_with_store`, which is
//! the SAME entry point `wayland-core cron ...` dispatches to — not a
//! reimplementation of it. A test that constructed `CronJob`s directly would
//! prove the model and nothing about the surface.

use wcore_cli::cron::{CronCmd, parse_phrase, run_with_store};
use wcore_cron::store::CronStore;
use wcore_cron::trigger::Trigger;
use wcore_cron::{CronJob, FileCronStore};

fn store(dir: &std::path::Path) -> FileCronStore {
    FileCronStore::new(dir.join("jobs.json"))
}

fn add(trigger: &str) -> CronCmd {
    CronCmd::Add {
        expression: None,
        trigger: Some(trigger.to_string()),
        describe: None,
        confirm: false,
        slash: Some("/brief".into()),
        channel: None,
        text: None,
        skill: None,
        args: None,
    }
}

fn describe(phrase: &str, confirm: bool) -> CronCmd {
    CronCmd::Add {
        expression: None,
        trigger: None,
        describe: Some(phrase.to_string()),
        confirm,
        slash: Some("/brief".into()),
        channel: None,
        text: None,
        skill: None,
        args: None,
    }
}

async fn only_job(s: &FileCronStore) -> CronJob {
    let jobs = s.list().await.unwrap();
    assert_eq!(jobs.len(), 1, "expected exactly one persisted job");
    jobs.into_iter().next().unwrap()
}

// ---------------------------------------------------------------------------
// Every trigger type is addable from the one existing verb
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_trigger_kind_is_addable_through_the_existing_add_verb() {
    // One surface, seven types. A second command surface for the new types
    // would leave the old verbs stranded and the operator guessing which one
    // a given job lives under.
    let cases: Vec<(&str, &str)> = vec![
        ("once:2026-08-01T09:00:00Z", "once"),
        ("every:900", "interval"),
        ("cron:0 9 * * *", "cron"),
        ("event:build.finished", "event"),
        ("webhook:/hooks/build", "webhook"),
        ("poll:https://status.test/health:300", "poll"),
        ("commit:2026-08-01T17:00:00Z:900", "commitment"),
    ];
    assert_eq!(
        cases.len(),
        Trigger::KINDS.len(),
        "a trigger kind exists with no CLI case"
    );

    for (spec, kind) in cases {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(add(spec), &s)
            .await
            .unwrap_or_else(|e| panic!("cron add --trigger {spec:?} failed: {e:#}"));
        let job = only_job(&s).await;
        assert_eq!(
            job.effective_trigger().kind(),
            kind,
            "--trigger {spec:?} produced the wrong kind"
        );
        // And it is listable and statusable through the same verbs.
        run_with_store(CronCmd::List, &s).await.unwrap();
        run_with_store(CronCmd::Status { id: job.id.clone() }, &s)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn an_unknown_trigger_kind_is_refused_rather_than_reinterpreted() {
    // Silently falling back to cron would persist a job that never fires and
    // never says why.
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let err = run_with_store(add("teleport:tuesday"), &s)
        .await
        .expect_err("an unknown kind must be refused");
    assert!(
        format!("{err:#}").contains("teleport"),
        "the refusal must name what it refused, got {err:#}"
    );
    assert!(s.list().await.unwrap().is_empty(), "nothing may be written");
}

#[tokio::test]
async fn a_webhook_is_authenticated_unless_open_is_typed_out() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    run_with_store(add("webhook:/hooks/x"), &s).await.unwrap();
    match only_job(&s).await.effective_trigger() {
        Trigger::Webhook { require_auth, .. } => assert!(
            require_auth,
            "an unqualified webhook must be authenticated by default"
        ),
        other => panic!("expected a webhook, got {other:?}"),
    }

    let dir2 = tempfile::tempdir().unwrap();
    let s2 = store(dir2.path());
    run_with_store(add("webhook:/hooks/x:open"), &s2)
        .await
        .unwrap();
    match only_job(&s2).await.effective_trigger() {
        Trigger::Webhook { require_auth, .. } => assert!(
            !require_auth,
            "an explicitly opened endpoint must be recorded as open"
        ),
        other => panic!("expected a webhook, got {other:?}"),
    }
}

#[tokio::test]
async fn the_timing_must_be_given_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());

    // None.
    let none = CronCmd::Add {
        expression: None,
        trigger: None,
        describe: None,
        confirm: false,
        slash: Some("/x".into()),
        channel: None,
        text: None,
        skill: None,
        args: None,
    };
    assert!(run_with_store(none, &s).await.is_err());

    // Two — a silent precedence rule here would mean the operator's other
    // instruction was discarded without a word.
    let both = CronCmd::Add {
        expression: Some("0 9 * * *".into()),
        trigger: Some("every:900".into()),
        describe: None,
        confirm: false,
        slash: Some("/x".into()),
        channel: None,
        text: None,
        skill: None,
        args: None,
    };
    assert!(run_with_store(both, &s).await.is_err());
    assert!(s.list().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Natural-language authoring produces a reviewable artefact, not a hidden job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_phrase_writes_nothing_until_it_is_confirmed() {
    // This is the safety property of the whole authoring aid: a background
    // runtime that silently schedules whatever a sentence was interpreted to
    // mean is a correctness problem and a safety problem at once.
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());

    run_with_store(describe("every weekday at 9am", false), &s)
        .await
        .expect("an interpretable phrase must succeed as a preview");
    assert!(
        s.list().await.unwrap().is_empty(),
        "an unconfirmed phrase must persist NOTHING"
    );

    run_with_store(describe("every weekday at 9am", true), &s)
        .await
        .unwrap();
    let job = only_job(&s).await;
    assert_eq!(
        job.effective_trigger(),
        Trigger::Cron {
            expression: "0 9 * * 1-5".into()
        }
    );
}

#[tokio::test]
async fn an_uninterpretable_phrase_is_quoted_back_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let phrase = "whenever the vibes are right";
    let err = run_with_store(describe(phrase, true), &s)
        .await
        .expect_err("an uninterpretable phrase must not be guessed at");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(phrase),
        "the phrase must be quoted back verbatim, got {msg}"
    );
    assert!(
        s.list().await.unwrap().is_empty(),
        "an uninterpretable phrase must persist nothing, even with --confirm"
    );
}

#[test]
fn the_phrase_vocabulary_is_exactly_what_is_documented() {
    // Each of these appears in the automation contract document. A phrase
    // OUTSIDE the vocabulary must return None rather than a near-miss: a
    // fuzzy match is how a sentence becomes a schedule the operator did not
    // intend.
    let cases: &[(&str, Trigger)] = &[
        ("every 15 minutes", Trigger::Interval { every_secs: 900 }),
        ("every 2 hours", Trigger::Interval { every_secs: 7200 }),
        ("every minute", Trigger::Interval { every_secs: 60 }),
        (
            "every day at 9am",
            Trigger::Cron {
                expression: "0 9 * * *".into(),
            },
        ),
        (
            "daily at 17:30",
            Trigger::Cron {
                expression: "30 17 * * *".into(),
            },
        ),
        (
            "every weekday at 9am",
            Trigger::Cron {
                expression: "0 9 * * 1-5".into(),
            },
        ),
        (
            "every monday at 8:15am",
            Trigger::Cron {
                expression: "15 8 * * 1".into(),
            },
        ),
        (
            "every day at 12pm",
            Trigger::Cron {
                expression: "0 12 * * *".into(),
            },
        ),
        (
            "every day at 12am",
            Trigger::Cron {
                expression: "0 0 * * *".into(),
            },
        ),
    ];
    for (phrase, expect) in cases {
        assert_eq!(
            parse_phrase(phrase).as_ref(),
            Some(expect),
            "phrase {phrase:?} resolved wrongly"
        );
    }

    for rejected in [
        "whenever",
        "every 15 fortnights",
        "every day at 25:00",
        "every day at 9:99",
        "every blursday at 9am",
        "",
    ] {
        assert!(
            parse_phrase(rejected).is_none(),
            "phrase {rejected:?} must be refused rather than guessed at"
        );
    }
}

// ---------------------------------------------------------------------------
// The existing verbs still work, unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_pre_existing_positional_expression_form_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let cmd = CronCmd::Add {
        expression: Some("0 9 * * *".into()),
        trigger: None,
        describe: None,
        confirm: false,
        slash: Some("/legacy".into()),
        channel: None,
        text: None,
        skill: None,
        args: None,
    };
    run_with_store(cmd, &s).await.unwrap();
    let job = only_job(&s).await;
    assert_eq!(job.expression, "0 9 * * *");
    assert!(
        job.trigger.is_none(),
        "the historical form must not start writing a trigger field"
    );

    for cmd in [
        CronCmd::List,
        CronCmd::Status { id: job.id.clone() },
        CronCmd::Disable { id: job.id.clone() },
        CronCmd::Enable { id: job.id.clone() },
        CronCmd::Remove { id: job.id.clone() },
    ] {
        run_with_store(cmd, &s).await.unwrap();
    }
    assert!(s.list().await.unwrap().is_empty());
}
