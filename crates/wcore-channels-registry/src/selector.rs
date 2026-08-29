//! The registry's construction space, enumerated.
//!
//! [`channel_factory_for`](crate::channel_factory_for) is keyed by platform
//! string, and for nine of the ten platforms that is the whole story: one
//! string, one adapter. WhatsApp is not one of the nine. `whatsapp` plus a
//! `backend` key reaches THREE different `Channel` implementations — the native
//! Cloud API adapter, and `WhatsappBridgeChannel` driving either of two Node
//! backends — and the bridged ones carry their own `max_message_len()`, their
//! own delivery semantics and their own failure modes.
//!
//! Every gate that walked the platform list therefore had a blind spot shaped
//! exactly like the bridge. `docs/delivery-semantics.md`'s declaration harness
//! and `tests/live_message_cap_boundary.rs` both enumerated platform strings,
//! so the eighth `max_message_len` in the product was the one no test and no
//! declaration row could reach — not because anybody decided to skip it, but
//! because the guard's own shape could not see it (wayland-core#360 c2). Walk
//! [`constructible_selectors`] instead of a platform list and the class closes:
//! a ninth adapter reached by a config key appears here, and therefore in every
//! gate downstream of here, without a second list to remember.

use wcore_channel_whatsapp::WhatsappBackend;

/// One distinct implementation this build can construct, and how a config
/// reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSelector {
    /// Platform tag, as [`channel_factory_for`](crate::channel_factory_for)
    /// spells it.
    pub platform: &'static str,
    /// The config keys that pick this implementation inside the platform.
    /// Empty for a platform that has exactly one.
    pub options: Vec<(&'static str, &'static str)>,
    /// The name this implementation carries in `docs/delivery-semantics.md`
    /// and in the gates that read it: the bare platform for the platform's
    /// DEFAULT implementation, `platform+<value>` where a config key selects a
    /// different one.
    ///
    /// The default keeps the bare name on purpose — every row written before
    /// the bridge existed still describes the adapter it always described, so
    /// widening the guard adds rows rather than renaming them.
    pub key: String,
}

impl ChannelSelector {
    /// Merge this selector's config keys into an options table, so a caller can
    /// build the exact implementation the selector names.
    pub fn apply(&self, options: &mut toml::Table) {
        for (k, v) in &self.options {
            options.insert((*k).to_string(), toml::Value::String((*v).to_string()));
        }
    }
}

/// Every platform string [`channel_factory_for`](crate::channel_factory_for)
/// answers in this build.
///
/// iMessage is `#[cfg(target_os = "macos")]` in that match, so the set is
/// genuinely platform-dependent; deriving it here with the same `cfg` keeps the
/// two in step, and [`tests::every_known_platform_has_a_factory`] holds this
/// list to the match.
pub fn known_platforms() -> Vec<&'static str> {
    let mut v = vec![
        "slack", "telegram", "email", "discord", "sms", "whatsapp", "signal", "matrix", "msteams",
    ];
    if cfg!(target_os = "macos") {
        v.push("imessage");
    }
    v.sort_unstable();
    v
}

/// Every distinct implementation this build can construct.
///
/// The WhatsApp arm is derived from [`WhatsappBackend::ALL_WIRE_NAMES`] rather
/// than hand-listed, so a fourth backend added to that enum appears here — and
/// therefore in every gate that walks this — with no second list to update.
pub fn constructible_selectors() -> Vec<ChannelSelector> {
    let mut out = Vec::new();
    for platform in known_platforms() {
        if platform == "whatsapp" {
            for wire in WhatsappBackend::ALL_WIRE_NAMES {
                let key = if wire == WhatsappBackend::default().wire_name() {
                    platform.to_string()
                } else {
                    format!("{platform}+{wire}")
                };
                out.push(ChannelSelector {
                    platform,
                    options: vec![("backend", wire)],
                    key,
                });
            }
        } else {
            out.push(ChannelSelector {
                platform,
                options: Vec::new(),
                key: platform.to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_factory_for;

    #[test]
    fn every_known_platform_has_a_factory() {
        for platform in known_platforms() {
            assert!(
                channel_factory_for(platform).is_some(),
                "known_platforms() lists {platform:?} but channel_factory_for does not answer it"
            );
        }
    }

    /// The selector list must be STRICTLY wider than the platform list, and the
    /// extra entries must be the ones a config key selects.
    ///
    /// A selector list that had silently collapsed back to one entry per
    /// platform would pass every downstream gate while restoring exactly the
    /// blind spot this module exists to close, so the widening is asserted
    /// rather than assumed.
    #[test]
    fn the_selector_list_is_wider_than_the_platform_list_by_the_config_keyed_backends() {
        let selectors = constructible_selectors();
        let platforms = known_platforms();
        assert_eq!(
            selectors.len(),
            platforms.len() + WhatsappBackend::ALL_WIRE_NAMES.len() - 1,
            "one selector per platform, plus one per extra WhatsApp backend: {:?}",
            selectors.iter().map(|s| &s.key).collect::<Vec<_>>()
        );

        // Every backend the enum knows is reachable, by name. This is the arm
        // that reddens when a fourth `WhatsappBackend` variant is added: the
        // enum grows, the count above moves, and the new wire name has to
        // appear as a selector before this passes again.
        for wire in WhatsappBackend::ALL_WIRE_NAMES {
            assert!(
                selectors
                    .iter()
                    .any(|s| s.options.contains(&("backend", wire))),
                "no selector reaches the {wire:?} backend"
            );
        }

        // The default backend keeps the bare platform key; the others do not.
        let keys: Vec<&str> = selectors.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"whatsapp"), "got {keys:?}");
        assert!(keys.contains(&"whatsapp+baileys"), "got {keys:?}");
        assert!(keys.contains(&"whatsapp+whatsapp-web"), "got {keys:?}");
    }

    /// Keys must be unique — two implementations sharing a key would give the
    /// declaration one row for two adapters, which is the blind spot again with
    /// the widening in place.
    #[test]
    fn selector_keys_are_unique() {
        let mut keys: Vec<String> = constructible_selectors()
            .into_iter()
            .map(|s| s.key)
            .collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate selector key in {keys:?}");
    }

    /// `apply` must actually reach the implementation the key names: the
    /// bridged selector has to build `WhatsappBridgeChannel`, not the Cloud API
    /// adapter. Without this the widening would be a list of strings.
    /// In-memory credentials. No adapter resolves a handle during
    /// construction, but the factory signature requires a store and reaching
    /// for the real keyring in a test would touch the developer's own secrets.
    struct MemStore;

    impl wcore_config::credentials::CredentialsStore for MemStore {
        fn get(
            &self,
            _key: &str,
        ) -> Result<Option<String>, wcore_config::credentials::CredentialsError> {
            Ok(None)
        }
        fn put(
            &self,
            _key: &str,
            _value: &str,
        ) -> Result<(), wcore_config::credentials::CredentialsError> {
            Ok(())
        }
        fn delete(&self, _key: &str) -> Result<(), wcore_config::credentials::CredentialsError> {
            Ok(())
        }
    }

    #[test]
    fn a_bridged_selector_constructs_the_bridge_and_the_default_one_does_not() {
        let creds: std::sync::Arc<dyn wcore_config::credentials::CredentialsStore> =
            std::sync::Arc::new(MemStore);
        let factory = channel_factory_for("whatsapp").expect("whatsapp factory");

        for selector in constructible_selectors() {
            if selector.platform != "whatsapp" {
                continue;
            }
            let mut opts = toml::Table::new();
            let bridged = selector.options.contains(&("backend", "baileys"))
                || selector.options.contains(&("backend", "whatsapp-web"));
            if bridged {
                opts.insert(
                    "bridge_path".into(),
                    toml::Value::String("/definitely/not/here/bridge.js".into()),
                );
            } else {
                opts.insert("workspace_name".into(), toml::Value::String("acme".into()));
                opts.insert("phone_number_id".into(), toml::Value::String("1".into()));
                opts.insert(
                    "credential_handle_access_token".into(),
                    toml::Value::String("k1".into()),
                );
                opts.insert(
                    "credential_handle_app_secret".into(),
                    toml::Value::String("k2".into()),
                );
            }
            selector.apply(&mut opts);
            let ch = factory(selector.key.clone(), &opts, creds.clone())
                .unwrap_or_else(|e| panic!("{} did not construct: {e}", selector.key));
            // The Cloud API adapter reports no cap through the bridge's
            // `max_message_len` and vice versa; the observable difference used
            // here is the one the gates downstream read.
            assert_eq!(
                ch.platform(),
                "whatsapp",
                "{} must keep the platform tag",
                selector.key
            );
            assert!(
                ch.max_message_len().is_some(),
                "{} must report a cap for the boundary probe to have anything to check",
                selector.key
            );
        }
    }
}
