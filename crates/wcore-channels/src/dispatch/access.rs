//! Fail-closed inbound access policy — THE security gate.
//!
//! `decide_access` is the single chokepoint that decides whether an
//! inbound message is permitted to reach the agent. Its posture is
//! deliberately fail-closed: an unconfigured channel denies everything
//! until the operator adds explicit allowlist entries (see
//! [`InboundPolicy`]'s `Default`).
//!
//! This module is pure config + logic — no I/O, no async. The
//! orchestrator in [`crate::dispatch`] combines it with classification,
//! dedup, and session-key derivation.

use serde::{Deserialize, Serialize};

use crate::event::{ChatType, IncomingMessage};

/// Policy governing who may DM the bot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Anyone may DM the bot.
    Open,
    /// Only `sender_id`s in `dm_allowlist` may DM the bot.
    Allowlist,
    /// Pairing handshake required (deferred to a later phase — currently
    /// fail-closed: every pairing DM is denied).
    Pairing,
    /// DMs are rejected entirely.
    Disabled,
}

/// Filesystem/shell posture for a channel-originated agent turn.
///
/// A channel sender is REMOTE and (depending on the access policy) may be
/// untrusted, so the per-conversation agent engine must not inherit the
/// local CLI's full host access. This enum selects which built-in tools
/// the channel engine is built with — enforced at tool-registration time
/// in `wcore-agent` (the `wcore-channels` crate only carries the config).
///
/// **Default is [`Conversational`](ChannelToolPosture::Conversational)** —
/// the safe floor: no host filesystem, no shell. Operators opt UP to
/// `Workspace` (jailed filesystem) or `Full` (host-wide) per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelToolPosture {
    /// No filesystem and no shell. Only conversational/network tools
    /// (and operator-wired MCP servers) are exposed. The safe default
    /// for remote chat senders — closes host-secret exfiltration.
    #[default]
    Conversational,
    /// Filesystem tools (Read/Write/Edit/Grep/Glob) are available but
    /// JAILED to a workspace root via `SandboxedFs`; shell/exec tools
    /// (Bash, git, kubectl, …) remain unavailable because they bypass the
    /// jail. Lets a channel agent do real, confined filesystem work.
    Workspace,
    /// Full host access — every tool, no jail. Identical to a local CLI
    /// session. Dangerous for publicly-reachable channels; explicit
    /// opt-in only.
    Full,
}

/// How the bot acknowledges an inbound message it's working on.
///
/// A human who messages a bot wants to know it heard them. This selects
/// the ack signal the inbound subscriber emits around a turn: emoji
/// reactions on the triggering message (👀 received → ✅ done / ❌ failed)
/// and/or a periodic "typing…" indicator while the turn runs. Both are
/// best-effort — a connector that lacks the platform API no-ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AckMode {
    /// No acknowledgement (default).
    #[default]
    Off,
    /// React 👀 on receipt, ✅/❌ on completion.
    Reactions,
    /// Send a typing indicator, refreshed while the turn runs.
    Typing,
    /// Both reactions and typing.
    Both,
}

impl AckMode {
    /// Whether this mode emits emoji reactions.
    pub fn reactions(self) -> bool {
        matches!(self, AckMode::Reactions | AckMode::Both)
    }
    /// Whether this mode emits a typing indicator.
    pub fn typing(self) -> bool {
        matches!(self, AckMode::Typing | AckMode::Both)
    }
}

/// Policy governing whether group/channel messages are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Any group/channel message is accepted (still subject to
    /// mention-gating, which is enforced in admission, not here).
    Open,
    /// Only allowlisted group chats AND allowlisted senders are accepted.
    Allowlist,
    /// Group/channel messages are rejected entirely.
    Disabled,
}

/// Inbound access + session-shaping policy for one channel.
///
/// **Fail-closed by default.** The [`Default`] impl denies all inbound
/// until the operator opts in: `dm: Allowlist` with an EMPTY allowlist
/// (so no one is permitted), `group: Disabled`, and `require_mention:
/// true`. An unconfigured channel therefore rejects every message. To
/// open DMs to everyone, set `dm_allowlist = ["*"]`; to allow a specific
/// person, add their stable `sender_id`.
///
/// Allowlist semantics: a list permits an id iff it contains the literal
/// `"*"` (wildcard) OR the exact id. An EMPTY list permits NOTHING.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InboundPolicy {
    /// Who may DM the bot.
    #[serde(default = "default_dm_policy")]
    pub dm: DmPolicy,
    /// Whether group/channel messages are accepted.
    #[serde(default = "default_group_policy")]
    pub group: GroupPolicy,
    /// In groups, only act when the bot is addressed (mention/reply/
    /// quote/thread). Enforced in admission classification.
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,
    /// Permitted `sender_id`s for DMs. `"*"` = any. Empty = none.
    #[serde(default)]
    pub dm_allowlist: Vec<String>,
    /// Permitted group `conversation_id`s. `"*"` = any. Empty = none.
    #[serde(default)]
    pub group_allowlist: Vec<String>,
    /// Permitted `sender_id`s within groups. `"*"` = any. Empty = none.
    #[serde(default)]
    pub sender_allowlist: Vec<String>,
    /// Give each user their own isolated session within a group. See
    /// [`crate::dispatch::build_session_key`].
    #[serde(default = "default_true")]
    pub group_sessions_per_user: bool,
    /// Split sessions per thread within a group. See
    /// [`crate::dispatch::build_session_key`].
    #[serde(default)]
    pub thread_sessions_per_user: bool,
    /// Filesystem/shell posture for this channel's agent turns. Defaults
    /// to [`ChannelToolPosture::Conversational`] (no host fs/shell) so a
    /// remote sender cannot read host secrets. See [`ChannelToolPosture`].
    #[serde(default)]
    pub tools: ChannelToolPosture,
    /// Root the `Workspace` posture jails filesystem tools to. Ignored
    /// for `Conversational`/`Full`. When `None`, the agent engine's
    /// working directory is used as the jail root.
    #[serde(default)]
    pub tool_workspace_root: Option<String>,
    /// How the bot acknowledges inbound messages it's working on
    /// (reactions / typing). Defaults to [`AckMode::Off`].
    #[serde(default)]
    pub ack: AckMode,
    /// Per-field opt-out from the admits-everyone startup refusal.
    ///
    /// Each entry is the NAME of an `[inbound]` key this channel is
    /// deliberately leaving open — one of `"dm"`, `"dm_allowlist"`,
    /// `"group"`, `"sender_allowlist"`. See [`open_admissions`] and
    /// [`refuse_open_admission`].
    ///
    /// It is a list of field names rather than a boolean on purpose: a
    /// blanket `allow_open = true` written once keeps silently covering
    /// every field the operator opens afterwards. Naming each field means
    /// opening a SECOND one refuses again, so the acknowledgement cannot
    /// outlive the decision it recorded.
    ///
    /// **Where it can be set.** This key lives in the channel's own file
    /// under `<profile home>/channels/<name>.toml` — the only source
    /// [`crate::config::ChannelConfigLoader`] ever reads. A project-local
    /// `.wayland-core.toml` travels with a cloned repository and is
    /// untrusted; it deserializes into `wcore_config`'s `Config`, which has
    /// no channel-policy surface at all, so it cannot reach this field.
    #[serde(default)]
    pub acknowledge_open_admission: Vec<String>,
}

fn default_dm_policy() -> DmPolicy {
    DmPolicy::Allowlist
}
fn default_group_policy() -> GroupPolicy {
    GroupPolicy::Disabled
}
fn default_require_mention() -> bool {
    true
}
fn default_true() -> bool {
    true
}

impl Default for InboundPolicy {
    /// Fail-closed posture — denies all inbound until configured.
    fn default() -> Self {
        Self {
            dm: DmPolicy::Allowlist,
            group: GroupPolicy::Disabled,
            require_mention: true,
            dm_allowlist: Vec::new(),
            group_allowlist: Vec::new(),
            sender_allowlist: Vec::new(),
            group_sessions_per_user: true,
            thread_sessions_per_user: false,
            tools: ChannelToolPosture::Conversational,
            tool_workspace_root: None,
            ack: AckMode::Off,
            acknowledge_open_admission: Vec::new(),
        }
    }
}

/// Outcome of the access gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// Message is permitted to proceed.
    Allow,
    /// Message is rejected. `reason` is a short, non-PII-leaking tag for
    /// logging — it never embeds sender ids or message content.
    Deny { reason: String },
}

/// The allowlist entry that matches every id. Named so the startup gate in
/// [`open_admissions`] tests for exactly what [`permits`] honours, rather
/// than for a second, drift-prone spelling of the same thing.
pub const WILDCARD: &str = "*";

/// True iff `list` permits `id`: contains the [`WILDCARD`] entry, or contains
/// `id` exactly. An empty list permits nothing (fail-closed).
fn permits(list: &[String], id: &str) -> bool {
    list.iter().any(|e| e == WILDCARD || e == id)
}

/// True iff `list` permits an arbitrary id nobody enumerated — i.e. it holds
/// the [`WILDCARD`]. This is `permits` over the set of all ids.
fn permits_anyone(list: &[String]) -> bool {
    list.iter().any(|e| e == WILDCARD)
}

/// One way a channel's `[inbound]` config admits an unbounded set of senders.
///
/// "Unbounded" means [`decide_access`] returns [`AccessDecision::Allow`] for a
/// `sender_id` the config never names — so the operator cannot say who can
/// drive the agent, and the answer is "whoever finds the bot".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAdmission {
    /// The `[inbound]` key at fault, spelled as it appears in the TOML. This
    /// is also the token an operator lists in
    /// [`InboundPolicy::acknowledge_open_admission`] to accept it.
    pub field: &'static str,
    /// What that key is set to, as the operator wrote it.
    pub found: String,
    /// The exact narrower configuration to use instead.
    pub remedy: String,
}

/// Every admits-everyone shape in `policy`, ignoring any acknowledgement.
///
/// Each arm mirrors a branch of [`decide_access`] that reaches
/// [`AccessDecision::Allow`] without consulting the sender id, so this list
/// cannot claim a shape is open that the gate would actually deny:
///
/// - `dm = "open"` — the `DmPolicy::Open` arm allows unconditionally.
/// - `dm = "allowlist"` with `"*"` in `dm_allowlist` — `permits` matches any.
///   `dm = "disabled"` / `"pairing"` deny BEFORE the allowlist is consulted,
///   so a `"*"` under those is inert and is deliberately not flagged.
/// - `group = "open"` — the `GroupPolicy::Open` arm allows unconditionally.
/// - `group = "allowlist"` with `"*"` in `sender_allowlist` AND a non-empty
///   `group_allowlist` — any sender in a reachable conversation is admitted.
///   With an EMPTY `group_allowlist` nothing matches the conversation test
///   first, so the sender wildcard admits nobody and is not flagged.
///
/// A `"*"` in `group_allowlist` alone is NOT listed: senders are still gated
/// by `sender_allowlist`, so the admitted set stays enumerated. When both are
/// `"*"` the `sender_allowlist` finding already fires.
pub fn open_admissions(policy: &InboundPolicy) -> Vec<OpenAdmission> {
    let mut out = Vec::new();

    match policy.dm {
        DmPolicy::Open => out.push(OpenAdmission {
            field: "dm",
            found: "dm = \"open\"".into(),
            remedy: "dm = \"allowlist\" with dm_allowlist = [\"<sender id>\", ...] naming each \
                     person permitted to DM this bot"
                .into(),
        }),
        DmPolicy::Allowlist if permits_anyone(&policy.dm_allowlist) => out.push(OpenAdmission {
            field: "dm_allowlist",
            found: "dm_allowlist = [\"*\"]".into(),
            remedy: "dm_allowlist = [\"<sender id>\", ...] naming each permitted sender id \
                     instead of the \"*\" wildcard"
                .into(),
        }),
        _ => {}
    }

    match policy.group {
        GroupPolicy::Open => out.push(OpenAdmission {
            field: "group",
            found: "group = \"open\"".into(),
            remedy: "group = \"allowlist\" with group_allowlist = [\"<conversation id>\", ...] \
                     and sender_allowlist = [\"<sender id>\", ...]"
                .into(),
        }),
        GroupPolicy::Allowlist
            if permits_anyone(&policy.sender_allowlist) && !policy.group_allowlist.is_empty() =>
        {
            out.push(OpenAdmission {
                field: "sender_allowlist",
                found: "sender_allowlist = [\"*\"]".into(),
                remedy: "sender_allowlist = [\"<sender id>\", ...] naming each permitted sender \
                         instead of the \"*\" wildcard"
                    .into(),
            })
        }
        _ => {}
    }

    out
}

/// [`open_admissions`] minus the shapes this channel's
/// [`InboundPolicy::acknowledge_open_admission`] names.
pub fn unacknowledged_open_admissions(policy: &InboundPolicy) -> Vec<OpenAdmission> {
    open_admissions(policy)
        .into_iter()
        .filter(|f| {
            !policy
                .acknowledge_open_admission
                .iter()
                .any(|a| a == f.field)
        })
        .collect()
}

/// Refusal to start over one or more channels that admit everyone.
///
/// Carries every offending `(channel, finding)` pair rather than the first,
/// so an operator fixes the whole configuration in one pass instead of
/// discovering the next one on the next failed start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAdmissionRefusal {
    /// `(channel name, finding)`, in channel order.
    pub findings: Vec<(String, OpenAdmission)>,
}

impl std::fmt::Display for OpenAdmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "refusing to start: {} inbound channel configuration(s) admit an unbounded set of \
             senders — anyone who can reach the platform could drive this agent.",
            self.findings.len()
        )?;
        for (channel, finding) in &self.findings {
            writeln!(
                f,
                "  channel {channel:?}: [inbound] {} — {}. Use instead: {}",
                finding.found,
                match finding.field {
                    "dm" | "dm_allowlist" => "every DM from every account is admitted",
                    "group" => "every group and channel message is admitted",
                    _ => "every sender in an allowlisted conversation is admitted",
                },
                finding.remedy
            )?;
        }
        write!(
            f,
            "If a channel is genuinely meant to be open, acknowledge each open field BY NAME in \
             that channel's own file (<profile home>/channels/<name>.toml):\n    [inbound]\n    \
             acknowledge_open_admission = [{}]\nThat key is read only from the profile-scoped \
             channel file — a project-local .wayland-core.toml cannot set it — and it must name \
             every open field, so opening another one refuses again.",
            self.findings
                .iter()
                .map(|(_, finding)| format!("{:?}", finding.field))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for OpenAdmissionRefusal {}

/// THE STARTUP GATE. `Err` iff any channel admits an unbounded set of senders
/// without acknowledging it.
///
/// Callers must refuse to start (or, on a reload, refuse to swap) rather than
/// warn: a warning leaves the dangerous configuration reachable, which is the
/// state this function exists to make impossible.
///
/// Channels are checked whether or not they are `enabled`. A disabled channel
/// admits nobody today, but its policy is loaded into the live registry all
/// the same, and `enabled = true` is a one-word edit away — refusing now is
/// what keeps the acknowledgement contemporaneous with the decision.
pub fn refuse_open_admission<'a>(
    channels: impl IntoIterator<Item = (&'a str, &'a InboundPolicy)>,
) -> Result<(), OpenAdmissionRefusal> {
    let findings: Vec<(String, OpenAdmission)> = channels
        .into_iter()
        .flat_map(|(name, policy)| {
            unacknowledged_open_admissions(policy)
                .into_iter()
                .map(move |f| (name.to_string(), f))
        })
        .collect();
    if findings.is_empty() {
        Ok(())
    } else {
        Err(OpenAdmissionRefusal { findings })
    }
}

/// The fail-closed access gate. Decides whether `msg` is permitted under
/// `policy`, without considering mention-gating (that lives in
/// admission). Reasons are short, content-free tags.
pub fn decide_access(msg: &IncomingMessage, policy: &InboundPolicy) -> AccessDecision {
    match msg.chat_type {
        ChatType::Direct => match policy.dm {
            DmPolicy::Disabled => AccessDecision::Deny {
                reason: "dms disabled".into(),
            },
            DmPolicy::Open => AccessDecision::Allow,
            DmPolicy::Allowlist => {
                if permits(&policy.dm_allowlist, &msg.sender_id) {
                    AccessDecision::Allow
                } else {
                    AccessDecision::Deny {
                        reason: "sender not in dm allowlist".into(),
                    }
                }
            }
            DmPolicy::Pairing => AccessDecision::Deny {
                reason: "pairing not yet implemented".into(),
            },
        },
        ChatType::Group | ChatType::Channel => match policy.group {
            GroupPolicy::Disabled => AccessDecision::Deny {
                reason: "groups disabled".into(),
            },
            GroupPolicy::Open => AccessDecision::Allow,
            GroupPolicy::Allowlist => {
                if !permits(&policy.group_allowlist, &msg.conversation_id) {
                    AccessDecision::Deny {
                        reason: "group not allowlisted".into(),
                    }
                } else if !permits(&policy.sender_allowlist, &msg.sender_id) {
                    AccessDecision::Deny {
                        reason: "sender not in group allowlist".into(),
                    }
                } else {
                    AccessDecision::Allow
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm_from(sender: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new("id1", "conv1", "Alice", "hi", 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Direct;
        m
    }

    fn group_from(conv: &str, sender: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new("id1", conv, "Alice", "hi", 0);
        m.sender_id = sender.into();
        m.chat_type = ChatType::Group;
        m
    }

    #[test]
    fn default_policy_is_fail_closed() {
        let p = InboundPolicy::default();
        assert_eq!(p.dm, DmPolicy::Allowlist);
        assert_eq!(p.group, GroupPolicy::Disabled);
        assert!(p.require_mention);
        assert!(p.dm_allowlist.is_empty());
        assert!(p.group_allowlist.is_empty());
        assert!(p.sender_allowlist.is_empty());
        assert!(p.group_sessions_per_user);
        assert!(!p.thread_sessions_per_user);
        // Tool posture defaults to the safe, no-host-access floor.
        assert_eq!(p.tools, ChannelToolPosture::Conversational);
        assert!(p.tool_workspace_root.is_none());
        // A DM under the default policy is denied (empty allowlist).
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
        // A group message under the default policy is denied (disabled).
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    // ---- DM ----

    #[test]
    fn dm_empty_allowlist_denies() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec![],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_wildcard_allows_anyone() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("anyone"), &p), AccessDecision::Allow);
    }

    #[test]
    fn dm_exact_id_allows_only_that_id() {
        let p = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["u1".into()],
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("u1"), &p), AccessDecision::Allow);
        assert!(matches!(
            decide_access(&dm_from("u2"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_open_allows_all() {
        let p = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        assert_eq!(decide_access(&dm_from("u1"), &p), AccessDecision::Allow);
    }

    #[test]
    fn dm_disabled_denies_even_with_wildcard() {
        let p = InboundPolicy {
            dm: DmPolicy::Disabled,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&dm_from("u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn dm_pairing_denies_with_specific_reason() {
        let p = InboundPolicy {
            dm: DmPolicy::Pairing,
            dm_allowlist: vec!["*".into()],
            ..Default::default()
        };
        match decide_access(&dm_from("u1"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("pairing")),
            AccessDecision::Allow => panic!("pairing must deny until implemented"),
        }
    }

    // ---- Group ----

    #[test]
    fn group_disabled_denies_even_with_wildcards() {
        let p = InboundPolicy {
            group: GroupPolicy::Disabled,
            group_allowlist: vec!["*".into()],
            sender_allowlist: vec!["*".into()],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn group_open_allows() {
        let p = InboundPolicy {
            group: GroupPolicy::Open,
            ..Default::default()
        };
        assert_eq!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Allow
        );
    }

    #[test]
    fn group_allowlist_requires_both_group_and_sender() {
        let p = InboundPolicy {
            group: GroupPolicy::Allowlist,
            group_allowlist: vec!["g1".into()],
            sender_allowlist: vec!["u1".into()],
            ..Default::default()
        };
        // Both match -> allow.
        assert_eq!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Allow
        );
        // Group not allowlisted -> deny.
        match decide_access(&group_from("g2", "u1"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("group")),
            AccessDecision::Allow => panic!("non-allowlisted group must deny"),
        }
        // Sender not allowlisted -> deny.
        match decide_access(&group_from("g1", "u2"), &p) {
            AccessDecision::Deny { reason } => assert!(reason.contains("sender")),
            AccessDecision::Allow => panic!("non-allowlisted sender must deny"),
        }
    }

    #[test]
    fn group_allowlist_empty_lists_deny() {
        let p = InboundPolicy {
            group: GroupPolicy::Allowlist,
            group_allowlist: vec![],
            sender_allowlist: vec![],
            ..Default::default()
        };
        assert!(matches!(
            decide_access(&group_from("g1", "u1"), &p),
            AccessDecision::Deny { .. }
        ));
    }

    #[test]
    fn channel_chat_type_uses_group_policy() {
        let p = InboundPolicy {
            group: GroupPolicy::Open,
            ..Default::default()
        };
        let mut m = group_from("c1", "u1");
        m.chat_type = ChatType::Channel;
        assert_eq!(decide_access(&m, &p), AccessDecision::Allow);
    }

    #[test]
    fn tool_posture_parses_and_defaults() {
        // Absent `tools` key -> Conversational (the fail-closed default),
        // even though `deny_unknown_fields` is set.
        let p: InboundPolicy = toml::from_str("dm = \"open\"").unwrap();
        assert_eq!(p.tools, ChannelToolPosture::Conversational);
        // Each posture string round-trips.
        let w: InboundPolicy = toml::from_str(
            "dm = \"open\"\ntools = \"workspace\"\ntool_workspace_root = \"/srv/agent\"",
        )
        .unwrap();
        assert_eq!(w.tools, ChannelToolPosture::Workspace);
        assert_eq!(w.tool_workspace_root.as_deref(), Some("/srv/agent"));
        let f: InboundPolicy = toml::from_str("dm = \"open\"\ntools = \"full\"").unwrap();
        assert_eq!(f.tools, ChannelToolPosture::Full);
    }

    #[test]
    fn ack_mode_parses_and_defaults() {
        let p: InboundPolicy = toml::from_str("dm = \"open\"").unwrap();
        assert_eq!(p.ack, AckMode::Off);
        assert!(!p.ack.reactions() && !p.ack.typing());
        let b: InboundPolicy = toml::from_str("dm = \"open\"\nack = \"both\"").unwrap();
        assert_eq!(b.ack, AckMode::Both);
        assert!(b.ack.reactions() && b.ack.typing());
        let r: InboundPolicy = toml::from_str("dm = \"open\"\nack = \"reactions\"").unwrap();
        assert!(r.ack.reactions() && !r.ack.typing());
    }

    #[test]
    fn permits_helper_semantics() {
        assert!(!permits(&[], "x"), "empty list permits nothing");
        assert!(permits(&["*".into()], "x"), "wildcard permits any");
        assert!(permits(&["x".into()], "x"), "exact match permits");
        assert!(!permits(&["y".into()], "x"), "non-match denies");
    }
}
