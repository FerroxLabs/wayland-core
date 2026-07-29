//! F24-C3-H5 — the per-channel inbound policy + tool posture, as ONE shared,
//! swappable object.
//!
//! # The defect this module closes
//!
//! `channel reload` rebuilt the gateway's **adapter** set and nothing else.
//! `channel health` then reported the new channel `healthy`, its webhook
//! endpoint answered `HTTP 200`, and every message to it was **silently
//! denied** — because the access-policy map had been read once at startup
//! ([`crate::channel_inbound_host::spawn`]), moved into the subscriber task
//! (`channel_inbound.rs`, `let policies = self.policies;`), and could not be
//! reached again by anything. A channel absent from that map falls through to
//! [`wcore_channels::InboundPolicy::default`], which is fail-closed over an
//! empty allowlist.
//!
//! Measured with a one-variable control: the identical config from the
//! identical generator was **admitted** when present at gateway start and
//! **denied** when introduced by reload.
//!
//! Failing closed is the correct posture and is not the bug. The bug is that
//! three independent surfaces told the operator the channel worked.
//!
//! # Why the two maps live in one object
//!
//! There are **two** stale maps, not one:
//!
//! 1. the access policy (`InboundPolicy`) the subscriber admits on, and
//! 2. the tool posture ([`ChannelToolScope`]) the dispatcher runs the turn
//!    under — built by the same code path, from the same configs, and moved
//!    into [`crate::channel_dispatch::ChannelTurnDispatcher`] just as finally.
//!
//! Refreshing only (1) is **worse than the original defect**: messages would
//! start arriving — so a re-run of the obvious test goes green — while the
//! channel runs under [`wcore_channels::ChannelToolPosture`]'s fallback rather
//! than the posture its config asked for. The bug stops being fail-closed and
//! starts being silently-wrong-permissions, and the green hides it.
//!
//! So the repair does not offer that half as an option. Both maps live behind
//! **one** `RwLock`, are derived by **one** function ([`ChannelPolicySnapshot::
//! from_configs`]), and are swapped by **one** call ([`ChannelPolicyRegistry::
//! replace`]) under a single write lock that bumps a single generation counter.
//! There is deliberately no `replace_policies` and no `policies_mut`: a caller
//! that wanted to refresh one facet cannot express it.
//!
//! # Locking
//!
//! `std::sync::RwLock`, not `tokio::sync::RwLock`. Both read sites are bounded
//! map lookups that clone their value out and are **never** held across an
//! `await` — the same discipline `channel_inbound.rs` already applies to its
//! `Arc<std::sync::Mutex<AutoReplyRateLimiter>>`. An async lock here would buy
//! nothing and would make the read sites `.await` inside the broadcast drain
//! loop, which must stay O(µs).
//!
//! A poisoned lock is recovered rather than panicked on: the guarded value is
//! two `HashMap`s that are only ever wholesale-replaced, so a panic elsewhere
//! cannot have left them torn. Propagating the poison would take the inbound
//! path down for the life of the process, which is a strictly worse failure
//! than serving the last good snapshot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use wcore_channels::InboundPolicy;
use wcore_channels::config::ChannelConfig;

use crate::channel_tools::ChannelToolScope;

/// One coherent view of every channel's inbound configuration.
///
/// The two maps are always derived together from the same `Vec<ChannelConfig>`,
/// so they cannot describe different channel sets.
#[derive(Debug, Clone, Default)]
pub struct ChannelPolicySnapshot {
    /// Per-channel access policy, keyed by channel name.
    pub policies: HashMap<String, InboundPolicy>,
    /// Per-channel resolved tool scope, keyed by channel name.
    pub postures: HashMap<String, ChannelToolScope>,
    /// Bumped once per [`ChannelPolicyRegistry::replace`]. `0` is the snapshot
    /// installed at construction. Carried so a caller (and a test) can prove a
    /// refresh actually happened rather than inferring it from a count that
    /// would be unchanged if the reload added nothing.
    pub generation: u64,
}

impl ChannelPolicySnapshot {
    /// Derive both maps from the channel configs on disk.
    ///
    /// This is the ONLY place either map is built. Before F24-C3-H5 the same
    /// derivation was open-coded twice — in `channel_inbound_host::spawn` and
    /// in `bootstrap.rs` — which is how the two hosts were able to drift and
    /// how a "refresh the policies" change could plausibly forget the postures.
    ///
    /// `default_workspace_root` is the working directory a channel that names
    /// no `tool_workspace_root` is jailed to.
    pub fn from_configs(configs: Vec<ChannelConfig>, default_workspace_root: &Path) -> Self {
        let postures: HashMap<String, ChannelToolScope> = configs
            .iter()
            .map(|c| {
                let root = c
                    .inbound
                    .tool_workspace_root
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_workspace_root.to_path_buf());
                (
                    c.name.clone(),
                    ChannelToolScope {
                        posture: c.inbound.tools,
                        workspace_root: root,
                    },
                )
            })
            .collect();

        let policies: HashMap<String, InboundPolicy> =
            configs.into_iter().map(|c| (c.name, c.inbound)).collect();

        debug_assert_eq!(
            policies.len(),
            postures.len(),
            "the two maps are derived from one config list and must cover the same channels"
        );

        Self {
            policies,
            postures,
            generation: 0,
        }
    }

    /// Channel names covered by this snapshot, sorted. Used by callers that
    /// report what a reload picked up, and by tests asserting set equality.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.policies.keys().cloned().collect();
        names.sort();
        names
    }
}

/// The shared, swappable inbound configuration the subscriber and the
/// dispatcher both read.
///
/// Held as an `Arc<ChannelPolicyRegistry>` by:
///
/// - [`crate::channel_inbound::InboundSubscriber`] — reads `policy_for` per
///   inbound event;
/// - [`crate::channel_dispatch::ChannelTurnDispatcher`] — reads `scope_for`
///   per admitted turn;
/// - [`crate::channel_inbound_host::InboundHost`] — hands it back to the
///   runtime so `gateway run`'s reload block can refresh it.
#[derive(Debug, Default)]
pub struct ChannelPolicyRegistry {
    inner: RwLock<ChannelPolicySnapshot>,
}

impl ChannelPolicyRegistry {
    /// Install `snapshot` as generation 0.
    pub fn new(snapshot: ChannelPolicySnapshot) -> Self {
        Self {
            inner: RwLock::new(snapshot),
        }
    }

    /// Build from two already-derived maps.
    ///
    /// Both are required — there is no `from_policies`. Constructing with an
    /// empty posture map is legal (it means "every channel takes the caller's
    /// fallback scope") but it has to be written down, so a production call
    /// site that dropped the postures is visible in the diff rather than
    /// implied by an absent argument.
    pub fn from_parts(
        policies: HashMap<String, InboundPolicy>,
        postures: HashMap<String, ChannelToolScope>,
    ) -> Self {
        Self::new(ChannelPolicySnapshot {
            policies,
            postures,
            generation: 0,
        })
    }

    /// Build from configs (see [`ChannelPolicySnapshot::from_configs`]).
    pub fn from_configs(configs: Vec<ChannelConfig>, default_workspace_root: &Path) -> Self {
        Self::new(ChannelPolicySnapshot::from_configs(
            configs,
            default_workspace_root,
        ))
    }

    /// Read guard, recovering from poisoning (see the module docs).
    fn read(&self) -> std::sync::RwLockReadGuard<'_, ChannelPolicySnapshot> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    /// The access policy for `channel_name`.
    ///
    /// An ABSENT channel yields [`InboundPolicy::default`], which is
    /// fail-closed. That is unchanged and deliberate: the repair makes the map
    /// current, it does not make an unknown channel permissive.
    pub fn policy_for(&self, channel_name: &str) -> InboundPolicy {
        self.read()
            .policies
            .get(channel_name)
            .cloned()
            .unwrap_or_default()
    }

    /// The resolved tool scope for `channel_name`, or `None` when the channel
    /// is absent — the caller supplies its own safe fallback, because the
    /// fallback's workspace root is the caller's `cwd` and is not knowable
    /// here.
    pub fn scope_for(&self, channel_name: &str) -> Option<ChannelToolScope> {
        self.read().postures.get(channel_name).cloned()
    }

    /// Swap BOTH maps, atomically, and bump the generation. Returns the number
    /// of channels now covered.
    ///
    /// There is no single-facet variant, by design: see the module docs.
    pub fn replace(&self, snapshot: ChannelPolicySnapshot) -> usize {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let generation = guard.generation + 1;
        *guard = ChannelPolicySnapshot {
            generation,
            ..snapshot
        };
        guard.policies.len()
    }

    /// Re-derive both maps from `configs` and swap them in. Returns the new
    /// channel count.
    pub fn replace_from_configs(
        &self,
        configs: Vec<ChannelConfig>,
        default_workspace_root: &Path,
    ) -> usize {
        self.replace(ChannelPolicySnapshot::from_configs(
            configs,
            default_workspace_root,
        ))
    }

    /// Number of channels covered by the current snapshot.
    pub fn len(&self) -> usize {
        self.read().policies.len()
    }

    /// Whether the current snapshot covers no channels.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sorted channel names in the current snapshot.
    pub fn names(&self) -> Vec<String> {
        self.read().names()
    }

    /// The current generation. `0` until the first [`Self::replace`].
    pub fn generation(&self) -> u64 {
        self.read().generation
    }

    /// A clone of the whole current snapshot. For callers that need the two
    /// maps to agree with each other across several lookups, and for tests
    /// asserting posture equality between two lifecycles.
    pub fn snapshot(&self) -> ChannelPolicySnapshot {
        self.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_channels::ChannelToolPosture;
    use wcore_channels::config::ChannelConfig;

    fn config(name: &str, posture: ChannelToolPosture, root: Option<&str>) -> ChannelConfig {
        let inbound = InboundPolicy {
            tools: posture,
            tool_workspace_root: root.map(|r| r.to_string()),
            // The admission-relevant difference from the fail-closed default.
            // Set on every fixture so a facet-1 assertion is about who may
            // send, not incidentally about the posture fields that also live
            // on this struct.
            dm_allowlist: vec![format!("sender-for-{name}")],
            ..Default::default()
        };
        ChannelConfig {
            name: name.to_string(),
            platform: "slack".to_string(),
            enabled: true,
            options: toml::Table::new(),
            secrets: toml::Table::new(),
            inbound,
        }
    }

    #[test]
    fn an_absent_channel_still_fails_closed() {
        let reg = ChannelPolicyRegistry::from_configs(vec![], Path::new("/w"));
        let policy = reg.policy_for("never-configured");
        assert_eq!(
            policy,
            InboundPolicy::default(),
            "an unknown channel must keep the fail-closed default; the repair makes the map \
             current, it must not make unknown channels permissive"
        );
        assert!(reg.scope_for("never-configured").is_none());
    }

    /// The whole point. Both facets must move together.
    #[test]
    fn replace_refreshes_the_policy_and_the_posture_in_one_swap() {
        let reg = ChannelPolicyRegistry::from_configs(
            vec![config("known", ChannelToolPosture::Conversational, None)],
            Path::new("/w"),
        );
        assert_eq!(reg.generation(), 0);
        assert!(
            reg.scope_for("added-later").is_none(),
            "precondition: the new channel is absent before the reload"
        );

        let n = reg.replace_from_configs(
            vec![
                config("known", ChannelToolPosture::Conversational, None),
                config("added-later", ChannelToolPosture::Workspace, Some("/jail")),
            ],
            Path::new("/w"),
        );

        assert_eq!(n, 2);
        assert_eq!(reg.generation(), 1, "a swap must be observable as a bump");

        // FACET 1 — the policy is now present, so admission consults the real
        // config rather than the fail-closed default. Asserted on the
        // admission-deciding field, not merely on "!= default".
        let policy = reg.policy_for("added-later");
        assert_eq!(
            policy.dm_allowlist,
            vec!["sender-for-added-later".to_string()],
            "facet 1: the reloaded channel must carry its own allowlist"
        );
        assert!(
            InboundPolicy::default().dm_allowlist.is_empty(),
            "control: the default this would otherwise fall through to permits nobody"
        );

        // FACET 2 — and the posture came with it. This is the assertion that a
        // policy-only repair fails.
        let scope = reg
            .scope_for("added-later")
            .expect("facet 2: the reloaded channel must also carry a tool posture");
        assert_eq!(scope.posture, ChannelToolPosture::Workspace);
        assert_eq!(scope.workspace_root, PathBuf::from("/jail"));
    }

    /// The equality the live acceptance test asserts, checked here without a
    /// gateway: the same config must produce the same posture whether it was
    /// present at construction or arrived by a later swap.
    #[test]
    fn a_reloaded_channel_gets_the_same_posture_as_one_present_at_startup() {
        let cfg = || config("c", ChannelToolPosture::Workspace, Some("/jail"));

        let at_startup = ChannelPolicyRegistry::from_configs(vec![cfg()], Path::new("/w"));
        let via_reload = ChannelPolicyRegistry::from_configs(vec![], Path::new("/w"));
        via_reload.replace_from_configs(vec![cfg()], Path::new("/w"));

        assert_eq!(
            at_startup
                .scope_for("c")
                .map(|s| (s.posture, s.workspace_root)),
            via_reload
                .scope_for("c")
                .map(|s| (s.posture, s.workspace_root)),
            "reloaded posture must EQUAL startup posture for identical config"
        );
        assert_eq!(
            at_startup.policy_for("c"),
            via_reload.policy_for("c"),
            "reloaded policy must EQUAL startup policy for identical config"
        );
    }

    /// A channel REMOVED from disk must lose its policy on the next reload,
    /// not keep the one it had. Without this, the repair would be a one-way
    /// ratchet that can add authority but never withdraw it.
    #[test]
    fn a_removed_channel_reverts_to_fail_closed() {
        let reg = ChannelPolicyRegistry::from_configs(
            vec![config(
                "going-away",
                ChannelToolPosture::Workspace,
                Some("/j"),
            )],
            Path::new("/w"),
        );
        assert!(reg.scope_for("going-away").is_some());

        reg.replace_from_configs(vec![], Path::new("/w"));

        assert_eq!(reg.policy_for("going-away"), InboundPolicy::default());
        assert!(reg.scope_for("going-away").is_none());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn the_default_workspace_root_is_used_only_when_the_config_names_none() {
        let reg = ChannelPolicyRegistry::from_configs(
            vec![
                config("inherits", ChannelToolPosture::Workspace, None),
                config("jailed", ChannelToolPosture::Workspace, Some("/elsewhere")),
            ],
            Path::new("/default-root"),
        );
        assert_eq!(
            reg.scope_for("inherits").unwrap().workspace_root,
            PathBuf::from("/default-root")
        );
        assert_eq!(
            reg.scope_for("jailed").unwrap().workspace_root,
            PathBuf::from("/elsewhere")
        );
    }
}
