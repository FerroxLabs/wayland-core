//! Compatibility evidence for seam request F21-04-01 (per-child attribution).
//!
//! The question this answers: can `ProtocolCommand::Stop` gain a child field
//! *additively*, or does converting it from a unit variant to a struct variant
//! break the existing wire form and force a separate `StopChild` command?
//!
//! The shapes below mirror `wcore-protocol/src/commands.rs` exactly on the two
//! attributes that decide the answer — `#[serde(tag = "type")]` (internally
//! tagged) and `rename_all = "snake_case"` — and `ProtocolCommand` derives only
//! `Deserialize`, so decoding is the only direction that exists for commands.
//!
//! Cases 1-3 are asserted: this probe fails loudly rather than printing a wrong
//! answer. Case 4 is only reported, because its outcome IS the finding rather
//! than a requirement.
//!
//! Expected final line: `PROBE_DONE`.

use serde::Deserialize;

/// Shape BEFORE: `Stop` is a unit variant, as it is today at `commands.rs:142`.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum Before {
    Stop,
    Other { x: u8 },
}

/// Shape AFTER: `Stop` gains one optional, defaulted child field.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum After {
    Stop {
        #[serde(default)]
        child: Option<String>,
    },
    Other { x: u8 },
}

fn main() {
    let legacy = r#"{"type":"stop"}"#;
    let scoped = r#"{"type":"stop","child":"c-1"}"#;

    // 1. Baseline: the legacy wire form decodes on the old shape.
    assert_eq!(
        serde_json::from_str::<Before>(legacy).unwrap(),
        Before::Stop
    );

    // 2. THE CLAIM: the legacy wire form still decodes after the change.
    //    If this ever fails, the seam request's "additive" premise is void and
    //    a separate StopChild command is required instead.
    let a: After = serde_json::from_str(legacy).expect("legacy stop must decode post-change");
    assert_eq!(a, After::Stop { child: None });
    println!("legacy-stop-on-new-shape  = OK -> {a:?}");

    // 3. The new scoped form decodes and carries the child through.
    let b: After = serde_json::from_str(scoped).unwrap();
    assert_eq!(
        b,
        After::Stop {
            child: Some("c-1".into())
        }
    );
    println!("scoped-stop-on-new-shape  = OK -> {b:?}");

    // 4. Version skew, host NEWER than Core — the direction that actually bites.
    //    An old Core decoder receiving a scoped stop from an upgraded host.
    //    Reported, not asserted: whichever way this lands is the finding.
    match serde_json::from_str::<Before>(scoped) {
        Ok(v) => println!("old-decoder-on-scoped     = ACCEPTS, ignores unknown field -> {v:?}"),
        Err(e) => println!("old-decoder-on-scoped     = REJECTS -> {e}"),
    }

    println!("PROBE_DONE");
}
