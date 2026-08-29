//! The probe cells, the shapes a cap can have, and the pure checkers that
//! decide each shape.
//!
//! Split out of `live_message_cap_boundary.rs` on 2026-08-29 when the file
//! outgrew the 1000-line module limit. Everything here is data plus total
//! functions over it: no network, no adapter construction, no environment. The
//! live driver lives next door and reads this.
//!
//! # A cap has a SHAPE, and the shape decides what a probe can settle
//!
//! Until this split, every cell was the same shape: a character count with a
//! two-point probe at `cap` and `cap + 1`. That shape is right for five of the
//! nine implementations and **cannot decide the other two**, which is
//! [wayland#934](https://github.com/FerroxLabs/wayland/issues/934) c7.
//!
//! * **Character caps** — Slack, Discord, Telegram, Twilio SMS, the WhatsApp
//!   Cloud API, the two bridge backends. The platform counts something
//!   proportional to the body, so a send at `cap` and a send at `cap + 1`
//!   bracket the boundary between them.
//! * **Byte-budget caps** — Matrix and MS Teams. The platform's limit is on a
//!   PAYLOAD in bytes (Matrix: a 65,536-byte Canonical-JSON PDU; Teams: an
//!   80 KB UTF-16 Activity) and the shipped number is a DERIVED scalar count,
//!   `budget / worst-case bytes per scalar`. An ASCII body of `cap` characters
//!   is a quarter of the budget and `cap + 1` is a quarter plus one byte, so
//!   both arms land deep inside the accepted region and both come back
//!   [`Above::AcceptedNormally`]. The probe learns nothing. A credential would
//!   not have changed that; the SHAPE was wrong.
//!
//! What decides a byte-budget cap is [`derivation_faults`] hermetically — the
//! arithmetic that both historical mistakes violated — plus a single
//! SATURATING live arm: `cap` scalars in the worst-case encoding, which is the
//! largest body the derivation claims is safe. That is one point, not two, and
//! it tests the claim the number actually makes.
//!
//! # And a character cap has a UNIT
//!
//! [wayland#934](https://github.com/FerroxLabs/wayland/issues/934) c8. Telegram
//! was driven at 4,096/4,097 in ASCII, where one character is one UTF-16 code
//! unit, so the run cannot tell a 4,096-CHARACTER limit from a 4,096-CODE-UNIT
//! one. Those differ by a factor of two for astral-plane text, and in the
//! dangerous direction: if the limit is code units, a 4,096-scalar emoji reply
//! is 8,192 code units and the platform refuses it — HIGH-6, at the cap we
//! ship. [`CapUnit`] records which of the two a cell has settled, and
//! [`unit_safety_faults`] refuses a cap that a settled UTF-16 verdict makes
//! unsafe.

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// What the platform did to a body one character above the recorded boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Above {
    /// The platform returned an error the adapter surfaces as a
    /// `ChannelError`. The string is the platform's own diagnostic, recorded so
    /// a future run can tell "the boundary moved" apart from "the credential
    /// broke".
    Refused(&'static str),
    /// The platform accepted the request and did NOT deliver one message — it
    /// reshaped the body. There is no error to catch, which is precisely why
    /// the shipped cap sits below this number.
    SilentlyReshaped(&'static str),
    /// The platform accepted it as one ordinary message and said nothing.
    ///
    /// **The variant the two-point probe was missing.** For a CHARACTER cap
    /// this means the boundary is higher than recorded and the cell is stale.
    /// For a BYTE-BUDGET cap it is the EXPECTED outcome of an ASCII arm and
    /// settles nothing at all: at four bytes per scalar worst case, `cap` and
    /// `cap + 1` ASCII characters spend a QUARTER of the budget, so neither arm
    /// is anywhere near the boundary. Recording that outcome is how the
    /// undecidability becomes an observation instead of an argument.
    AcceptedNormally(&'static str),
}

/// Which unit a platform counts a single message in.
///
/// ASCII cannot distinguish characters from UTF-16 code units, because in ASCII
/// they are the same number. Every cap in the product was probed in ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapUnit {
    /// The platform's own diagnostic names the unit, so no encoding probe is
    /// needed. Twilio's `21617` says "the 1600 character limit" in words.
    StatedByThePlatform(&'static str),
    /// Driven with astral-plane characters and the boundary held at the same
    /// SCALAR count, so the platform counts scalars.
    MeasuredScalars {
        /// ISO date of the run.
        on: &'static str,
        /// What the run saw, in the platform's own terms.
        evidence: &'static str,
    },
    /// Driven with astral-plane characters and the boundary HALVED, so the
    /// platform counts UTF-16 code units.
    ///
    /// `limit_code_units` is the limit in the platform's unit, which is what
    /// [`unit_safety_faults`] divides to get the largest scalar count that is
    /// safe for any text.
    MeasuredUtf16CodeUnits {
        limit_code_units: usize,
        on: &'static str,
        evidence: &'static str,
    },
    /// Probed in ASCII only. The cap is confirmed for ASCII text and the unit
    /// question is OPEN — say so rather than letting an ASCII run read as a
    /// settled one.
    UnsettledAsciiOnly {
        /// What a run would need to settle it.
        needs: &'static str,
    },
}

impl CapUnit {
    /// Whether an astral-plane run is still owed.
    pub fn is_unsettled(&self) -> bool {
        matches!(self, Self::UnsettledAsciiOnly { .. })
    }
}

/// Whether the saturating (worst-case encoding) arm of a byte-budget cap has
/// ever been driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saturating {
    /// Driven at the real destination. `accepted` is what the platform did with
    /// a body of `cap` scalars in the worst-case encoding — the largest body
    /// the derivation claims is safe.
    Driven {
        accepted: bool,
        on: &'static str,
        evidence: &'static str,
    },
    /// Never driven. `waiting_on` names what a run needs.
    NotDriven { waiting_on: &'static str },
}

/// A cap derived from a payload budget rather than a character limit.
#[derive(Debug, Clone, Copy)]
pub struct ByteBudget {
    /// The budget the vendor documents, in bytes.
    pub budget_bytes: usize,
    /// Worst-case bytes one Unicode scalar costs in the encoding the budget is
    /// stated in. Four, for both of today's cells: UTF-8 uses up to four bytes
    /// per scalar, and a UTF-16 surrogate pair is two code units of two bytes.
    pub worst_case_bytes_per_scalar: usize,
    /// What the budget covers BEYOND the body, and therefore why even a
    /// saturating arm is an upper bound rather than the exact boundary.
    pub unmodelled: &'static str,
    /// What an ASCII arm at `cap + 1` does. This is the field that records why
    /// the two-point probe cannot decide this shape.
    pub ascii_two_point: Above,
    /// The arm that CAN decide it.
    pub saturating: Saturating,
}

/// One implementation's boundary, as measured at the platform.
#[derive(Debug, Clone, Copy)]
pub enum Boundary {
    /// A character cap, driven at the real destination on `on` (ISO date).
    ///
    /// `accepts_up_to` is NOT a constant this repository returns from anywhere.
    /// It is an observation of a third party, recorded here so the shipped
    /// constant has something outside itself to be checked against, and
    /// re-derived by the live cell every time one runs.
    Measured {
        accepts_up_to: usize,
        above: Above,
        unit: CapUnit,
        on: &'static str,
    },
    /// A character cap nobody has driven. `waiting_on` names the credential,
    /// not an excuse.
    NotMeasured { waiting_on: &'static str },
    /// A cap derived from a payload budget. The two-point character probe
    /// cannot decide this shape; see the module docs.
    Derived(ByteBudget),
}

/// One implementation's probe cell.
pub struct Cell {
    /// The selector key, as `ChannelSelector::key` renders it and as
    /// `docs/delivery-semantics.md` names the row: the bare platform tag for a
    /// platform's default implementation, `platform+<backend>` for one a config
    /// key selects.
    pub key: &'static str,
    pub boundary: Boundary,
    /// The three variables the live cell requires, in the order the panic names
    /// them.
    pub env: [&'static str; 3],
}

impl Cell {
    /// The platform tag `channel_factory_for` answers to.
    pub fn platform(&self) -> &'static str {
        match self.key.split_once('+') {
            Some((p, _)) => p,
            None => self.key,
        }
    }

    /// The config value that selects this implementation inside its platform,
    /// or `None` for the platform's default one.
    pub fn backend(&self) -> Option<&'static str> {
        self.key.split_once('+').map(|(_, b)| b)
    }

    /// The length the live probe sends as its "accepted" arm.
    ///
    /// For a measured platform that is the recorded boundary, so a re-run
    /// re-derives the same fact. For an unmeasured one there is no boundary
    /// yet, so the probe drives the DECLARED cap and reports what happened —
    /// which is the discovery run that turns `NotMeasured` into `Measured`.
    /// A byte-budget cell always drives the declared cap: its saturating arm IS
    /// the claim, not a search for a boundary.
    pub fn probe_at(&self, declared_cap: usize) -> usize {
        match self.boundary {
            Boundary::Measured { accepts_up_to, .. } => accepts_up_to,
            Boundary::NotMeasured { .. } | Boundary::Derived(_) => declared_cap,
        }
    }
}

/// Every implementation that declares a finite `max_message_len()`.
///
/// The set is checked against the registry by
/// `every_capped_adapter_has_a_probe_cell`, which walks
/// `wcore_channels_registry::constructible_selectors()` — the enumeration of
/// what the registry can BUILD, not of platform strings. That widening is
/// wayland-core#360 c2, and it is why the last two rows exist at all: the
/// bridge is reached through `whatsapp` plus a `backend` key, so a
/// platform-keyed guard structurally could not see it.
pub const CELLS: &[Cell] = &[
    Cell {
        key: "slack",
        boundary: Boundary::Measured {
            // 4,040 is the largest body that stayed one message. The shipped
            // cap is 4,000 — Slack's own documented "for best results" figure
            // and the point its splitter uses — which is BELOW the boundary and
            // therefore safe.
            accepts_up_to: 4_040,
            above: Above::SilentlyReshaped(
                "4,041 is accepted and split by the API into 4,000-char messages; no error",
            ),
            unit: CapUnit::UnsettledAsciiOnly {
                needs: "an astral-plane run. Slack documents no unit and the 2026-08-27 probe was \
                        ASCII. The shipped 4,000 is 40 below the ASCII boundary, so a UTF-16 \
                        verdict would still leave it unsafe for astral text and the run is owed",
            },
            on: "2026-08-27",
        },
        env: [
            "WL_LIVE_CAP_SLACK_HOME",
            "WL_LIVE_CAP_SLACK_CHANNEL",
            "WL_LIVE_CAP_SLACK_TO",
        ],
    },
    Cell {
        key: "discord",
        boundary: Boundary::Measured {
            accepts_up_to: 2_000,
            above: Above::Refused("HTTP 400, code 50035 Invalid Form Body"),
            unit: CapUnit::UnsettledAsciiOnly {
                needs: "an astral-plane run. Discord's own documentation says \"up to 2000 \
                        characters\" and the 2026-08-27 probe was ASCII, so the word \
                        \"characters\" is the vendor's, not a measurement",
            },
            on: "2026-08-27",
        },
        env: [
            "WL_LIVE_CAP_DISCORD_HOME",
            "WL_LIVE_CAP_DISCORD_CHANNEL",
            "WL_LIVE_CAP_DISCORD_TO",
        ],
    },
    Cell {
        key: "matrix",
        boundary: Boundary::Derived(ByteBudget {
            // Client-Server API, Size limits: "The complete event MUST NOT be
            // larger than 65536 bytes ... encoded as Canonical JSON." Synapse
            // enforces exactly that (MAX_PDU_SIZE = 65536).
            budget_bytes: 65_536,
            worst_case_bytes_per_scalar: 4,
            unmodelled: "the complete signed PDU the homeserver assembles after the PUT — event \
                         ids, sender, room id, hashes, signatures and any formatted_body — none \
                         of which the client can size. So even an accepted saturating arm is an \
                         UPPER BOUND on the body, not the boundary",
            ascii_two_point: Above::AcceptedNormally(
                "16,384 and 16,385 ASCII characters are 16,384 and 16,385 bytes against a \
                 65,536-byte budget. Both are a quarter of the way in, both are accepted, and \
                 neither says anything about the boundary",
            ),
            saturating: Saturating::NotDriven {
                waiting_on: "a live homeserver access token. The one this programme held was \
                             found dead on 2026-07-31 (M_UNKNOWN_TOKEN) and has not been \
                             reissued. The arm to drive is 16,384 astral-plane scalars — 65,536 \
                             UTF-8 bytes, the budget exactly — NOT the two-point ASCII probe",
            },
        }),
        env: [
            "WL_LIVE_CAP_MATRIX_HOME",
            "WL_LIVE_CAP_MATRIX_CHANNEL",
            "WL_LIVE_CAP_MATRIX_TO",
        ],
    },
    Cell {
        key: "telegram",
        boundary: Boundary::Measured {
            // Driven at a real bot and a real group. 4,096 was accepted as one
            // message; 4,097 was refused outright, so the shipped cap sits
            // exactly ON the boundary rather than below it — unlike Slack,
            // where the boundary is 4,040 against a 4,000 cap.
            accepts_up_to: 4_096,
            above: Above::Refused("400: Bad Request: message is too long"),
            unit: CapUnit::UnsettledAsciiOnly {
                needs: "an astral-plane run, and this is the one that MATTERS. Telegram's \
                        sendMessage says \"1-4096 characters after entities parsing\" while \
                        MessageEntity on the same page indexes in UTF-16 code units. If the \
                        limit is code units then a 4,096-scalar astral body is 8,192 code units \
                        and the platform refuses it AT THE CAP WE SHIP — send_to_keyed would \
                        hand it over whole and nothing re-sends it. Drive \
                        live_boundary_at_real_telegram with WL_LIVE_CAP_TELEGRAM_ASTRAL=1",
            },
            on: "2026-08-29",
        },
        env: [
            "WL_LIVE_CAP_TELEGRAM_HOME",
            "WL_LIVE_CAP_TELEGRAM_CHANNEL",
            "WL_LIVE_CAP_TELEGRAM_TO",
        ],
    },
    Cell {
        key: "sms",
        boundary: Boundary::Measured {
            // Driven at a real Twilio account to a real handset. 1,600 was
            // accepted as one concatenated message; 1,601 was refused by Twilio
            // BEFORE reaching a carrier, so the over-arm cost nothing.
            accepts_up_to: 1_600,
            above: Above::Refused(
                "HTTP 400 code 21617, The concatenated message body exceeds the 1600 character limit",
            ),
            unit: CapUnit::StatedByThePlatform(
                "Twilio's own error names the unit: \"The concatenated message body exceeds the \
                 1600 character limit\" (21617). It is a CHARACTER limit and it does not move \
                 with the segment encoding — GSM-7 packs 153 per segment and UCS-2 packs 67, but \
                 the 1,600 ceiling is the same. No astral run is owed here",
            ),
            on: "2026-08-29",
        },
        env: [
            "WL_LIVE_CAP_SMS_HOME",
            "WL_LIVE_CAP_SMS_CHANNEL",
            "WL_LIVE_CAP_SMS_TO",
        ],
    },
    Cell {
        key: "whatsapp",
        boundary: Boundary::NotMeasured {
            waiting_on: "a Meta Business app with the WhatsApp product, a phone-number id, a \
                         system-user access token, and a recipient inside the 24-hour \
                         customer-service window. No Meta credential exists on this programme",
        },
        env: [
            "WL_LIVE_CAP_WHATSAPP_HOME",
            "WL_LIVE_CAP_WHATSAPP_CHANNEL",
            "WL_LIVE_CAP_WHATSAPP_TO",
        ],
    },
    Cell {
        key: "msteams",
        boundary: Boundary::Derived(ByteBudget {
            // "the size of the message itself is within 80 KB" — Microsoft's
            // own recommendation against a 100 KB hard limit that returns 413
            // MessageSizeTooBig. No character limit is documented at all.
            budget_bytes: 81_920,
            worst_case_bytes_per_scalar: 4,
            unmodelled: "the serialized Activity: @-mentions, attachment JSON, adaptive-card \
                         payloads and the conversation envelope, none of which the client sizes. \
                         And 80 KB is Microsoft's RECOMMENDATION inside a 100 KB hard limit, so \
                         the derivation is deliberately below the enforced boundary",
            ascii_two_point: Above::AcceptedNormally(
                "20,480 and 20,481 ASCII characters are 20,480 and 20,481 UTF-16 code units, \
                 40,962 bytes at the top — half of the 80 KB budget. Both arms are accepted and \
                 neither is near the boundary",
            ),
            saturating: Saturating::NotDriven {
                waiting_on: "a registered Bot Framework app id + password and a Teams tenant \
                             that will accept it. The arm to drive is 20,480 astral-plane \
                             scalars — 40,960 UTF-16 code units, 81,920 bytes, the recommended \
                             budget exactly — NOT the two-point ASCII probe",
            },
        }),
        env: [
            "WL_LIVE_CAP_MSTEAMS_HOME",
            "WL_LIVE_CAP_MSTEAMS_CHANNEL",
            "WL_LIVE_CAP_MSTEAMS_TO",
        ],
    },
    Cell {
        key: "whatsapp+baileys",
        boundary: Boundary::NotMeasured {
            waiting_on: "a running bridge: Node, an operator's own bridge.js with \
                         @whiskeysockets/baileys installed, and a WhatsApp number QR-paired to \
                         it. NOT a credential — the bridged backends authenticate by pairing, so \
                         nobody can issue one. The number under probe is a POLICY \
                         (BRIDGE_UNMEASURED_CHUNK_WIDTH), not a vendor figure: no vendor \
                         publishes a body limit for the WhatsApp Web protocol",
        },
        env: [
            "WL_LIVE_CAP_WHATSAPP_BAILEYS_HOME",
            "WL_LIVE_CAP_WHATSAPP_BAILEYS_CHANNEL",
            "WL_LIVE_CAP_WHATSAPP_BAILEYS_TO",
        ],
    },
    Cell {
        key: "whatsapp+whatsapp-web",
        boundary: Boundary::NotMeasured {
            waiting_on: "a running bridge: Node, an operator's own bridge.js with \
                         whatsapp-web.js and a Chromium it can drive, and a WhatsApp number \
                         QR-paired to it. Same shape as the baileys cell and same policy number; \
                         a separate cell because the selector is separate and the two backends \
                         speak the protocol through different clients",
        },
        env: [
            "WL_LIVE_CAP_WHATSAPP_WEB_HOME",
            "WL_LIVE_CAP_WHATSAPP_WEB_CHANNEL",
            "WL_LIVE_CAP_WHATSAPP_WEB_TO",
        ],
    },
];

/// The cell for a selector key, or a panic naming the gap.
pub fn cell(key: &str) -> &'static Cell {
    CELLS
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("no probe cell for selector {key:?}"))
}

// ---------------------------------------------------------------------------
// The checkers. Pure, total, and driven by BOTH the passing tests and the
// mutated ones — a second implementation used only by the red arms would prove
// nothing about this one.
// ---------------------------------------------------------------------------

/// Everything wrong with a byte-budget derivation, as human-readable lines.
/// Empty means the shipped cap is exactly the largest scalar count the budget
/// admits at the worst-case encoding.
///
/// **This is the check that decides a byte-budget cap, and it needs no
/// credential.** The claim such a cap makes is arithmetic — *every body of at
/// most `cap` scalars encodes within `budget` bytes* — so it can be checked
/// here, in the same run, for both platforms at once. Both of the mistakes
/// `docs/delivery-semantics.md` records violate it:
///
/// * `matrix.cap` was 32,768, which is `65536 / 2`. At four bytes per scalar
///   that is 131,072 bytes against a 65,536-byte budget: a CJK reply over about
///   21,800 characters was handed to the homeserver whole and dropped.
/// * `msteams.cap` was 28,000, read from the wrong surface and from KB into
///   characters. At four bytes per scalar that is 112,000 bytes against an
///   81,920-byte budget.
///
/// Neither was visible to a cap-vs-document comparison, because in both cases
/// the document and the adapter agreed with each other perfectly.
pub fn derivation_faults(cap: usize, b: &ByteBudget) -> Vec<String> {
    let mut out = Vec::new();
    let worst = b.worst_case_bytes_per_scalar;
    if worst == 0 {
        out.push("worst_case_bytes_per_scalar is zero, which would make every cap admissible and \
                  this checker vacuous"
            .to_string());
        return out;
    }
    let spend = cap.saturating_mul(worst);
    if spend > b.budget_bytes {
        out.push(format!(
            "a body of {cap} scalars costs up to {spend} bytes at {worst} bytes per scalar, over \
             the {} byte budget. The platform rejects the send and send_to_keyed does not \
             re-send it — this is HIGH-6, and it is exactly the shape matrix.cap shipped in when \
             it was 32,768",
            b.budget_bytes
        ));
    }
    let next = cap.saturating_add(1).saturating_mul(worst);
    if next <= b.budget_bytes {
        out.push(format!(
            "the budget admits {} scalars at {worst} bytes each but the cap is {cap}, so bodies \
             are chunked that did not need to be. Below the boundary is the safe direction, but \
             a derived cap that is not the derivation is a number nobody can re-derive",
            b.budget_bytes / worst
        ));
    }
    out
}

/// Everything a settled unit verdict says is wrong with the shipped cap.
///
/// Only a SETTLED verdict can produce a fault: an ASCII-only run establishes
/// nothing about astral text either way, and inventing a fault from it would be
/// the same defect as inventing a pass. The rule that does fire is the one that
/// matters: when a platform counts UTF-16 code units, an astral scalar costs
/// TWO of them, so a scalar cap above `limit / 2` hands the platform a body it
/// refuses — at the cap we ship, for exactly the text emoji-carrying replies
/// are made of.
pub fn unit_safety_faults(cap: usize, unit: &CapUnit) -> Vec<String> {
    let mut out = Vec::new();
    if let CapUnit::MeasuredUtf16CodeUnits {
        limit_code_units,
        on,
        ..
    } = unit
    {
        let safe = limit_code_units / 2;
        if cap > safe {
            out.push(format!(
                "the {on} run settled this limit as {limit_code_units} UTF-16 CODE UNITS, so a \
                 body of astral-plane scalars may be at most {safe} scalars long — but the \
                 shipped cap is {cap}. A {cap}-scalar emoji reply is {} code units, the platform \
                 refuses it, and nothing re-sends it",
                cap * 2
            ));
        }
    }
    out
}
