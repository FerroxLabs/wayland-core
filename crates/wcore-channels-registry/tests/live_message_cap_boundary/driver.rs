//! The live arms: the only code here that talks to a real destination.
//!
//! Split out of `live_message_cap_boundary.rs` on 2026-08-29 to keep both files
//! under the 1000-line module limit. The hermetic tests stayed in the root
//! module because they are what runs on every build; everything here is
//! `#[ignore]`d behind a real credential and a real conversation.
//!
//! Two shapes, dispatched by [`drive_cell`] on the cell's [`Boundary`]:
//!
//! * [`drive_boundary`] — the two-point character probe, at `cap` and `cap + 1`.
//!   Optionally in astral-plane characters, which is what settles whether the
//!   platform counts scalars or UTF-16 code units (wayland#934 c8).
//! * [`drive_saturating`] — one send of `cap` astral scalars, which spends a
//!   byte budget exactly, plus an ASCII control at `cap + 1` whose job is to be
//!   ACCEPTED. That is the arm a byte-budget cap needs and the two-point probe
//!   could never be (wayland#934 c7).

use std::sync::Arc;

use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::{Channel, ChannelConfig};
use wcore_channels_registry::channel_factory_for;
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};

use super::cells::{Above, Boundary, ByteBudget, Cell, Saturating, cell, derivation_faults};

/// One astral-plane character: U+1F600, four bytes in UTF-8 and a surrogate
/// pair — two code units — in UTF-16. The one body that tells a character limit
/// apart from a code-unit limit, and the one that saturates a byte budget.
const ASTRAL: char = '\u{1F600}';

/// Required env var, or a loud failure. Never a silent skip.
pub fn required(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. This test was invoked with --ignored, which means a live run was \
             explicitly requested; returning quietly would print a pass for zero work."
        ),
    }
}

/// Whether an optional flag is set to something truthy.
fn flag(var: &str) -> bool {
    matches!(std::env::var(var), Ok(v) if v.trim() == "1" || v.trim() == "true")
}

/// Build and start the adapter through the production construction chain.
///
/// This is `auto_register_from_dir`'s body with the manager left out: the same
/// `parse_channel_config`, the same [`channel_factory_for`], the same
/// `factory(..)`, the same on-disk `channels/<name>.toml` and
/// `credentials.toml` that `gateway run` reads. The manager is omitted for one
/// reason, stated at the top of this file: its send chunks, and a chunked send
/// measures our chunker rather than the platform.
async fn production_adapter(home: &str, name: &str, c: &Cell) -> Box<dyn Channel> {
    let path = std::path::Path::new(home)
        .join("channels")
        .join(format!("{name}.toml"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let cfg: ChannelConfig = wcore_channels::parse_channel_config(name, &body)
        .unwrap_or_else(|e| panic!("{} did not parse: {e}", path.display()));
    assert_eq!(
        cfg.platform,
        c.platform(),
        "{} declares platform {:?} but this cell probes {:?}; probing the wrong adapter would \
         record one platform's boundary against another's cap",
        path.display(),
        cfg.platform,
        c.platform()
    );
    // And the backend, for a selector that a config key picks. A `whatsapp`
    // config with no `backend` builds the Cloud API adapter, whose cap is a
    // different number from the bridge's — the platform assertion above cannot
    // see that.
    let configured_backend = cfg.options.get("backend").and_then(|v| v.as_str());
    assert_eq!(
        configured_backend,
        c.backend(),
        "{} selects backend {:?} but this cell probes {:?}",
        path.display(),
        configured_backend,
        c.backend()
    );
    let creds: Arc<dyn CredentialsStore> = Arc::new(PlaintextCredentialsStore::new(
        std::path::Path::new(home).join("credentials.toml"),
    ));
    let factory = channel_factory_for(&cfg.platform)
        .unwrap_or_else(|| panic!("registry has no factory for {:?}", cfg.platform));
    let mut ch = factory(cfg.name.clone(), &cfg.options, creds)
        .unwrap_or_else(|e| panic!("could not construct {:?}: {e}", cfg.platform));
    ch.start()
        .await
        .expect("start() must succeed — constructing does NOT authenticate");
    ch
}

/// Send one body and report what happened, deleting it where the adapter can.
async fn send_and_clean(
    ch: &mut Box<dyn Channel>,
    to: &str,
    body: String,
    label: &str,
) -> Result<String, String> {
    let scalars = body.chars().count();
    let utf8 = body.len();
    let utf16: usize = body.chars().map(char::len_utf16).sum();
    let sent = ch
        .send_message(OutgoingMessage::text(to.to_string(), body))
        .await;
    let outcome = match &sent {
        Ok(r) => {
            println!(
                "{label} scalars={scalars} utf8_bytes={utf8} utf16_units={utf16} accepted id={}",
                r.id
            );
            Ok(r.id.clone())
        }
        Err(e) => {
            println!("{label} scalars={scalars} utf8_bytes={utf8} utf16_units={utf16} REFUSED {e}");
            Err(e.to_string())
        }
    };
    // Clean up before the caller asserts, so a failing assertion does not
    // strand multi-thousand-character messages in a real destination.
    if let Ok(id) = &outcome {
        match ch.delete_message(to, id).await {
            Ok(()) => println!("LIVE_CAP_CLEAN  deleted {id}"),
            Err(e) => println!("LIVE_CAP_CLEAN  could NOT delete {id} — {e}"),
        }
    }
    outcome
}

/// The two-point character probe. Only valid for a character-shaped cap.
async fn drive_boundary(key: &str, to: &str, astral: bool) {
    let c = cell(key);
    let home = required(c.env[0]);
    let name = required(c.env[1]);
    let mut ch = production_adapter(&home, &name, c).await;

    let declared = ch.max_message_len().unwrap_or_else(|| {
        panic!("{key} declares no cap; there is no boundary to probe and chunking is off")
    });
    let at = c.probe_at(declared);
    println!(
        "LIVE_CAP_PROBE selector={key} shipped_cap={declared} probing_at={at} astral={astral}"
    );

    // ASCII: one char is one scalar is one byte is one UTF-16 code unit, so a
    // refusal is readable but the UNIT is not. Astral: one scalar is four UTF-8
    // bytes and TWO UTF-16 code units, which is what tells the units apart.
    let fill = if astral { ASTRAL } else { 'x' };
    let at_boundary: String = std::iter::repeat_n(fill, at).collect();
    let first = send_and_clean(&mut ch, to, at_boundary.clone(), "LIVE_CAP_AT  ").await;
    let over = format!("{at_boundary}{fill}");
    let second = send_and_clean(&mut ch, to, over, "LIVE_CAP_OVER").await;

    // The at-boundary send is the instrument. A measurement taken after it
    // failed is not a boundary result, it is no result — EXCEPT on the astral
    // arm, where a refusal at the cap IS the finding.
    if astral {
        println!(
            "LIVE_CAP_UNIT selector={key} astral_at_cap_accepted={} astral_over_accepted={} — a \
             REFUSAL at {at} astral scalars means the platform counts UTF-16 CODE UNITS and the \
             shipped cap of {declared} is unsafe for non-BMP text; record \
             CapUnit::MeasuredUtf16CodeUnits and lower the cap. An ACCEPT means it counts \
             scalars; record CapUnit::MeasuredScalars.",
            first.is_ok(),
            second.is_ok()
        );
        return;
    }

    let first = first.unwrap_or_else(|e| {
        panic!(
            "INSTRUMENT_FAULT: {key} refused a body of {at} chars — {e}. Nothing below this is a \
             boundary measurement: grade the run INCOMPLETE, not a narrower boundary. A dead \
             credential and a moved boundary produce the same red here, and only the platform's \
             own diagnostic tells them apart."
        )
    });
    assert!(
        !first.is_empty(),
        "INSTRUMENT_FAULT: the platform returned an empty message id, so nothing can be \
         corroborated at its console"
    );

    match c.boundary {
        Boundary::Measured {
            above: Above::Refused(evidence),
            ..
        } => {
            let err = second.err().unwrap_or_else(|| {
                panic!(
                    "{key} ACCEPTED {} chars. It refused that on the recorded run ({evidence}), \
                     so the boundary has MOVED — re-measure it, update the cell, and only then \
                     consider raising the shipped cap.",
                    at + 1
                )
            });
            println!("LIVE_CAP_VERDICT selector={key} boundary={at} above=refused ({err})");
        }
        Boundary::Measured {
            above: Above::SilentlyReshaped(evidence),
            ..
        } => {
            // There is no error to assert on: the platform takes the body and
            // reshapes it. Reported, not asserted, and the operator corroborates
            // at the platform's own console — an id is our read of their
            // response, whereas the claim is about what a human sees in the
            // channel, and those are the two claims this repository has already
            // conflated once.
            println!(
                "LIVE_CAP_VERDICT selector={key} boundary={at} above=silently-reshaped \
                 ({evidence}) — CORROBORATE AT THE PLATFORM: count the messages that arrived for \
                 the over-boundary send before believing this row."
            );
        }
        Boundary::Measured {
            above: Above::AcceptedNormally(evidence),
            ..
        } => panic!(
            "{key} records AcceptedNormally at cap + 1 under a character shape ({evidence}). \
             That is not a boundary: the platform took both arms, so the boundary is somewhere \
             above and this cell records a number nobody has bracketed."
        ),
        Boundary::NotMeasured { .. } => {
            println!(
                "LIVE_CAP_VERDICT selector={key} DISCOVERY at shipped cap {at}: \
                 over_boundary_accepted={}. This cell is still NotMeasured — record the numbers \
                 above in docs/delivery-semantics.md §4.2 and convert the cell in the same \
                 commit.",
                second.is_ok()
            );
        }
        Boundary::Derived(_) => unreachable!("dispatched to drive_saturating"),
    }
}

/// The SATURATING arm — the one that can decide a byte-budget cap.
///
/// One point, not two: a body of `cap` scalars in the worst-case encoding,
/// which is the largest body the derivation claims is safe. If the platform
/// takes it, the derivation holds at its worst case. If it refuses, the cap is
/// above the real boundary and the refusal names the budget.
///
/// An ASCII control at `cap + 1` runs beside it, and its job is to be ACCEPTED:
/// that is the observation that the two-point probe cannot decide this shape,
/// recorded in the run rather than argued in a comment.
async fn drive_saturating(key: &str, to: &str, b: ByteBudget) {
    let c = cell(key);
    let home = required(c.env[0]);
    let name = required(c.env[1]);
    let mut ch = production_adapter(&home, &name, c).await;

    let cap = ch
        .max_message_len()
        .unwrap_or_else(|| panic!("{key} declares no cap; there is no derivation to check"));
    let faults = derivation_faults(cap, &b);
    assert!(
        faults.is_empty(),
        "{key}: refusing to drive a live arm against a cap that already disagrees with its own \
         budget — fix the arithmetic first:\n  {}",
        faults.join("\n  ")
    );
    println!(
        "LIVE_CAP_PROBE selector={key} shape=byte-budget shipped_cap={cap} budget_bytes={} \
         worst_case_bytes_per_scalar={} unmodelled={}",
        b.budget_bytes, b.worst_case_bytes_per_scalar, b.unmodelled
    );

    let saturating: String = std::iter::repeat_n(ASTRAL, cap).collect();
    let sat = send_and_clean(&mut ch, to, saturating, "LIVE_CAP_SAT ").await;

    let control: String = std::iter::repeat_n('x', cap + 1).collect();
    let ctl = send_and_clean(&mut ch, to, control, "LIVE_CAP_CTL ").await;
    if ctl.is_err() {
        println!(
            "LIVE_CAP_NOTE the ASCII control at cap + 1 was REFUSED. This cell is recorded as a \
             byte budget on the strength of the control being accepted; a refusal means there IS \
             a character boundary here and the cell shape is wrong."
        );
    }

    let observed = Saturating::Driven {
        accepted: sat.is_ok(),
        on: "this run",
        evidence: "see LIVE_CAP_SAT above",
    };
    match (b.saturating, observed) {
        (Saturating::NotDriven { .. }, Saturating::Driven { accepted, .. }) => println!(
            "LIVE_CAP_VERDICT selector={key} DISCOVERY saturating_at={cap}_astral_scalars \
             accepted={accepted} ascii_control_at_cap_plus_1_accepted={}. Record it in \
             docs/delivery-semantics.md §4.2 and convert the cell in the same commit — and note \
             that an ACCEPT is an upper bound, not the boundary, because of: {}",
            ctl.is_ok(),
            b.unmodelled
        ),
        (
            Saturating::Driven {
                accepted: recorded,
                on,
                evidence,
            },
            Saturating::Driven { accepted: now, .. },
        ) => {
            assert_eq!(
                now, recorded,
                "{key}: the saturating arm was {recorded} on {on} ({evidence}) and is {now} now. \
                 The budget or the derivation has moved; re-derive before changing the cap."
            );
            println!(
                "LIVE_CAP_VERDICT selector={key} saturating_at={cap}_astral_scalars \
                 accepted={now} (re-derived; recorded {on})"
            );
        }
        (_, Saturating::NotDriven { .. }) => {
            unreachable!("the observed arm is always Driven")
        }
    }
}

/// Dispatch on the cell's shape. A byte-budget cell must never reach the
/// two-point probe.
pub async fn drive_cell(key: &str, to: &str) {
    let astral_var = format!(
        "WL_LIVE_CAP_{}_ASTRAL",
        key.to_uppercase().replace(['+', '-'], "_")
    );
    match cell(key).boundary {
        Boundary::Derived(b) => drive_saturating(key, to, b).await,
        _ => drive_boundary(key, to, flag(&astral_var)).await,
    }
}
