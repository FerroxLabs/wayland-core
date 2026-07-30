//! Does a Twilio / WhatsApp arrival now carry a delivery identity — measured at
//! the journey's OWN sink, graded by the journey's OWN classifier.
//!
//! # The claim, and why nothing smaller establishes it
//!
//! `lane/twilio-whatsapp-identity` made both adapters transmit the gateway's
//! delivery id. The value of that change is a claim about a NUMBER the journey
//! gate produces — `indeterminate`, which was **8 of 12** repeats on the Windows
//! run because `twilio.messages` and `whatsapp.messages` arrivals carried no
//! identity and a repeat was therefore unjudgeable *in principle*.
//!
//! A claim about a gate's output is only settled by running that gate. The unit
//! tests inside each adapter crate prove a header and a JSON field leave the
//! process; they say nothing about what the sink journals or what the classifier
//! then makes of it. Between those two facts sat a **second, independent
//! defect**: `scripts/f24-sink.mjs` hardcoded `idempotency_key: null` on both
//! endpoints, so an adapter fix alone would have moved the number by zero while
//! every unit test went green.
//!
//! So this test drives the real chain end to end:
//!
//! ```text
//! production factory -> real adapter -> real HTTP -> f24-sink.mjs (own process)
//!   -> arrivals journal -> classifyRepeats() imported from f24-journey.mjs
//! ```
//!
//! Nothing on that path is reimplemented for the test. The classifier in
//! particular is *imported*, not copied — grading a copy would grade the copy.
//!
//! # Both directions, by construction
//!
//! A gate that cannot fail proves as little as one that cannot pass
//! (LANE-BRIEF §3.2, §3b-iii). Three arms run against the same binary, the same
//! sink and the same adapters, differing only in what identity the send carries:
//!
//! | arm | what it sends | expected | what it proves |
//! |---|---|---|---|
//! | `A-UNKEYED` | one body twice, no key | `NOT-PROVEN` | the gate CAN fail, and this reproduces the pre-change state on the post-change binary — so the improvement is not the harness being kinder |
//! | `B-KEYED` | one body twice, two distinct keys | `RECURRENCE` | the gate CAN pass. For these two adapters this state was previously unreachable |
//! | `C-REPLAY` | one body twice, ONE key | `EXACTLY-ONCE-VIOLATED` | identity did not make the gate blind — the real violation is still caught |
//!
//! Arm C is the one that matters for trust. The cheap way for this change to go
//! wrong is for every repeat to start classifying as a benign recurrence, which
//! would look like a large improvement and be a regression.
//!
//! # Why it is `#[ignore]`d, and why that is not a hidden skip
//!
//! It shells out to `node`, which is not a build dependency of this workspace. A
//! test that hard-failed without node would redden every machine that has none;
//! one that silently skipped would be the `live_fs_acl` shape — exit 0, `test
//! result: ok`, zero tests run. So it is `#[ignore]`d and named, and the
//! transcript of a real run is committed under
//! `.planning/evidence/twilio-whatsapp-identity/`.
//!
//! ```text
//! cargo test -p wcore-channels-registry --test identity_at_the_sink \
//!   -- --ignored --nocapture
//! ```
//!
//! Inside the run there is no silent skip anywhere: a missing `node`, a sink
//! that never prints `SINK_READY`, an empty journal, or a classifier that grades
//! zero arms are each a loud failure.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use wcore_channels::{Channel, OutgoingMessage};
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// Repo root, from this crate's manifest dir. Used to find `scripts/`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> must have two ancestors")
        .to_path_buf()
}

/// An in-memory credentials store. The values are synthetic: the sink
/// fingerprints the `Authorization` header and never journals it in the clear,
/// and no real platform is contacted.
#[derive(Debug)]
struct MapStore(BTreeMap<String, String>);

impl CredentialsStore for MapStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.get(key).cloned())
    }
    fn put(&self, _key: &str, _value: &str) -> Result<(), CredentialsError> {
        unreachable!("this test never writes a credential")
    }
    fn delete(&self, _key: &str) -> Result<(), CredentialsError> {
        unreachable!("this test never deletes a credential")
    }
}

fn creds() -> Arc<dyn CredentialsStore> {
    Arc::new(MapStore(BTreeMap::from([
        ("fx.sms.account_sid".into(), "ACfixture0000000000".into()),
        ("fx.sms.auth_token".into(), "fixture-auth-token".into()),
        ("fx.wa.access_token".into(), "EAAfixture-token".into()),
        ("fx.wa.app_secret".into(), "fixture-app-secret".into()),
    ])))
}

/// The sink, as its own OS process — the same script the journey uses.
struct Sink {
    child: Child,
    url: String,
    journal: PathBuf,
}

impl Sink {
    fn start(journal: PathBuf) -> Self {
        let script = repo_root().join("scripts").join("f24-sink.mjs");
        assert!(
            script.is_file(),
            "the sink script is missing at {}; this test measures the REAL sink and will not \
             substitute a stand-in",
            script.display()
        );
        let mut child = Command::new("node")
            .arg(&script)
            .arg("--port")
            .arg("0")
            .arg("--journal")
            .arg(&journal)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect(
                "could not spawn `node`. This test grades the journey's own classifier, which \
                 is JavaScript, so node is required. It is `#[ignore]`d precisely so a machine \
                 without node is not reddened by default — a run that reaches here asked for it.",
            );

        // Read the bound URL back rather than assuming a port. A gateway pointed
        // at an unbound port fails its sends in a way that looks like a product
        // defect.
        let stdout = child.stdout.take().expect("piped");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the sink must print SINK_READY before anything is sent to it");
        let url = line
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("url="))
            .unwrap_or_else(|| {
                panic!("sink did not announce a url; it printed {line:?}");
            })
            .to_string();
        Self {
            child,
            url,
            journal,
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build one adapter through the PRODUCTION factory — the same dispatch
/// `auto_register_from_dir` uses — pointed at the sink.
fn adapter(platform: &str, sink_url: &str) -> Box<dyn Channel> {
    let toml_body = match platform {
        "sms" => format!(
            "from_number = \"+15550000000\"\n\
             credential_handle_account_sid = \"fx.sms.account_sid\"\n\
             credential_handle_auth_token = \"fx.sms.auth_token\"\n\
             api_base_url = \"{sink_url}\"\n"
        ),
        "whatsapp" => format!(
            "workspace_name = \"fixture\"\n\
             phone_number_id = \"10000000000\"\n\
             credential_handle_access_token = \"fx.wa.access_token\"\n\
             credential_handle_app_secret = \"fx.wa.app_secret\"\n\
             api_base_url = \"{sink_url}\"\n"
        ),
        other => panic!("this test drives sms and whatsapp only, not {other:?}"),
    };
    let options: toml::Table = toml::from_str(&toml_body).expect("fixture config must parse");
    let factory = wcore_channels_registry::channel_factory_for(platform)
        .unwrap_or_else(|| panic!("the production registry has no factory for {platform:?}"));
    factory(format!("fx-{platform}"), &options, creds())
        .unwrap_or_else(|e| panic!("could not construct {platform:?}: {e}"))
}

/// Recipient per platform. Twilio rejects an empty `To` outright, and WhatsApp
/// needs one too, so both are addressed explicitly rather than relying on a
/// default-destination fallback that differs between them.
const TO: &str = "+15551234567";

#[tokio::test]
#[ignore = "shells out to node to run the journey's own classifier"]
async fn twilio_and_whatsapp_arrivals_now_carry_a_delivery_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = dir.path().join("arrivals.jsonl");
    let sink = Sink::start(journal.clone());
    println!("SINK url={} journal={}", sink.url, journal.display());

    for platform in ["sms", "whatsapp"] {
        let mut ch = adapter(platform, &sink.url);
        ch.start().await.expect("adapter must start");

        // ---- arm A: the PRE-CHANGE state, reproduced on the post-change
        // binary. Two arrivals of one body, neither carrying an identity.
        for _ in 0..2 {
            ch.send_message(OutgoingMessage::text(TO, format!("A-UNKEYED|{platform}")))
                .await
                .expect("unkeyed send must reach the sink");
        }

        // ---- arm B: two DISTINCT delivery ids. The recurring trigger firing
        // again, which is the ordinary and correct case.
        for occurrence in 0..2u64 {
            ch.send_message_idempotent(
                OutgoingMessage::text(TO, format!("B-KEYED|{platform}")),
                &format!(
                    "cron:fx-{platform}:{}",
                    1_785_000_000_000u64 + occurrence * 60_000
                ),
            )
            .await
            .expect("keyed send must reach the sink");
        }

        // ---- arm C: ONE delivery id, twice. A genuine exactly-once violation,
        // which must still be caught now that repeats are classifiable at all.
        let replay_key = format!("cron:fx-{platform}-replay:1785000000000");
        for _ in 0..2 {
            ch.send_message_idempotent(
                OutgoingMessage::text(TO, format!("C-REPLAY|{platform}")),
                &replay_key,
            )
            .await
            .expect("replayed send must reach the sink");
        }
    }

    // The sink fsyncs each arrival before answering, and every send above was
    // awaited to completion, so the journal is complete at this point without a
    // sleep. Stopping the process first would race that guarantee.
    let arms = grade(&journal);

    // ---- the assertions, one arm at a time. Aggregating them would let a
    // failure in one direction be paid for by a success in the other.
    for endpoint in ["twilio.messages", "whatsapp.messages"] {
        let a = tally(&arms, "A-UNKEYED", endpoint);
        assert_eq!(a["arrived"], 2, "{endpoint} arm A must have two arrivals");
        assert_eq!(
            a["unidentified"], 2,
            "{endpoint} arm A: an unkeyed send must still journal NO identity. If this is 0 the \
             adapter has started attaching an id unconditionally, which would mark every \
             unkeyed arrival as identified — a silent false-clean, because a receipt full of \
             identified arrivals is what a healthy run looks like."
        );
        assert_eq!(
            a["indeterminate"], 1,
            "{endpoint} arm A: the repeat must be UNJUDGEABLE. This is the gate proving it can \
             still fail; if it passes here the improvement below is the harness being kinder, \
             not the product being better."
        );
        assert_eq!(a["verdict"], "NOT-PROVEN");

        let b = tally(&arms, "B-KEYED", endpoint);
        assert_eq!(b["arrived"], 2, "{endpoint} arm B must have two arrivals");
        assert_eq!(
            b["unidentified"], 0,
            "{endpoint} arm B: THE MEASUREMENT. Every keyed arrival must carry an identity. A \
             non-zero here means either the adapter did not transmit it or the sink did not \
             read it — the two independent causes of the NOT-PROVEN verdict, and this \
             assertion cannot tell them apart, only that neither is present."
        );
        assert_eq!(
            b["indeterminate"], 0,
            "{endpoint} arm B: with identities present nothing may remain unjudgeable"
        );
        assert_eq!(
            b["recurrences"], 1,
            "{endpoint} arm B: the repeat must be classified as a recurrence — the trigger \
             firing again under a new (job, scheduled instant) pair"
        );
        assert_eq!(b["replays"], 0, "{endpoint} arm B carries no replay");
        assert_eq!(b["verdict"], "RECURRENCE");

        let c = tally(&arms, "C-REPLAY", endpoint);
        assert_eq!(c["arrived"], 2, "{endpoint} arm C must have two arrivals");
        assert_eq!(
            c["replays"], 1,
            "{endpoint} arm C: a genuine exactly-once violation must STILL be caught. Making \
             repeats classifiable is worthless — worse than worthless — if everything now \
             classifies as a benign recurrence."
        );
        assert_eq!(c["verdict"], "EXACTLY-ONCE-VIOLATED");
    }
}

/// Run the journey's own classifier over the journal, arm by arm.
fn grade(journal: &Path) -> serde_json::Value {
    let script = repo_root().join("scripts").join("f24-identity-arms.mjs");
    let out = Command::new("node")
        .arg(&script)
        .arg("--journal")
        .arg(journal)
        .stderr(Stdio::inherit())
        .output()
        .expect("could not run the arm grader");
    assert!(
        out.status.success(),
        "the arm grader exited {:?}. It refuses an empty journal on purpose, so this is a real \
         failure rather than a quiet zero.",
        out.status.code()
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the grader must emit parsable JSON");
    let armc = parsed.as_object().map_or(0, serde_json::Map::len);
    assert_eq!(
        armc, 3,
        "expected exactly 3 arms, got {armc}. A grader that saw fewer arms than were sent has \
         lost arrivals, and every count below it would be drawn from a run that did not happen."
    );
    parsed
}

/// One arm/endpoint tally as a string map, so an assertion names the field it
/// is reading rather than indexing a tuple.
fn tally(arms: &serde_json::Value, arm: &str, endpoint: &str) -> BTreeMap<String, String> {
    let node = arms
        .get(arm)
        .and_then(|a| a.get(endpoint))
        .unwrap_or_else(|| panic!("no tally for arm {arm} endpoint {endpoint}"));
    node.as_object()
        .expect("a tally is an object")
        .iter()
        .map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}
