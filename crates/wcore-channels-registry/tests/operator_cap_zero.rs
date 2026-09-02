//! No operator-supplied number may make a channel declare a cap of ZERO.
//!
//! # The defect this file closes
//!
//! `wcore-channel-whatsapp`'s bridge adapter is the only adapter in the product
//! whose `Channel::max_message_len` is read from configuration rather than
//! written as a literal. The knob — `max_message_chars` — was added so an
//! operator who has actually driven their own `baileys` / `whatsapp-web.js`
//! bridge can replace a chunking width the programme cannot source
//! (wayland-core#360 c1).
//!
//! At exactly one value, that knob did the opposite of its purpose.
//! [`ChannelManager::chunks_for`] reads
//!
//! ```text
//! match max {
//!     Some(max) if max > 0 => chunk_message(text, max),
//!     _ => vec![text.to_string()],
//! }
//! ```
//!
//! so `Some(0)` is not a one-character cap — it misses the guarded arm and
//! falls through to the "this connector declares no limit" arm. Reproduced on
//! `integ/f13` at e1f151a5 through this exact path: `max_message_chars = 0`
//! parsed clean, `max_message_len()` returned `Some(0)`, and a 20,000-character
//! body came back from `chunks_for` as **one** 20,000-character chunk, which is
//! the reject-and-drop direction (HIGH-6) reached through the knob that exists
//! to avoid it.
//!
//! The field's own documentation said "Zero is rejected at parse time by the
//! schema". It was not. `Channel::config_schema` returns a `&str` for a host to
//! display; the only code in this workspace that touches it is three per-crate
//! tests calling `serde_json::from_str(..).expect("schema parses")`, which
//! checks that the schema is valid JSON and validates no config against it. No
//! JSON-Schema validator is a dependency of this build. The `"minimum": 1` in
//! `schemas/whatsapp-bridge.json` documented a bound nothing enforced.
//!
//! # Why this file is at the registry rather than in the adapter's own tests
//!
//! The adapter's unit tests own the value question; this file owns two things
//! they structurally cannot see.
//!
//! 1. **The operator's real path.** A TOML file on disk reaches the adapter
//!    through `parse_channel_config` → [`channel_factory_for`] → the
//!    per-platform factory's `parse_options`. A `serde` guard that works when a
//!    unit test calls `toml::from_str::<WhatsappBridgeConfig>` directly could
//!    still be bypassed if the factory rebuilt the config some other way. This
//!    drives the chain the loader drives.
//! 2. **The class.** The defect is not "the WhatsApp bridge mishandles zero",
//!    it is "an operator-supplied cap reaches a sink that reads zero as
//!    *unbounded*". Today the bridge is the only adapter with such a knob —
//!    every other `max_message_len` in `crates/wcore-channel-*/src` is a
//!    literal. [`no_constructible_adapter_accepts_a_zero_cap`] walks
//!    [`constructible_selectors`] so the *ninth* adapter to grow one is caught
//!    here rather than in production.
//!
//! What that loop can and cannot see is stated at the test, not implied.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wcore_channels_registry::{channel_factory_for, constructible_selectors};
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// In-memory store so no test touches the real keyring. Never read here:
/// every factory in this file resolves handles at `start()`, which is never
/// called — nothing in this file makes a network call.
#[derive(Default)]
struct MemStore(Mutex<HashMap<String, String>>);

impl CredentialsStore for MemStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The key whose zero value is the defect. Named once so the class gate and
/// the end-to-end test cannot drift apart.
const CAP_KEY: &str = "max_message_chars";

/// A complete `~/.wayland/channels/wa.toml` for the bridged backend, with the
/// cap key set to `width`.
///
/// `bridge_path` names a file that need not exist: bridged construction
/// deliberately never touches the filesystem (the failure surfaces at `probe`
/// and `start`), which is what lets this test reach the cap without Node, a
/// `bridge.js` or a paired number.
fn bridge_toml(width: &str) -> String {
    format!(
        r#"
name = "wa"
platform = "whatsapp"

[options]
backend = "baileys"
bridge_path = "/nonexistent/bridge.js"
{CAP_KEY} = {width}
"#
    )
}

/// Build a channel the way `auto_register_from_dir` does: parse the file, look
/// up the factory by platform tag, hand it the `[options]` table.
fn load(body: &str) -> Result<Box<dyn wcore_channels::Channel>, String> {
    let cfg = wcore_channels::parse_channel_config("wa.toml", body).map_err(|e| e.to_string())?;
    let factory = channel_factory_for(&cfg.platform)
        .unwrap_or_else(|| panic!("no factory for platform {:?}", cfg.platform));
    factory(
        cfg.name.clone(),
        &cfg.options,
        Arc::new(MemStore::default()),
    )
    .map_err(|e| e.to_string())
}

/// `max_message_chars = 0` must not load, and the refusal must be legible.
///
/// This is the reproduction, inverted. Before the guard it passed every layer:
/// `parse_channel_config` accepted the file, the factory accepted the options,
/// and the channel came back declaring `Some(0)`. The assertion below is on
/// the LOAD, not on the cap, because a channel that loads with a zero has
/// already lost — nothing downstream of the factory is told what the operator
/// asked for.
#[test]
fn a_zero_cap_config_does_not_load_at_all() {
    let err = match load(&bridge_toml("0")) {
        Err(e) => e,
        Ok(ch) => panic!(
            "a channel config setting max_message_chars = 0 LOADED, declaring cap {:?}. \
             Zero misses ChannelManager::chunks_for's `Some(max) if max > 0` arm, so the \
             whole body goes to the bridge as one message at a limit no vendor publishes.",
            ch.max_message_len()
        ),
    };

    assert!(
        err.contains(CAP_KEY),
        "the operator has to know WHICH key to change; got: {err}"
    );
    assert!(
        err.contains("at least 1"),
        "the operator has to know what a legal value is; got: {err}"
    );
}

/// Known-positive for the harness above: the same file, the same chain, a
/// legal width — and the cap that comes out is the operator's.
///
/// Without this, [`a_zero_cap_config_does_not_load_at_all`] would still pass if
/// the whatsapp factory started rejecting every config for an unrelated reason,
/// and the gate would be measuring nothing.
#[test]
fn the_same_chain_loads_a_legal_width_and_the_operators_number_is_the_one_in_force() {
    let ch = load(&bridge_toml("1"))
        .expect("1 is the smallest legal width; the guard is `>= 1`, not `> 1`");
    assert_eq!(
        ch.max_message_len(),
        Some(1),
        "the operator's width must be the one the chunker reads"
    );

    let wide = load(&bridge_toml("60000")).expect("a wide measured width must still load");
    assert_eq!(wide.max_message_len(), Some(60_000));

    // And the cap is load-bearing at that width, read exactly as `send_to_keyed`
    // reads it — a declared number nothing splits on would be decoration.
    let body = "x".repeat(3);
    let chunks = wcore_channels::manager::ChannelManager::chunks_for(ch.max_message_len(), &body);
    assert_eq!(chunks.len(), 3, "a 3-char body at cap 1 must split into 3");
    assert_eq!(chunks.concat(), body, "the split must be lossless");
}

/// THE CLASS GATE. No adapter this build can construct may be talked into
/// declaring a cap of zero by an operator setting [`CAP_KEY`].
///
/// # What this loop sees
///
/// [`constructible_selectors`] is the enumeration of every distinct
/// implementation `channel_factory_for` can build, including the two reached by
/// a config key rather than a platform string (`whatsapp+baileys`,
/// `whatsapp+whatsapp-web`) — the pair that made the bridge invisible to every
/// gate written against a platform list. Each selector gets a minimal options
/// table with `max_message_chars = 0` injected, and the only two acceptable
/// outcomes are: the factory REFUSES, or it builds an adapter whose declared
/// cap is not `Some(0)`.
///
/// Today every adapter but the bridge refuses on `deny_unknown_fields` — they
/// have no such knob, and their caps are literals. That is the point: when a
/// ninth adapter grows an operator-settable width, this loop starts exercising
/// it the moment the key is accepted, and reddens unless the author guarded
/// zero.
///
/// # What this loop does NOT see, stated rather than implied
///
/// It is keyed on the SPELLING `max_message_chars`. A sibling that names its
/// knob something else is not covered, and there is no way to recognise "this
/// integer is a cap" generically. It also cannot reach an adapter whose other
/// required fields are unsatisfied by the probe table — such a factory errors
/// for a different reason and its arm passes without having tested anything.
///
/// So the loop runs TWICE, at `0` and at `1`, and asserts on the difference.
/// The `1` pass is the non-vacuity proof: it names the selectors that accept
/// this key at all, and that set must not be empty — if it ever is, every arm
/// of the `0` pass is refusing for an unrelated reason and this gate has gone
/// quiet without going red. The two sets together are the property: a selector
/// that takes the key must take `1` and refuse `0`.
#[test]
fn no_constructible_adapter_accepts_a_zero_cap() {
    let selectors = constructible_selectors();
    assert!(
        selectors.len() >= 2,
        "constructible_selectors() returned {} entries — if the enumeration has \
         collapsed this gate walks nothing",
        selectors.len()
    );

    /// Which selectors load with `CAP_KEY = width`, and what cap each declares.
    fn loads_with(width: i64) -> Vec<(String, Option<usize>)> {
        let mut out = Vec::new();
        for sel in constructible_selectors() {
            let mut options = toml::Table::new();
            sel.apply(&mut options);
            // Enough for the bridged whatsapp factory to construct; harmless
            // elsewhere, where the factory refuses on its own required fields.
            options.insert(
                "bridge_path".to_string(),
                toml::Value::String("/nonexistent/bridge.js".to_string()),
            );
            options.insert(CAP_KEY.to_string(), toml::Value::Integer(width));

            let Some(factory) = channel_factory_for(sel.platform) else {
                panic!(
                    "constructible_selectors() names platform {:?} but channel_factory_for \
                     does not answer it",
                    sel.platform
                );
            };

            if let Ok(ch) = factory(
                "cap-probe".to_string(),
                &options,
                Arc::new(MemStore::default()),
            ) {
                out.push((sel.key.clone(), ch.max_message_len()));
            }
        }
        out
    }

    // Non-vacuity first, so a silent loss of coverage reddens here rather than
    // dressing itself up as a clean pass below.
    let took_one = loads_with(1);
    assert!(
        !took_one.is_empty(),
        "NO constructible adapter accepted `{CAP_KEY} = 1`. Every arm of the zero pass is \
         therefore refusing for some unrelated reason and this gate is measuring nothing. \
         Either the probe options no longer satisfy any factory, or the key was renamed — \
         fix the probe, do not delete this assertion."
    );
    for (key, cap) in &took_one {
        assert_eq!(
            *cap,
            Some(1),
            "{key} accepted `{CAP_KEY} = 1` but declares {cap:?} — the operator's width is \
             not the number the chunker reads"
        );
    }

    let took_zero = loads_with(0);
    for (key, cap) in &took_zero {
        assert_ne!(
            *cap,
            Some(0),
            "{key} built from a config with {CAP_KEY} = 0 and declares a cap of zero. \
             ChannelManager::chunks_for reads that as NO cap, so every outbound body goes \
             to the platform whole and an over-long one is rejected and dropped (HIGH-6)."
        );
    }
    let names: Vec<&str> = took_zero.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        took_zero.is_empty(),
        "these adapters LOADED with {CAP_KEY} = 0 instead of refusing it: {names:?}. \
         An adapter that survives a zero has to prove it cannot declare one; if that is what \
         it now does, say so here rather than deleting this assertion."
    );
}
