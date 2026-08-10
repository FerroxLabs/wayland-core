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
/// allow a specific person, add their stable `sender_id` to
/// `dm_allowlist`.
///
/// **`dm_allowlist = ["*"]` is refused at startup.** So are `dm = "open"`,
/// `group = "open"` and a `"*"` `sender_allowlist` over a non-empty
/// `group_allowlist`: each of them admits senders nobody named. See
/// [`refuse_open_admission`] for the gate and
/// [`InboundPolicy::acknowledge_open_admission`] for the deliberate opt-out.
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
    /// Opt-out from the admits-everyone startup refusal, bound to the exact
    /// configuration it consents to.
    ///
    /// Each entry is a TOKEN naming both the `[inbound]` key that is open and
    /// the value that opens it — `"dm=open"`, `"dm_allowlist=*"`,
    /// `"group=open"`, `"sender_allowlist=*"`. See [`open_admissions`],
    /// [`required_acknowledgement`] and [`refuse_open_admission`].
    ///
    /// # The list must match the open configuration EXACTLY
    ///
    /// Not "cover" it — match it. Both directions are refusals:
    ///
    /// - an open shape with no token is unacknowledged, and
    /// - a token naming a shape that is NOT open right now is stale.
    ///
    /// The second direction is the load-bearing one. Without it a bare
    /// field-name list is just `allow_open = true` spelled at length: an
    /// operator could write every field name into a channel that is currently
    /// bounded, and that one edit would silently consent to every way the
    /// channel might be opened afterwards, forever. Refusing a stale token
    /// means a token can only be written while the shape it names is already
    /// live — so the acknowledgement is contemporaneous with the decision by
    /// construction, and consenting to "open to my team's group" cannot
    /// quietly become consent to "and to DMs from anyone".
    ///
    /// A configuration change therefore always fails CLOSED: the process
    /// refuses to start and the refusal prints the token list that matches
    /// the new configuration, for the operator to accept deliberately.
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
    /// The `[inbound]` key at fault, spelled as it appears in the TOML.
    pub field: &'static str,
    /// What that key is set to, as the operator wrote it.
    pub found: String,
    /// The token that acknowledges THIS shape and no other, for
    /// [`InboundPolicy::acknowledge_open_admission`]: `<field>=<the value
    /// that opens it>`.
    ///
    /// The value is part of the token on purpose. A token naming only the
    /// field would still be true of that field's NEXT open value, so an
    /// acknowledgement would outlive the configuration it was written for.
    pub token: &'static str,
    /// The exact narrower configuration to use instead.
    pub remedy: String,
}

impl OpenAdmission {
    /// What this shape means for the operator, in one clause. Written for the
    /// refusal message, where "your config is open" is not actionable but
    /// "every DM from every account is admitted" is.
    pub fn consequence(&self) -> &'static str {
        match self.field {
            "dm" | "dm_allowlist" => "every DM from every account is admitted",
            "group" => "every group and channel message is admitted",
            _ => "every sender in an allowlisted conversation is admitted",
        }
    }
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
            token: "dm=open",
            remedy: "dm = \"allowlist\" with dm_allowlist = [\"<sender id>\", ...] naming each \
                     person permitted to DM this bot"
                .into(),
        }),
        DmPolicy::Allowlist if permits_anyone(&policy.dm_allowlist) => out.push(OpenAdmission {
            field: "dm_allowlist",
            found: "dm_allowlist = [\"*\"]".into(),
            token: "dm_allowlist=*",
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
            token: "group=open",
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
                token: "sender_allowlist=*",
                remedy: "sender_allowlist = [\"<sender id>\", ...] naming each permitted sender \
                         instead of the \"*\" wildcard"
                    .into(),
            })
        }
        _ => {}
    }

    out
}

/// The `acknowledge_open_admission` value that matches `policy` EXACTLY as it
/// stands: one [`OpenAdmission::token`] per open shape, in [`open_admissions`]
/// order.
///
/// Empty when nothing is open — in which case the correct file has no
/// `acknowledge_open_admission` key at all, because a token that names
/// nothing open is a stale consent, not a harmless leftover.
pub fn required_acknowledgement(policy: &InboundPolicy) -> Vec<&'static str> {
    open_admissions(policy)
        .into_iter()
        .map(|f| f.token)
        .collect()
}

/// [`open_admissions`] minus the shapes this channel's
/// [`InboundPolicy::acknowledge_open_admission`] names.
///
/// One half of [`open_admission_faults`]; on its own it cannot see a stale
/// acknowledgement, which is why the gate uses the other function.
pub fn unacknowledged_open_admissions(policy: &InboundPolicy) -> Vec<OpenAdmission> {
    open_admissions(policy)
        .into_iter()
        .filter(|f| {
            !policy
                .acknowledge_open_admission
                .iter()
                .any(|a| a == f.token)
        })
        .collect()
}

/// Why the startup gate refuses one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAdmissionFault {
    /// The channel admits everyone through this shape, and nothing in
    /// [`InboundPolicy::acknowledge_open_admission`] names it.
    Unacknowledged(OpenAdmission),
    /// [`InboundPolicy::acknowledge_open_admission`] carries a token that
    /// names no shape this channel currently has open.
    ///
    /// This is the arm that bounds an acknowledgement in TIME. Without it the
    /// key degenerates into the `allow_open = true` boolean the design
    /// rejects: an operator could pre-arm a currently-bounded channel with
    /// every token, and that single edit would consent in advance to every
    /// way the channel might later be opened. Because a stale token refuses,
    /// a token can only be written while the shape it names is already live.
    ///
    /// It fails CLOSED: the refusal reports what no longer matches and prints
    /// the list that does, so the operator re-acknowledges deliberately.
    Stale {
        /// The token as the operator wrote it.
        token: String,
    },
}

/// Everything wrong with `policy`'s acknowledgement: every open shape that is
/// unacknowledged, then every acknowledgement that names nothing open.
///
/// Empty iff `acknowledge_open_admission` names EXACTLY the open
/// configuration — the only state the gate accepts. "Covers" is not enough,
/// because a list that covers more than is open is a consent to
/// configurations nobody has looked at yet.
pub fn open_admission_faults(policy: &InboundPolicy) -> Vec<OpenAdmissionFault> {
    let open = open_admissions(policy);
    let mut faults: Vec<OpenAdmissionFault> = unacknowledged_open_admissions(policy)
        .into_iter()
        .map(OpenAdmissionFault::Unacknowledged)
        .collect();
    for ack in &policy.acknowledge_open_admission {
        if !open.iter().any(|f| f.token == ack) {
            faults.push(OpenAdmissionFault::Stale { token: ack.clone() });
        }
    }
    faults
}

/// One refused channel: why, and the acknowledgement that would match it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOpenAdmission {
    /// The channel whose file is at fault.
    pub channel: String,
    /// Why, in [`open_admission_faults`] order.
    pub faults: Vec<OpenAdmissionFault>,
    /// [`required_acknowledgement`] for this channel — what the operator must
    /// write to consent to the configuration as it stands. Empty means the
    /// key must be removed.
    pub required_acknowledgement: Vec<&'static str>,
}

/// Refusal to start over one or more channels.
///
/// Carries every offending channel rather than the first, so an operator
/// fixes the whole configuration in one pass instead of discovering the next
/// one on the next failed start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAdmissionRefusal {
    /// Every refused channel, in channel order.
    pub channels: Vec<ChannelOpenAdmission>,
}

impl std::fmt::Display for OpenAdmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "refusing to start: {} inbound channel configuration(s) do not match their \
             acknowledgement of open admission.",
            self.channels.len()
        )?;
        for refused in &self.channels {
            let channel = &refused.channel;
            for fault in &refused.faults {
                match fault {
                    OpenAdmissionFault::Unacknowledged(finding) => writeln!(
                        f,
                        "  channel {channel:?}: [inbound] {} — {}, and nothing acknowledges it. \
                         Use instead: {}",
                        finding.found,
                        finding.consequence(),
                        finding.remedy
                    )?,
                    OpenAdmissionFault::Stale { token } => writeln!(
                        f,
                        "  channel {channel:?}: acknowledge_open_admission names {token:?}, but \
                         this channel is not open that way any more. An acknowledgement consents \
                         to ONE configuration; when that configuration changes it stops applying \
                         rather than silently covering whatever replaced it.",
                    )?,
                }
            }
            if refused.required_acknowledgement.is_empty() {
                writeln!(
                    f,
                    "  channel {channel:?}: nothing in this channel is open, so remove the \
                     acknowledge_open_admission key from <profile home>/channels/{channel}.toml \
                     entirely."
                )?;
            } else {
                writeln!(
                    f,
                    "  channel {channel:?}: to keep this configuration, acknowledge exactly what \
                     is open in <profile home>/channels/{channel}.toml:\n      [inbound]\n      \
                     acknowledge_open_admission = [{}]",
                    refused
                        .required_acknowledgement
                        .iter()
                        .map(|t| format!("{t:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )?;
            }
        }
        write!(
            f,
            "Each token names the open field AND the value that opens it, so it can only be \
             written while that exact configuration is live — opening another field, or changing \
             one, refuses again instead of being covered by the old consent. This key is read \
             only from the profile-scoped channel file; a project-local .wayland-core.toml \
             cannot set it."
        )
    }
}

impl std::error::Error for OpenAdmissionRefusal {}

/// THE STARTUP GATE. `Err` iff any channel's acknowledgement does not name
/// exactly the set of admits-everyone shapes that channel currently has.
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
    let refused: Vec<ChannelOpenAdmission> = channels
        .into_iter()
        .filter_map(|(name, policy)| {
            let faults = open_admission_faults(policy);
            if faults.is_empty() {
                return None;
            }
            Some(ChannelOpenAdmission {
                channel: name.to_string(),
                faults,
                required_acknowledgement: required_acknowledgement(policy),
            })
        })
        .collect();
    if refused.is_empty() {
        Ok(())
    } else {
        Err(OpenAdmissionRefusal { channels: refused })
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

    // ---- The acknowledgement is bound to the configuration it names ----

    /// A channel that is bounded today, with every token pre-written.
    fn preacked_but_bounded() -> InboundPolicy {
        InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U-NAMED".into()],
            group: GroupPolicy::Disabled,
            acknowledge_open_admission: vec![
                "dm=open".into(),
                "group=open".into(),
                "dm_allowlist=*".into(),
                "sender_allowlist=*".into(),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_blanket_pre_acknowledgement_cannot_be_written_in_the_first_place() {
        // This is the whole defect, at the level of the pure gate: if a bounded
        // channel accepts tokens for shapes it does not have, that file is a
        // standing consent to every future opening, and the field-name list is
        // just `allow_open = true` spelled out.
        let refusal = refuse_open_admission([("preacked", &preacked_but_bounded())])
            .expect_err("pre-arming a bounded channel must refuse, not be silently accepted");
        let refused = &refusal.channels[0];
        assert_eq!(refused.channel, "preacked");
        assert_eq!(
            refused.faults.len(),
            4,
            "each token names a shape this channel does not have; every one is stale: {:?}",
            refused.faults
        );
        assert!(
            refused
                .faults
                .iter()
                .all(|f| matches!(f, OpenAdmissionFault::Stale { .. })),
            "nothing here is OPEN — the faults are all stale consents: {:?}",
            refused.faults
        );
        assert!(
            refused.required_acknowledgement.is_empty(),
            "the channel is bounded, so the correct file has no acknowledgement at all"
        );
        let msg = refusal.to_string();
        assert!(
            msg.contains("remove the acknowledge_open_admission key"),
            "the operator must be told what to do; got: {msg}"
        );
    }

    #[test]
    fn a_matching_acknowledgement_is_accepted_and_a_superset_is_not() {
        // POSITIVE CONTROL first, or the test above is satisfied by a gate that
        // refuses every acknowledgement.
        let exact = InboundPolicy {
            dm: DmPolicy::Open,
            acknowledge_open_admission: vec!["dm=open".into()],
            ..Default::default()
        };
        assert_eq!(required_acknowledgement(&exact), vec!["dm=open"]);
        refuse_open_admission([("exact", &exact)])
            .expect("an acknowledgement that names exactly what is open must be accepted");

        // One extra token — the smallest possible pre-arm — must refuse.
        let superset = InboundPolicy {
            acknowledge_open_admission: vec!["dm=open".into(), "group=open".into()],
            ..exact.clone()
        };
        let refusal = refuse_open_admission([("superset", &superset)])
            .expect_err("a token for a shape that is not open must refuse");
        assert_eq!(
            refusal.channels[0].faults,
            vec![OpenAdmissionFault::Stale {
                token: "group=open".into()
            }],
            "and only the extra token is at fault"
        );
    }

    #[test]
    fn changing_which_shape_is_open_invalidates_the_old_acknowledgement() {
        // The consent said "dm = open". The operator swaps that for a different
        // open shape ON THE SAME FIELD FAMILY. The old consent must not carry
        // over: it named a configuration this file no longer has.
        let changed = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec![WILDCARD.into()],
            acknowledge_open_admission: vec!["dm=open".into()],
            ..Default::default()
        };
        let refusal = refuse_open_admission([("changed", &changed)])
            .expect_err("a different open shape is a different decision");
        let faults = &refusal.channels[0].faults;
        assert!(
            faults.iter().any(|f| matches!(
                f,
                OpenAdmissionFault::Unacknowledged(a) if a.token == "dm_allowlist=*"
            )),
            "the new shape is unacknowledged: {faults:?}"
        );
        assert!(
            faults.contains(&OpenAdmissionFault::Stale {
                token: "dm=open".into()
            }),
            "and the old consent is stale: {faults:?}"
        );
        assert_eq!(
            refusal.channels[0].required_acknowledgement,
            vec!["dm_allowlist=*"],
            "the refusal must print the list that matches the NEW configuration"
        );
    }

    #[test]
    fn narrowing_a_channel_refuses_until_the_stale_consent_is_removed() {
        // Fails CLOSED, deliberately, even though the new configuration is
        // SAFER. Accepting a leftover consent silently is exactly how a token
        // written for a bounded moment ends up covering an open one.
        let narrowed = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U-NAMED".into()],
            acknowledge_open_admission: vec!["dm=open".into()],
            ..Default::default()
        };
        let refusal = refuse_open_admission([("narrowed", &narrowed)])
            .expect_err("a leftover consent must be cleared deliberately, not honoured silently");
        let msg = refusal.to_string();
        assert!(
            msg.contains("\"dm=open\"") && msg.contains("not open that way any more"),
            "the refusal must say WHICH consent no longer matches; got: {msg}"
        );
    }

    #[test]
    fn no_near_miss_spelling_of_a_token_acknowledges_anything() {
        // ATTACK VARIANT. If the match were fuzzy — trimmed, case-folded, or
        // prefix-based — an operator (or a config generator) could write a
        // token that covers more than one shape, and the binding would leak
        // back to a field-name list. Every one of these must leave the channel
        // BOTH unacknowledged and carrying a stale token: fail-closed in both
        // directions at once.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        for spelling in [
            "dm",                 // the old field-name form
            "dm=Open",            // case
            "DM=OPEN",            //
            "dm = open",          // spaces
            " dm=open",           // leading space
            "dm=open ",           // trailing space
            "dm=\"open\"",        // TOML-ish quoting
            "dm=open,group=open", // two in one entry
            "dm=*",               // wrong value
            "*",                  // a wildcard acknowledgement
            "",                   // empty
        ] {
            let policy = InboundPolicy {
                acknowledge_open_admission: vec![spelling.to_string()],
                ..open.clone()
            };
            let faults = open_admission_faults(&policy);
            assert!(
                faults.iter().any(|f| matches!(
                    f,
                    OpenAdmissionFault::Unacknowledged(a) if a.token == "dm=open"
                )),
                "{spelling:?} must NOT acknowledge dm = \"open\"; faults were {faults:?}"
            );
            assert!(
                faults.contains(&OpenAdmissionFault::Stale {
                    token: spelling.to_string()
                }),
                "{spelling:?} names no open shape, so it must also be reported stale; faults \
                 were {faults:?}"
            );
        }
    }

    #[test]
    fn the_gate_flags_every_config_that_admits_an_unnamed_sender() {
        // ATTACK VARIANT, and the one that could not be found by re-reading
        // the gate: enumerate the whole small configuration space and ask
        // `decide_access` itself — the function that actually admits people —
        // whether a sender NOBODY named gets in. Anything it admits and
        // `open_admissions` does not flag is a shape the acknowledgement can
        // never be bound to, i.e. an unbounded channel that starts silently.
        //
        // This is deliberately not seeded from the implementation's own list
        // of open shapes; that is how a self-certifying test gets written.
        const DMS: [DmPolicy; 4] = [
            DmPolicy::Open,
            DmPolicy::Allowlist,
            DmPolicy::Disabled,
            DmPolicy::Pairing,
        ];
        const GROUPS: [GroupPolicy; 3] = [
            GroupPolicy::Open,
            GroupPolicy::Allowlist,
            GroupPolicy::Disabled,
        ];
        let lists: [Vec<String>; 3] = [vec![], vec!["*".into()], vec!["U1".into()]];
        let convs: [Vec<String>; 3] = [vec![], vec!["*".into()], vec!["G1".into()]];

        // A sender and a conversation that appear in NO list above.
        let stranger = "U-STRANGER-NEVER-NAMED";
        let mut checked = 0usize;
        let mut flagged = 0usize;
        for dm in &DMS {
            for group in &GROUPS {
                for dm_allowlist in &lists {
                    for group_allowlist in &convs {
                        for sender_allowlist in &lists {
                            let policy = InboundPolicy {
                                dm: dm.clone(),
                                group: group.clone(),
                                dm_allowlist: dm_allowlist.clone(),
                                group_allowlist: group_allowlist.clone(),
                                sender_allowlist: sender_allowlist.clone(),
                                ..Default::default()
                            };
                            let admits_stranger = [
                                dm_from(stranger),
                                group_from("G1", stranger),
                                group_from("G-STRANGER-NEVER-NAMED", stranger),
                            ]
                            .iter()
                            .any(|m| decide_access(m, &policy) == AccessDecision::Allow);

                            let open = open_admissions(&policy);
                            checked += 1;
                            if admits_stranger {
                                flagged += 1;
                                assert!(
                                    !open.is_empty(),
                                    "this config admits {stranger}, whom it never names, and the \
                                     gate does not flag it: {policy:?}"
                                );
                            } else {
                                assert!(
                                    open.is_empty(),
                                    "this config admits NOBODY unnamed, so flagging it would be \
                                     an over-refusal an operator cannot satisfy: {policy:?} -> \
                                     {open:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 4 * 3 * 3 * 3 * 3, "the whole space was walked");
        assert!(
            flagged > 0 && flagged < checked,
            "control: the corpus must contain BOTH open and bounded configurations, or the \
             assertions above are vacuous; got {flagged} open of {checked}"
        );
    }

    #[test]
    fn every_open_shape_has_a_token_and_the_tokens_are_distinct() {
        // If two shapes shared a token, acknowledging one would acknowledge the
        // other — the binding would be back to a field-name list. Drive every
        // shape through `open_admissions` rather than listing the constants, so
        // a new shape added without a token cannot slip past.
        let shapes = [
            InboundPolicy {
                dm: DmPolicy::Open,
                ..Default::default()
            },
            InboundPolicy {
                dm: DmPolicy::Allowlist,
                dm_allowlist: vec![WILDCARD.into()],
                ..Default::default()
            },
            InboundPolicy {
                group: GroupPolicy::Open,
                ..Default::default()
            },
            InboundPolicy {
                group: GroupPolicy::Allowlist,
                group_allowlist: vec!["G1".into()],
                sender_allowlist: vec![WILDCARD.into()],
                ..Default::default()
            },
        ];
        let mut tokens: Vec<&str> = Vec::new();
        for policy in &shapes {
            let found = open_admissions(policy);
            assert_eq!(found.len(), 1, "each fixture is exactly one open shape");
            let token = found[0].token;
            assert!(
                token.contains('=') && token.starts_with(found[0].field),
                "a token must name the field AND the value that opens it; got {token:?}"
            );
            assert!(
                !tokens.contains(&token),
                "tokens must be distinct: {token:?}"
            );
            tokens.push(token);
            // And the token it advertises must be the token that satisfies it.
            let acknowledged = InboundPolicy {
                acknowledge_open_admission: vec![token.to_string()],
                ..policy.clone()
            };
            assert!(
                open_admission_faults(&acknowledged).is_empty(),
                "the advertised token must be the one the gate accepts: {token:?}"
            );
        }
        assert_eq!(tokens.len(), 4);
    }
}
