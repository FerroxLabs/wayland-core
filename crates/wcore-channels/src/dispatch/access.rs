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

use crate::config::ChannelConfig;
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
    /// Holds EXACTLY ONE entry when this channel admits an unbounded set of
    /// senders, and NO entry otherwise. That entry is the channel's
    /// [`admission_shape_token`] — a canonical rendering of every setting that
    /// decides who is admitted and what they can trigger. See
    /// [`open_admissions`], [`required_acknowledgement`] and
    /// [`refuse_open_admission`].
    ///
    /// # Why the token is the WHOLE shape and not a list of open fields
    ///
    /// The first cut of this key took one token per open shape
    /// (`"dm=open"`, `"sender_allowlist=*"`, …). That is a hand-picked field
    /// list, and consent bound to a hand-picked field list is bypassed by
    /// changing a field nobody picked:
    ///
    /// ```toml
    /// group            = "allowlist"
    /// group_allowlist  = ["G1"]   # <- change this ONE field to ["*"]
    /// sender_allowlist = ["*"]
    /// acknowledge_open_admission = ["sender_allowlist=*"]
    /// ```
    ///
    /// The admitted set grows from "anyone who can reach G1" to "anyone,
    /// anywhere", and the old token set does not change, so nothing refused.
    /// The same hole opened through `enabled`: a switched-off channel admits
    /// nobody, so a consent written against it was never contemporaneous with
    /// anything, and one later word (`enabled = true`) admitted everyone.
    ///
    /// So the token is derived from the whole shape — see [`SHAPE_FIELDS`] for
    /// the fixed part and for what is deliberately outside it. Any change to
    /// any of those settings changes the token and demands fresh consent.
    ///
    /// The same hole opened a THIRD time through `[options]`, which is not in
    /// `[inbound]` at all: four adapters keep their own admission filter there
    /// and an ABSENT one admits everyone, so deleting one line widened the
    /// admitted set while changing no token. The shape therefore covers
    /// `platform` and every `[options]` key too — see [`AdmissionShape`] for
    /// why that is the whole table rather than a declared list of keys.
    ///
    /// # What is cosmetic, precisely
    ///
    /// Allowlists are normalised before rendering: sorted, de-duplicated, and
    /// (in `[inbound]`) collapsed to `*` when they hold the wildcard, which
    /// subsumes every other entry. `["G1","G2"]` and `["G2","G1","G2"]`
    /// therefore produce the same token, because they admit the same
    /// principals. `["G1"]` and `["*"]` do not. Refusing a reordering would
    /// teach operators that the gate cries wolf, and an operator who stops
    /// reading refusals reaches for the nuclear option — which is the failure
    /// mode this key exists to prevent.
    ///
    /// `[options]` lists are sorted and de-duplicated too, but the wildcard is
    /// NOT collapsed there: no adapter option list honours `"*"`, so `["*"]`
    /// and `["*","111"]` admit different sets.
    ///
    /// # Both directions are refusals
    ///
    /// - an open shape with no token is unacknowledged;
    /// - a token present while nothing is open is stale;
    /// - a token naming a DIFFERENT shape than the live one is a changed shape,
    ///   and the refusal prints field-by-field what changed;
    /// - more than one entry is not the exact match this key requires.
    ///
    /// The stale direction is what bounds a consent in TIME: a token can only
    /// be written while the shape it names is already live, so the
    /// acknowledgement is contemporaneous with the decision by construction.
    /// Without it the key degenerates into the `allow_open = true` boolean the
    /// design rejects.
    ///
    /// A configuration change therefore always fails CLOSED: the process
    /// refuses to start and the refusal prints the token that matches the new
    /// configuration, for the operator to accept deliberately.
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
///
/// # Both matches are exhaustive by NAME, and neither has a `_` arm
///
/// This function's answer for a policy is "this admits senders nobody named"
/// or "this does not", and a catch-all answers the second for every variant
/// that has not been thought about yet — silently, at compile time, with no
/// test able to see it. So every posture is spelled out. Adding a
/// [`DmPolicy`] or [`GroupPolicy`] variant breaks this build until somebody
/// classifies it, which is the only mechanism that survives the author
/// leaving.
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

        // `Allowlist` reaching here means the guard above did not hold, i.e.
        // the list names people (or names nobody, which admits nobody).
        DmPolicy::Allowlist => {}

        // `Pairing` is bounded, and this arm is where that is DECIDED rather
        // than defaulted. It is fail-closed in this tree, and on the branch
        // that implements the handshake it becomes permissive-on-success — in
        // both shapes the admitted set is "senders who redeemed a code the
        // operator issued", which is precisely the "somebody named them" this
        // gate looks for. `dm_allowlist` is not consulted under `Pairing`, so
        // a `"*"` sitting in it is inert and flagging it would be an
        // over-refusal the operator could satisfy only by deleting a dead key.
        DmPolicy::Pairing => {}

        // Admits nobody at all.
        DmPolicy::Disabled => {}
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

        // `Allowlist` reaching here is one of the two bounded cases: either
        // `sender_allowlist` names people, or `group_allowlist` is EMPTY, in
        // which case the conversation test denies first and a `"*"` sender
        // wildcard admits nobody.
        GroupPolicy::Allowlist => {}

        // Admits nobody at all.
        GroupPolicy::Disabled => {}
    }

    out
}

// ---------------------------------------------------------------------------
// The admission SHAPE — what a consent is actually bound to
// ---------------------------------------------------------------------------

/// Version tag opening every [`admission_shape_token`].
///
/// Present so the encoding can change without a stored token silently meaning
/// something else: a token written under an older tag does not parse, and an
/// unparseable token refuses rather than being ignored.
///
/// `v2` added `platform` and the whole `[options]` table — see
/// [`AdmissionShape`].
pub const ADMISSION_SHAPE_VERSION: &str = "admission-v2";

/// The FIXED part of the shape, in token order. The `[options]` part is
/// dynamic and follows these — see [`OPTION_FIELD_PREFIX`].
///
/// # The rule for what is in here
///
/// A field belongs in the shape iff it decides **who is admitted, what an
/// admitted principal can trigger without addressing the bot, or what the
/// resulting turn is allowed to do to this host**:
///
/// - `name` — the channel's identity, and the FIRST component of every session
///   key its turns run under: `build_session_key` composes
///   `agent:main:<name>:…`, `ChannelTurnDispatcher::hashed_session_id` hashes
///   that string, and `engine_for` RESUMES the session already on disk under
///   that id (`wcore-agent/src/channel_dispatch.rs`). Renaming a channel
///   therefore re-points every future turn at a different persisted history —
///   and if that name previously belonged to another channel, admitted senders
///   resume ITS conversation, operator messages included. That is the same
///   confidentiality axis `group_sessions_per_user` is in the shape for, one
///   scope wider, and it is a one-line edit (plus the matching file rename the
///   loader's stem check demands). It also makes the consent name the thing it
///   consents to: without it, a token written for `sandbox` is equally valid
///   for whatever that file is called tomorrow.
/// - `platform` — which adapter parses `[options]`, and therefore what every
///   key in it MEANS. `allowed_senders` is an email delivery filter and nothing
///   at all to a Slack channel, so a token that fixed the options but floated
///   the platform would be consenting to a reading of them that could change.
/// - `enabled` — a switched-off channel admits nobody. It lives on
///   [`crate::config::ChannelConfig`] rather than in `[inbound]`, and it is in
///   the shape precisely because leaving it out let an operator consent to an
///   open door while the door was bricked up, then unbrick it in one word.
/// - `dm`, `dm_allowlist` — who may DM.
/// - `group`, `group_allowlist`, `sender_allowlist` — which conversations, and
///   who inside them.
/// - `require_mention` — whether an admitted sender must address the bot.
///   Turning it off does not widen the sender set, but it does mean every
///   message in a reachable conversation drives an agent turn, which is a
///   widening of exposure an operator must look at.
/// - `tools`, `tool_workspace_root` — what an admitted stranger's turn can do
///   to this host. "Anyone may DM this bot" is a materially different decision
///   at [`ChannelToolPosture::Conversational`] than at
///   [`ChannelToolPosture::Full`], and moving between them is a one-word edit.
///   Leaving them out would leave a live consent silently covering a blast
///   radius nobody looked at — which is the whole failure mode this key exists
///   to prevent, arriving through the authority axis instead of the admission
///   one.
/// - `group_sessions_per_user`, `thread_sessions_per_user` — whether admitted
///   senders share ONE agent session. These were once excluded as "cosmetic",
///   on the reasoning that they change neither who reaches the agent nor what
///   the turn may do. The second half of that is false. With
///   `group_sessions_per_user = false` every sender in a group collapses into
///   the single session key `agent:main:<channel>:<conversation_id>` (see
///   [`crate::dispatch::build_session_key`]), so each turn is run with every
///   other sender's history in context — including the operator's. On a channel
///   that already admits an unbounded set of senders, flipping that one bool
///   turns "N mutually blind strangers" into "every stranger reads what
///   everyone, operator included, has said". It is a change in what an admitted
///   turn may READ rather than in who is admitted, but it is squarely inside
///   "what the turn may do", so it is inside the token.
///
/// # What is deliberately OUT, and why
///
/// An exclusion here is a DECISION, not an omission. BOTH structs that feed the
/// shape are destructured exhaustively — [`crate::config::ChannelConfig`] in
/// [`ChannelConfig::admission_shape`] and [`InboundPolicy`] in
/// [`shape_fields`] — so every field must be either rendered or explicitly
/// discarded with a reason written at the binding. `ChannelConfig` discards
/// NOTHING: all five of its fields are in the token. `InboundPolicy` discards
/// two:
///
/// - `ack` — how the bot signals it is working (reactions / typing). Purely
///   OUTBOUND presentation. It cannot change who is admitted, what an admitted
///   turn may read, or what it may do to this host.
/// - `acknowledge_open_admission` — the field that HOLDS this token. Rendering
///   it would make the token contain itself, which no fixed point can satisfy;
///   it is excluded by construction rather than by judgement.
///
/// Everything else is in, and the line is drawn at reach and authority.
///
/// # The `..` trap, and why a comment is not the guard
///
/// rustc's third suggestion for the E0027 both destructurings raise is "or
/// always ignore missing fields here" — i.e. add `..`. Taking it compiles,
/// keeps every existing test green, and silently re-opens the hole. A comment
/// the compiler argues against is not a guard, so the compile-time half is
/// backed by a runtime one: `a_field_added_to_*_cannot_be_left_out_of_the_
/// consent` reads the field set back off serde (the only reflection Rust
/// offers, and the one thing that cannot fall behind the struct) and fails
/// unless every field is either proven to move the token or named in that
/// test's exclusion list. Those tests go red in the `..` world.
pub const SHAPE_FIELDS: [&str; 13] = [
    "name",
    "platform",
    "enabled",
    "dm",
    "dm_allowlist",
    "group",
    "group_allowlist",
    "sender_allowlist",
    "require_mention",
    "tools",
    "tool_workspace_root",
    "group_sessions_per_user",
    "thread_sessions_per_user",
];

/// Prefix every rendered `[options]` key carries in a token, so the dynamic
/// tail cannot be confused with (or forge) one of the [`SHAPE_FIELDS`].
pub const OPTION_FIELD_PREFIX: &str = "options.";

/// Everything a consent is bound to, for ONE channel.
///
/// # Why the whole `[options]` table is in here, and not a list of keys
///
/// `[inbound]` is not the only place a channel decides who is admitted. Four
/// of the ten adapters carry a SECOND, adapter-level admission filter in their
/// own `[options]` table, and in every one of them an ABSENT or EMPTY list is
/// the most permissive state:
///
/// | Adapter | Key | Empty means |
/// |---|---|---|
/// | email | `options.imap.allowed_senders` | every `From:` is admitted |
/// | discord | `options.allowed_channel_ids` | every channel is admitted |
/// | telegram | `options.allowed_chat_ids` | every chat is admitted |
/// | imessage | `options.allowed_handles` | every handle is admitted |
///
/// So `[options.imap] allowed_senders = ["boss@acme.test"]` and the same file
/// with that ONE line deleted used to produce a byte-identical token, and the
/// widened config started on the narrow config's consent. That is the same
/// defect the whole-shape token was built to close, one config section over.
///
/// **Read that table as four EXAMPLES, not as the contract.** It lists the four
/// keys with the empty-means-everyone inversion; it is not the set of
/// `[options]` keys that touch reach. matrix `options.user_id` is the
/// counter-example: it is the id the mention gate matches against
/// (`wcore-channel-matrix/src/sync.rs`, `body.contains(bot_user_id)`), so it
/// decides which group messages count as addressed to the bot and therefore
/// which ones drive a turn under `require_mention`. It carries no inversion, so
/// it is not in the table, and it is nonetheless bound by the consent — because
/// the token renders the whole table and never consults a list like this one.
/// Any summary of "which options matter" is a snapshot that can rot; the gate
/// deliberately does not depend on one.
///
/// # Why this is the whole table rather than a declared key list
///
/// The obvious repair is for each adapter to declare its admission-relevant
/// keys and for the token to render those. It was rejected, because **the
/// enumeration IS the failure mode.** This gate has now been bypassed three
/// times by a category nobody enumerated: first a per-field token list that
/// could not see a widening of a field it did not name, then `enabled`, and now
/// `[options]`. A declared key list fails in exactly that direction again —
/// adapter number eleven ships a new filter, nobody adds it to the list, and
/// the bypass is silent, at runtime, with no test able to see it.
///
/// Rendering the WHOLE table inverts the direction of the failure. There is
/// nothing to enumerate, so nothing to forget; a new adapter, a new key, or a
/// key nobody has thought about yet is inside the token the day it is written,
/// with no registration step and no coverage test to keep current. The cost of
/// being wrong becomes an over-refusal an operator SEES (the refusal names the
/// key, the old value and the new one) rather than a widening nobody sees.
///
/// # The cost, honestly
///
/// An operator who edits a genuinely reach-irrelevant option — `poll_interval_secs`,
/// `max_retry_attempts` — on an ALREADY-OPEN channel is asked to re-acknowledge.
/// That is the whole price, and it is bounded three ways: it applies only to
/// channels that already admit an unbounded set of senders (a deliberately
/// exceptional, deliberately uncomfortable state), the ten adapters' option
/// sets are small and static, and the refusal prints `key: acknowledged <was>,
/// now <is>` so the operator re-consents in seconds rather than decoding a
/// hash. Paying that against a silent third bypass is not a close call.
///
/// Cosmetic edits are still free: option lists are sorted and de-duplicated
/// before rendering, exactly as the `[inbound]` allowlists are.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionShape<'a> {
    /// `name` from [`crate::config::ChannelConfig`].
    pub name: &'a str,
    /// `platform` from [`crate::config::ChannelConfig`].
    pub platform: &'a str,
    /// `enabled` from [`crate::config::ChannelConfig`].
    pub enabled: bool,
    /// The channel's whole `[options]` table.
    pub options: &'a toml::Table,
    /// The channel's `[inbound]` policy.
    pub inbound: &'a InboundPolicy,
}

impl ChannelConfig {
    /// This channel's admission shape — every setting a consent is bound to.
    ///
    /// Borrowing the whole `ChannelConfig` is necessary but NOT sufficient, and
    /// the difference is what this method got wrong once. Taking `&self` stops
    /// a CALLER handing the gate a subset of the file; it does nothing about
    /// this method reading four of the five fields by field access and
    /// dropping the fifth. A reach-relevant field added to `ChannelConfig` and
    /// populated with an attacker id was invisible to the token while the whole
    /// suite stayed green — the same enumeration defect [`shape_fields`] closes
    /// one level down, at the level that projects the file INTO the gate.
    ///
    /// So the projection destructures EXHAUSTIVELY, deliberately without a `..`
    /// rest pattern: a sixth field does not compile (E0027) until someone
    /// decides here whether a consent is bound to it. Every binding is either
    /// carried into the shape or discarded with a written reason. Do not
    /// silence a new field with `..` or `_` — write why.
    pub fn admission_shape(&self) -> AdmissionShape<'_> {
        let ChannelConfig {
            name,
            platform,
            enabled,
            options,
            inbound,
        } = self;
        AdmissionShape {
            name,
            platform,
            enabled: *enabled,
            options,
            inbound,
        }
    }
}

/// Rendering of an EMPTY allowlist. Distinct from `1:` (a list holding one
/// empty-string entry) because those permit different sets.
const EMPTY_LIST: &str = "0";

fn dm_policy_name(p: &DmPolicy) -> &'static str {
    match p {
        DmPolicy::Open => "open",
        DmPolicy::Allowlist => "allowlist",
        DmPolicy::Pairing => "pairing",
        DmPolicy::Disabled => "disabled",
    }
}

fn group_policy_name(p: &GroupPolicy) -> &'static str {
    match p {
        GroupPolicy::Open => "open",
        GroupPolicy::Allowlist => "allowlist",
        GroupPolicy::Disabled => "disabled",
    }
}

fn posture_name(p: ChannelToolPosture) -> &'static str {
    match p {
        ChannelToolPosture::Conversational => "conversational",
        ChannelToolPosture::Workspace => "workspace",
        ChannelToolPosture::Full => "full",
    }
}

/// Canonical rendering of an optional single value, in the same `<count>[:v]`
/// shape [`canonical_list`] uses so the two cannot be read differently.
fn canonical_opt(value: Option<&str>) -> String {
    match value {
        None => EMPTY_LIST.to_string(),
        Some(v) => format!("1:{}", escape_entry(v)),
    }
}

/// Percent-escape every character that carries structure in a token: the field
/// separator (space, and any other whitespace or control), the field/value
/// separator (`=`), the entry separator (`,`), the `[options]` path separator
/// (`.`), and the escape character itself.
///
/// Without this, `["a,b"]` and `["a","b"]` would render identically while
/// admitting different senders — a collision that would let a widening pass as
/// unchanged. `.` and `=` matter for the same reason on the `[options]` side:
/// a table key `"a.b"` must not render as the nested path `a.b`, and a key
/// holding `=` must not be able to split into a field nobody wrote.
fn escape_entry(entry: &str) -> String {
    let mut out = String::with_capacity(entry.len());
    for ch in entry.chars() {
        match ch {
            '%' => out.push_str("%25"),
            ',' => out.push_str("%2C"),
            '.' => out.push_str("%2E"),
            '=' => out.push_str("%3D"),
            c if c.is_whitespace() || c.is_control() => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Canonical rendering of one allowlist.
///
/// - holds the [`WILDCARD`] -> `*`. Correct rather than merely convenient:
///   [`permits`] returns true for every id once `"*"` is present, so `["*"]`
///   and `["*","U1"]` admit exactly the same set.
/// - empty -> [`EMPTY_LIST`].
/// - otherwise -> `<count>:<entries>`, sorted, de-duplicated and escaped. The
///   count is carried so `[]` and `[""]` cannot render alike.
fn canonical_list(list: &[String]) -> String {
    if permits_anyone(list) {
        return WILDCARD.to_string();
    }
    let mut entries: Vec<&str> = list.iter().map(String::as_str).collect();
    entries.sort_unstable();
    entries.dedup();
    if entries.is_empty() {
        return EMPTY_LIST.to_string();
    }
    let escaped: Vec<String> = entries.iter().map(|e| escape_entry(e)).collect();
    format!("{}:{}", escaped.len(), escaped.join(","))
}

/// Rendering of a `[options]` table that is PRESENT but empty. Needed because
/// an empty table contributes no leaf paths, and "present and empty" must not
/// render as "absent" — for `[options.imap]` those are the difference between
/// polling a mailbox and not having an inbound leg at all.
const EMPTY_TABLE: &str = "t:0";

/// Canonical rendering of one `[options]` value.
///
/// Every rendering carries a TYPE TAG, because unlike the fixed
/// [`SHAPE_FIELDS`] the options tree is schemaless: without a tag the string
/// `"1"` and the integer `1` would render alike, and an operator (or a config
/// generator) could change the type of a key under a live consent.
fn canonical_option_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("s:{}", escape_entry(s)),
        toml::Value::Integer(i) => format!("i:{i}"),
        toml::Value::Float(f) => format!("f:{f}"),
        toml::Value::Boolean(b) => format!("b:{b}"),
        toml::Value::Datetime(d) => format!("d:{}", escape_entry(&d.to_string())),
        // Sorted and de-duplicated for the same reason the `[inbound]`
        // allowlists are: every option list in the tree is an ID SET (the four
        // adapter admission filters all test membership), so a reorder or a
        // repeat admits exactly the same principals and must not refuse.
        //
        // The wildcard is deliberately NOT collapsed here, unlike
        // [`canonical_list`]. No adapter option list honours `"*"` — they are
        // exact-membership tests — so `["*"]` and `["*","111"]` admit
        // DIFFERENT sets and must not share a rendering.
        toml::Value::Array(items) => {
            let mut rendered: Vec<String> = items
                .iter()
                .map(|v| escape_entry(&canonical_option_value(v)))
                .collect();
            rendered.sort();
            rendered.dedup();
            format!("a:{}:{}", rendered.len(), rendered.join(","))
        }
        // Only reachable for a table nested inside an ARRAY; a table reached
        // through the tree is flattened into dotted paths by `render_options`.
        toml::Value::Table(table) => {
            let mut rendered: Vec<String> = table
                .iter()
                .map(|(k, v)| {
                    escape_entry(&format!(
                        "{}={}",
                        escape_entry(k),
                        canonical_option_value(v)
                    ))
                })
                .collect();
            rendered.sort();
            format!("t:{}:{}", rendered.len(), rendered.join(","))
        }
    }
}

/// `(dotted path, canonical value)` for every leaf in the `[options]` tree,
/// sorted by path and each path prefixed with [`OPTION_FIELD_PREFIX`].
///
/// Nested tables are flattened rather than rendered whole so a refusal can name
/// the ONE key that moved — `options.imap.allowed_senders` — instead of dumping
/// the whole subtree twice and leaving the operator to diff it by eye.
///
/// A key that is ABSENT simply produces no entry, which is what makes deleting
/// a line change the token: the deleted path is in the acknowledged token and
/// not in the live one, and the diff reports it as `now (absent)`. That is the
/// direction that matters, because for all four adapter filters an absent list
/// is the MOST permissive state.
fn render_options(options: &toml::Table) -> Vec<(String, String)> {
    fn walk(prefix: &str, table: &toml::Table, out: &mut Vec<(String, String)>) {
        if table.is_empty() {
            // The top-level `[options]` is exempt: absent and empty are the
            // same input to every adapter (serde fills in an empty table), so
            // distinguishing them would refuse over a deleted blank header.
            if !prefix.is_empty() {
                out.push((prefix.to_string(), EMPTY_TABLE.to_string()));
            }
            return;
        }
        for (key, value) in table {
            let path = if prefix.is_empty() {
                format!("{OPTION_FIELD_PREFIX}{}", escape_entry(key))
            } else {
                format!("{prefix}.{}", escape_entry(key))
            };
            match value {
                toml::Value::Table(sub) => walk(&path, sub, out),
                other => out.push((path, canonical_option_value(other))),
            }
        }
    }
    let mut out = Vec::new();
    walk("", options, &mut out);
    out.sort();
    out
}

/// `(field, canonical value)` for the whole shape: every field in
/// [`SHAPE_FIELDS`] in order, then every `[options]` leaf, sorted.
fn shape_fields(shape: &AdmissionShape<'_>) -> Vec<(String, String)> {
    // EXHAUSTIVE destructuring of the shape itself, deliberately WITHOUT a
    // `..` rest pattern — for the same reason as the `InboundPolicy`
    // destructure below, one hop earlier.
    //
    // `AdmissionShape` is the struct this whole token is rendered from, and
    // reading it by field access (`shape.name`, `shape.options`, …) left it as
    // the one struct on the path with no compile-time guard at all: a sixth
    // shape field would compile, every test would stay green, and the token
    // would simply not mention it. `ChannelConfig::admission_shape` refuses to
    // CARRY a field silently; this refuses to RENDER one silently. Both halves
    // are needed — a field carried into the shape and then never read is the
    // same widening as a field never carried.
    let &AdmissionShape {
        name,
        platform,
        enabled,
        options,
        inbound,
    } = shape;

    // EXHAUSTIVE destructuring, deliberately WITHOUT a `..` rest pattern.
    //
    // This is the compile-time half of the gate, and it is the reason there is
    // no hand-maintained list of `[inbound]` keys to fall out of date. Reading
    // the policy by field access (`policy.dm`, `policy.group`, …) made adding a
    // fourteenth reach-relevant field to `InboundPolicy` a silent no-op: it
    // compiled, every access test stayed green, and the emitted token simply
    // did not mention it — so a consent written against the narrow value
    // covered the widened one. That is the SAME defect the whole-`[options]`
    // rendering exists to close (see [`AdmissionShape`]), one config section
    // over, and "the enumeration IS the failure mode" applies to `[inbound]`
    // exactly as it applies to `[options]`.
    //
    // With no `..`, field thirteen does not compile until someone decides here
    // whether it belongs in the consent. A build error is the intended
    // behaviour: the cost of a new field is one deliberate line, and the
    // alternative is a widening nobody sees.
    //
    // Every binding below is therefore either RENDERED or discarded with a
    // written reason. Do not silence a new field with `_` — write why.
    let InboundPolicy {
        dm,
        group,
        require_mention,
        dm_allowlist,
        group_allowlist,
        sender_allowlist,
        group_sessions_per_user,
        thread_sessions_per_user,
        tools,
        tool_workspace_root,
        // EXCLUDED — outbound presentation only. `ack` decides whether the bot
        // adds a reaction or a typing indicator to a message it is already
        // working on. It cannot change who is admitted, what an admitted turn
        // may read, or what that turn may do to this host.
        ack: _,
        // EXCLUDED — by construction, not by judgement. This is the field that
        // HOLDS the token being built; rendering it would require the token to
        // contain itself.
        acknowledge_open_admission: _,
    } = inbound;

    let mut fields: Vec<(String, String)> = vec![
        ("name".into(), escape_entry(name)),
        ("platform".into(), escape_entry(platform)),
        ("enabled".into(), enabled.to_string()),
        ("dm".into(), dm_policy_name(dm).to_string()),
        ("dm_allowlist".into(), canonical_list(dm_allowlist)),
        ("group".into(), group_policy_name(group).to_string()),
        ("group_allowlist".into(), canonical_list(group_allowlist)),
        ("sender_allowlist".into(), canonical_list(sender_allowlist)),
        ("require_mention".into(), require_mention.to_string()),
        ("tools".into(), posture_name(*tools).to_string()),
        (
            "tool_workspace_root".into(),
            canonical_opt(tool_workspace_root.as_deref()),
        ),
        (
            "group_sessions_per_user".into(),
            group_sessions_per_user.to_string(),
        ),
        (
            "thread_sessions_per_user".into(),
            thread_sessions_per_user.to_string(),
        ),
    ];
    debug_assert!(
        fields.iter().map(|(k, _)| k.as_str()).eq(SHAPE_FIELDS),
        "the rendered fields and SHAPE_FIELDS must not drift: the parser trusts the order"
    );
    fields.extend(render_options(options));
    fields
}

/// The canonical token for one channel's admission shape.
///
/// This is what [`InboundPolicy::acknowledge_open_admission`] must hold, and it
/// is a rendering rather than a hash on purpose: a refusal has to be able to
/// print what CHANGED, and nothing can be recovered from a digest. An operator
/// told only "hash mismatch" learns nothing and reaches for the nuclear option.
pub fn admission_shape_token(shape: &AdmissionShape<'_>) -> String {
    let mut out = String::from(ADMISSION_SHAPE_VERSION);
    for (field, value) in shape_fields(shape) {
        out.push(' ');
        out.push_str(&field);
        out.push('=');
        out.push_str(&value);
    }
    out
}

/// Parse a token back into `(field, value)` pairs, or `None` if it is not a
/// token this version wrote.
///
/// The fixed head must match [`SHAPE_FIELDS`] exactly, in order — a token
/// missing one of those would be a consent that silently says nothing about it.
/// The `[options]` tail is dynamic, so it is checked structurally instead:
/// every entry carries [`OPTION_FIELD_PREFIX`] and the paths STRICTLY ASCEND,
/// which is the same shape [`render_options`] emits and which no repeated or
/// re-ordered key can satisfy.
fn parse_shape_token(token: &str) -> Option<Vec<(String, String)>> {
    let mut parts = token.split(' ');
    if parts.next()? != ADMISSION_SHAPE_VERSION {
        return None;
    }
    let mut out = Vec::with_capacity(SHAPE_FIELDS.len());
    for expected in SHAPE_FIELDS {
        let (field, value) = parts.next()?.split_once('=')?;
        if field != expected {
            return None;
        }
        out.push((expected.to_string(), value.to_string()));
    }
    let mut previous: Option<&str> = None;
    for part in parts {
        let (field, value) = part.split_once('=')?;
        if !field.starts_with(OPTION_FIELD_PREFIX) {
            return None;
        }
        if previous.is_some_and(|prev| field <= prev) {
            return None;
        }
        previous = Some(field);
        out.push((field.to_string(), value.to_string()));
    }
    Some(out)
}

/// A consent an OLDER cut of this key wrote. Recognised only so an operator
/// upgrading is told what happened instead of being handed a bare "this is not
/// a token".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyToken {
    /// One of the per-field tokens the FIRST cut used (`"dm=open"`, …).
    PerField,
    /// An `admission-v1` whole-shape token — correct in its day, but it named
    /// neither `platform` nor `[options]`, so it cannot see an adapter-level
    /// admission filter being widened.
    ShapeV1,
}

/// The per-field tokens the FIRST cut of this key used.
const LEGACY_TOKENS: [&str; 4] = [
    "dm=open",
    "dm_allowlist=*",
    "group=open",
    "sender_allowlist=*",
];

/// Version tag of the whole-shape token that predates `[options]`.
const LEGACY_SHAPE_VERSION: &str = "admission-v1";

/// Which older spelling `token` is, if any.
fn legacy_kind(token: &str) -> Option<LegacyToken> {
    if LEGACY_TOKENS.contains(&token) {
        Some(LegacyToken::PerField)
    } else if token
        .split_once(' ')
        .is_some_and(|(head, _)| head == LEGACY_SHAPE_VERSION)
    {
        Some(LegacyToken::ShapeV1)
    } else {
        None
    }
}

/// How an ABSENT field is printed in a refusal. A `[options]` key can be
/// present on one side of the comparison and gone from the other, and for the
/// four adapter admission filters DELETING the key is the widening — so the
/// refusal has to be able to say so in words rather than print an empty string
/// that reads like a narrow value.
pub const ABSENT_FIELD: &str = "(absent)";

/// One field whose value differs between the acknowledged shape and the live
/// one. Carried so the refusal can name the change rather than the mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionFieldChange {
    /// The setting that changed: one of [`SHAPE_FIELDS`], or an `[options]`
    /// path carrying [`OPTION_FIELD_PREFIX`].
    pub field: String,
    /// Its canonical value in the token the operator wrote, or `None` when the
    /// token did not mention it at all.
    pub acknowledged: Option<String>,
    /// Its canonical value in the configuration as it now stands, or `None`
    /// when the configuration no longer has it.
    pub current: Option<String>,
}

/// Render one side of a change for the refusal message.
fn or_absent(value: Option<&String>) -> &str {
    value.map_or(ABSENT_FIELD, String::as_str)
}

/// Every field that differs between an acknowledged shape and the live one.
///
/// A UNION over both key sets, not a positional zip: the `[options]` tail is
/// dynamic, so a key can be on one side only, and a zip would either miss that
/// or misalign every field after it. Ordered [`SHAPE_FIELDS`] first (which are
/// always on both sides), then option paths in sorted order.
fn shape_changes(
    acknowledged: &[(String, String)],
    current: &[(String, String)],
) -> Vec<AdmissionFieldChange> {
    let acknowledged: std::collections::BTreeMap<&str, &String> =
        acknowledged.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let current: std::collections::BTreeMap<&str, &String> =
        current.iter().map(|(k, v)| (k.as_str(), v)).collect();

    let mut fields: Vec<&str> = SHAPE_FIELDS.to_vec();
    let mut options: Vec<&str> = acknowledged
        .keys()
        .chain(current.keys())
        .copied()
        .filter(|f| f.starts_with(OPTION_FIELD_PREFIX))
        .collect();
    options.sort_unstable();
    options.dedup();
    fields.extend(options);

    fields
        .into_iter()
        .filter_map(|field| {
            let was = acknowledged.get(field).copied();
            let now = current.get(field).copied();
            (was != now).then(|| AdmissionFieldChange {
                field: field.to_string(),
                acknowledged: was.cloned(),
                current: now.cloned(),
            })
        })
        .collect()
}

/// The `acknowledge_open_admission` value that matches this channel EXACTLY as
/// it stands: one [`admission_shape_token`] when anything is open, and nothing
/// otherwise.
///
/// Empty when nothing is open — in which case the correct file has no
/// `acknowledge_open_admission` key at all, because a token present while
/// nothing is open is a stale consent, not a harmless leftover.
pub fn required_acknowledgement(shape: &AdmissionShape<'_>) -> Vec<String> {
    if open_admissions(shape.inbound).is_empty() {
        Vec::new()
    } else {
        vec![admission_shape_token(shape)]
    }
}

/// Why the startup gate refuses one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAdmissionFault {
    /// The channel admits everyone through this shape, and
    /// [`InboundPolicy::acknowledge_open_admission`] is empty.
    Unacknowledged(OpenAdmission),
    /// A consent is present while nothing in this channel is open.
    ///
    /// This is the arm that bounds an acknowledgement in TIME. Without it the
    /// key degenerates into the `allow_open = true` boolean the design
    /// rejects: an operator could pre-arm a currently-bounded channel, and that
    /// single edit would consent in advance to every way the channel might
    /// later be opened. Because a stale token refuses, a token can only be
    /// written while the shape it names is already live.
    Stale {
        /// The token as the operator wrote it.
        token: String,
    },
    /// A consent is present, is a well-formed shape token, and names a
    /// DIFFERENT shape than this channel now has.
    ///
    /// The whole finding this arm exists for: under a per-field token list,
    /// widening `group_allowlist` from `["G1"]` to `["*"]` changed no token, so
    /// consent to "anyone who can reach G1" silently became consent to
    /// "anyone, anywhere".
    ShapeChanged {
        /// The token the operator wrote, verbatim.
        acknowledged: String,
        /// Every field that differs, in [`SHAPE_FIELDS`] order.
        changes: Vec<AdmissionFieldChange>,
    },
    /// A consent is present but is not a shape token this version can read.
    Unreadable {
        /// The entry as the operator wrote it.
        token: String,
        /// Set when it is a spelling an OLDER cut of this key wrote, so the
        /// refusal can say which one rather than being cryptic.
        legacy: Option<LegacyToken>,
    },
    /// More than one entry while the channel is open. Consent is ONE token
    /// naming the whole shape, so a repeated or extra entry is not the exact
    /// match this key requires — and accepting a repeat would make the key a
    /// bag rather than the set it documents itself as.
    NotExactlyOne {
        /// How many entries were found.
        count: usize,
    },
}

/// Everything wrong with this channel's acknowledgement.
///
/// Empty iff `acknowledge_open_admission` holds exactly the
/// [`required_acknowledgement`] for the configuration as it stands — the only
/// state the gate accepts. "Covers" is not enough, because a consent that
/// covers more than is open is a consent to configurations nobody has looked
/// at yet.
pub fn open_admission_faults(shape: &AdmissionShape<'_>) -> Vec<OpenAdmissionFault> {
    let open = open_admissions(shape.inbound);
    let acks = &shape.inbound.acknowledge_open_admission;

    if open.is_empty() {
        return acks
            .iter()
            .map(|token| OpenAdmissionFault::Stale {
                token: token.clone(),
            })
            .collect();
    }

    match acks.len() {
        0 => open
            .into_iter()
            .map(OpenAdmissionFault::Unacknowledged)
            .collect(),
        1 => {
            let token = &acks[0];
            let Some(acknowledged) = parse_shape_token(token) else {
                return vec![OpenAdmissionFault::Unreadable {
                    token: token.clone(),
                    legacy: legacy_kind(token),
                }];
            };
            let changes = shape_changes(&acknowledged, &shape_fields(shape));
            if changes.is_empty() {
                Vec::new()
            } else {
                vec![OpenAdmissionFault::ShapeChanged {
                    acknowledged: token.clone(),
                    changes,
                }]
            }
        }
        count => vec![OpenAdmissionFault::NotExactlyOne { count }],
    }
}

/// One refused channel: what is open, why the consent does not match, and the
/// acknowledgement that would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOpenAdmission {
    /// The channel whose file is at fault.
    pub channel: String,
    /// Every admits-everyone shape this channel currently has. Printed
    /// whatever the fault is: an operator asked to re-consent needs to see
    /// what they are consenting TO, not only that a token stopped matching.
    pub open: Vec<OpenAdmission>,
    /// Why, in [`open_admission_faults`] order.
    pub faults: Vec<OpenAdmissionFault>,
    /// [`required_acknowledgement`] for this channel — what the operator must
    /// write to consent to the configuration as it stands. Empty means the
    /// key must be removed.
    pub required_acknowledgement: Vec<String>,
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

            // WHAT IS OPEN — printed for every fault kind, not only for the
            // unacknowledged one. An operator being asked to re-consent has to
            // see what they are consenting TO; "your token stopped matching" on
            // its own invites a blind copy-paste of the new token.
            for finding in &refused.open {
                writeln!(
                    f,
                    "  channel {channel:?}: [inbound] {} — {}. Use instead: {}",
                    finding.found,
                    finding.consequence(),
                    finding.remedy
                )?;
            }

            for fault in &refused.faults {
                match fault {
                    OpenAdmissionFault::Unacknowledged(_) => {}
                    OpenAdmissionFault::Stale { token } => writeln!(
                        f,
                        "  channel {channel:?}: acknowledge_open_admission names {token:?}, but \
                         nothing in this channel admits an unbounded set of senders any more, so \
                         there is nothing to consent to. An acknowledgement consents to ONE \
                         configuration; when that configuration changes it stops applying rather \
                         than silently covering whatever replaced it.",
                    )?,
                    OpenAdmissionFault::ShapeChanged {
                        acknowledged,
                        changes,
                    } => {
                        writeln!(
                            f,
                            "  channel {channel:?}: acknowledge_open_admission consents to a \
                             DIFFERENT admission shape than this channel now has. It names \
                             {acknowledged:?}. What changed:"
                        )?;
                        for change in changes {
                            writeln!(
                                f,
                                "      {}: acknowledged {}, now {}",
                                change.field,
                                or_absent(change.acknowledged.as_ref()),
                                or_absent(change.current.as_ref())
                            )?;
                        }
                    }
                    OpenAdmissionFault::Unreadable { token, legacy } => {
                        write!(
                            f,
                            "  channel {channel:?}: acknowledge_open_admission names {token:?}, \
                             which is not an admission-shape token."
                        )?;
                        match legacy {
                            Some(LegacyToken::PerField) => write!(
                                f,
                                " That is the older per-field spelling. Consent is now bound to \
                                 this channel's WHOLE admission shape, because a per-field list \
                                 could not see a widening of a field it did not name."
                            )?,
                            Some(LegacyToken::ShapeV1) => write!(
                                f,
                                " That is an {LEGACY_SHAPE_VERSION} token. It named the \
                                 [inbound] table and `enabled`, but neither `platform` nor the \
                                 [options] table — so it could not see an adapter's OWN \
                                 admission filter (for example deleting \
                                 `{OPTION_FIELD_PREFIX}imap.allowed_senders`, which admits every \
                                 sender rather than none) being widened underneath it."
                            )?,
                            None => {}
                        }
                        writeln!(f)?;
                    }
                    OpenAdmissionFault::NotExactlyOne { count } => writeln!(
                        f,
                        "  channel {channel:?}: acknowledge_open_admission holds {count} entries. \
                         Consent is ONE token naming this channel's whole admission shape, so a \
                         repeated or extra entry is not the exact match this key requires."
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
                if refused
                    .faults
                    .iter()
                    .any(|fault| matches!(fault, OpenAdmissionFault::Unacknowledged(_)))
                {
                    writeln!(
                        f,
                        "  channel {channel:?}: nothing acknowledges this configuration."
                    )?;
                }
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
            "The token names this channel's ENTIRE admission shape — {} — AND every key in its \
             [options] table, because four of the ten adapters carry their own admission filter \
             there (email {p}imap.allowed_senders, discord {p}allowed_channel_ids, telegram \
             {p}allowed_chat_ids, imessage {p}allowed_handles) and in every one of them an \
             ABSENT or EMPTY list admits EVERYONE. So changing any of them — including deleting \
             one — refuses again instead of being covered by the old consent. Reordering or \
             repeating a list entry admits exactly the same principals and is NOT a change. This \
             key is read only from the profile-scoped channel file; a project-local \
             .wayland-core.toml cannot set it.",
            SHAPE_FIELDS.join(", "),
            p = OPTION_FIELD_PREFIX,
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
/// the same, and `enabled = true` is a one-word edit away. `enabled` is
/// therefore part of the shape the consent names (see [`SHAPE_FIELDS`]) rather
/// than a reason to skip the check: consenting to an open door while the door
/// is bricked up is not consent to unbricking it.
///
/// Takes the WHOLE [`ChannelConfig`], not a projection of it. The previous
/// signature was `(name, enabled, &InboundPolicy)`, and that is exactly how
/// `platform` and `[options]` — where four adapters keep their own admission
/// filter — stayed outside the consent. A caller cannot now hand the gate a
/// subset of the file it is meant to be gating.
pub fn refuse_open_admission<'a>(
    channels: impl IntoIterator<Item = &'a ChannelConfig>,
) -> Result<(), OpenAdmissionRefusal> {
    let refused: Vec<ChannelOpenAdmission> = channels
        .into_iter()
        .filter_map(|config| {
            let shape = config.admission_shape();
            let faults = open_admission_faults(&shape);
            if faults.is_empty() {
                return None;
            }
            Some(ChannelOpenAdmission {
                channel: config.name.clone(),
                open: open_admissions(&config.inbound),
                faults,
                required_acknowledgement: required_acknowledgement(&shape),
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

    // ---- The acknowledgement is bound to the WHOLE admission shape ----

    /// A channel that is bounded today, pre-armed with the four per-field
    /// tokens the first cut of this key used.
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

    /// An OPEN channel: `sender_allowlist = ["*"]` over a named group. Used as
    /// the base for the widening legs because every other field can be varied
    /// around it without changing whether anything is open.
    fn open_over_one_group() -> InboundPolicy {
        InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U1".into()],
            group: GroupPolicy::Allowlist,
            group_allowlist: vec!["G1".into()],
            sender_allowlist: vec![WILDCARD.into()],
            ..Default::default()
        }
    }

    /// The channel every `[inbound]`-only leg is written against: one platform,
    /// no `[options]`. Legs that exercise the `[options]` half build their own.
    fn config(name: &str, enabled: bool, policy: &InboundPolicy) -> ChannelConfig {
        ChannelConfig {
            name: name.to_string(),
            platform: "slack".to_string(),
            enabled,
            options: toml::Table::new(),
            inbound: policy.clone(),
        }
    }

    /// A channel whose `[options]` is parsed from TOML, for the adapter-level
    /// admission filters.
    fn config_with_options(
        name: &str,
        platform: &str,
        options: &str,
        policy: &InboundPolicy,
    ) -> ChannelConfig {
        ChannelConfig {
            platform: platform.to_string(),
            options: options.parse().expect("test fixture options must parse"),
            ..config(name, true, policy)
        }
    }

    /// The channel every single-channel helper below models.
    ///
    /// ONE name, deliberately. `name` is inside the admission shape, so a
    /// helper that acknowledges a shape under one name and then runs the gate
    /// under another refuses for the RENAME rather than for the thing under
    /// test. These helpers used to take a descriptive per-leg name (`"narrow"`
    /// then `"widened"`, `"off"` then `"on"`) and did exactly that.
    const CHANNEL: &str = "ch";

    /// Run the gate over ONE `[inbound]`-only channel.
    fn refuse_one(enabled: bool, policy: &InboundPolicy) -> Result<(), OpenAdmissionRefusal> {
        refuse_open_admission([&config(CHANNEL, enabled, policy)])
    }

    /// The token for one `[inbound]`-only channel.
    fn token_of(enabled: bool, policy: &InboundPolicy) -> String {
        admission_shape_token(&config(CHANNEL, enabled, policy).admission_shape())
    }

    /// The faults of one `[inbound]`-only channel.
    fn faults_of(enabled: bool, policy: &InboundPolicy) -> Vec<OpenAdmissionFault> {
        open_admission_faults(&config(CHANNEL, enabled, policy).admission_shape())
    }

    /// The required acknowledgement of one `[inbound]`-only channel.
    fn required_of(enabled: bool, policy: &InboundPolicy) -> Vec<String> {
        required_acknowledgement(&config(CHANNEL, enabled, policy).admission_shape())
    }

    /// `policy` with its own required acknowledgement written in.
    fn acknowledged(enabled: bool, policy: &InboundPolicy) -> InboundPolicy {
        InboundPolicy {
            acknowledge_open_admission: required_acknowledgement(
                &config(CHANNEL, enabled, policy).admission_shape(),
            ),
            ..policy.clone()
        }
    }

    /// `config` with its own required acknowledgement written into `[inbound]`.
    fn acknowledged_config(config: ChannelConfig) -> ChannelConfig {
        let ack = required_acknowledgement(&config.admission_shape());
        ChannelConfig {
            inbound: InboundPolicy {
                acknowledge_open_admission: ack,
                ..config.inbound
            },
            ..config
        }
    }

    #[test]
    fn a_blanket_pre_acknowledgement_cannot_be_written_in_the_first_place() {
        // This is the whole defect, at the level of the pure gate: if a bounded
        // channel accepts a consent for shapes it does not have, that file is a
        // standing consent to every future opening.
        let refusal = refuse_one(true, &preacked_but_bounded())
            .expect_err("pre-arming a bounded channel must refuse, not be silently accepted");
        let refused = &refusal.channels[0];
        assert_eq!(refused.channel, CHANNEL);
        assert_eq!(
            refused.faults.len(),
            4,
            "nothing here is open, so every entry is a stale consent: {:?}",
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
    fn a_matching_acknowledgement_is_accepted_and_extra_entries_are_not() {
        // POSITIVE CONTROL first, or the test above is satisfied by a gate that
        // refuses every acknowledgement.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let required = required_of(true, &open);
        assert_eq!(
            required,
            vec![token_of(true, &open)],
            "an open channel requires exactly its own shape token"
        );
        let exact = acknowledged(true, &open);
        refuse_one(true, &exact)
            .expect("an acknowledgement that names the live shape exactly must be accepted");

        // The SAME token twice. Under the first cut this was accepted, which
        // made the list a bag rather than the set the docs claim.
        let doubled = InboundPolicy {
            acknowledge_open_admission: vec![required[0].clone(), required[0].clone()],
            ..open.clone()
        };
        let refusal =
            refuse_one(true, &doubled).expect_err("a repeated consent is not an exact match");
        assert_eq!(
            refusal.channels[0].faults,
            vec![OpenAdmissionFault::NotExactlyOne { count: 2 }]
        );

        // A second, DIFFERENT entry alongside the right one is refused for the
        // same reason — consent is one token, not a set of candidates.
        let extra = InboundPolicy {
            acknowledge_open_admission: vec![required[0].clone(), "group=open".into()],
            ..open.clone()
        };
        assert_eq!(
            refuse_one(true, &extra)
                .expect_err("two entries must refuse")
                .channels[0]
                .faults,
            vec![OpenAdmissionFault::NotExactlyOne { count: 2 }]
        );
    }

    #[test]
    fn widening_a_field_the_old_consent_never_singled_out_refuses() {
        // THE ROOT CAUSE (V-A). `sender_allowlist = ["*"]` over
        // `group_allowlist = ["G1"]` admits anyone who can reach G1. Widening
        // `group_allowlist` to `["*"]` grows that to anyone, anywhere. Under a
        // per-field token list this changed NO token and nothing refused.
        let narrow = acknowledged(true, &open_over_one_group());
        refuse_one(true, &narrow).expect("control: the narrow shape, acknowledged, is accepted");

        let widened = InboundPolicy {
            group_allowlist: vec![WILDCARD.into()],
            ..narrow.clone()
        };
        let refusal = refuse_one(true, &widened)
            .expect_err("widening a field the consent did not single out must still refuse");
        let faults = &refusal.channels[0].faults;
        assert!(
            matches!(
                faults.as_slice(),
                [OpenAdmissionFault::ShapeChanged { changes, .. }]
                    if changes.len() == 1
                        && changes[0].field == "group_allowlist"
                        && changes[0].acknowledged.as_deref() == Some("1:G1")
                        && changes[0].current.as_deref() == Some(WILDCARD)
            ),
            "the fault must name the field, what was acknowledged, and what it is now: {faults:?}"
        );

        // And the message must print the CHANGE, not an opaque mismatch. An
        // operator who cannot tell why consent was invalidated reaches for the
        // nuclear option.
        let msg = refusal.to_string();
        assert!(
            msg.contains("group_allowlist: acknowledged 1:G1, now *"),
            "the refusal must show old -> new for the field that moved; got: {msg}"
        );
    }

    #[test]
    fn switching_on_a_channel_that_was_switched_off_refuses() {
        // V-B. A disabled channel admits NOBODY, so a consent written against
        // it was never contemporaneous with anything. One word later it admits
        // everyone.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let off = acknowledged(false, &open);
        refuse_one(false, &off)
            .expect("control: the switched-off shape, acknowledged, is accepted");

        let refusal = refuse_one(true, &off).expect_err("flipping enabled must be a new decision");
        assert!(
            matches!(
                refusal.channels[0].faults.as_slice(),
                [OpenAdmissionFault::ShapeChanged { changes, .. }]
                    if changes.len() == 1
                        && changes[0].field == "enabled"
                        && changes[0].acknowledged.as_deref() == Some("false")
                        && changes[0].current.as_deref() == Some("true")
            ),
            "the fault must be the `enabled` flip and nothing else: {:?}",
            refusal.channels[0].faults
        );

        // CONTROL: `enabled` only matters when there is something to consent
        // to. A BOUNDED channel must not acquire a consent requirement just by
        // being switched on and off.
        let bounded = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U-NAMED".into()],
            ..Default::default()
        };
        for enabled in [false, true] {
            refuse_one(enabled, &bounded)
                .expect("a bounded channel needs no consent at either setting");
        }
    }

    #[test]
    fn escalating_the_tool_posture_under_a_live_consent_refuses() {
        // An attack on the SECOND cut of this key, not on the first: consent to
        // "anyone may DM this bot" is a different decision at the safe
        // Conversational floor than at Full host access, and moving between
        // them is one word. A consent bound only to the admission axis would
        // silently cover the escalation.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        assert_eq!(
            open.tools,
            ChannelToolPosture::Conversational,
            "control: the fixture consents at the safe floor"
        );
        let consented = acknowledged(true, &open);
        refuse_one(true, &consented).expect("control: the acknowledged floor starts");

        for escalated in [ChannelToolPosture::Workspace, ChannelToolPosture::Full] {
            let policy = InboundPolicy {
                tools: escalated,
                ..consented.clone()
            };
            let refusal = match refuse_one(true, &policy) {
                Ok(()) => panic!(
                    "{escalated:?}: escalating the tool posture under a live open-admission \
                     consent must refuse"
                ),
                Err(e) => e,
            };
            assert!(
                matches!(
                    refusal.channels[0].faults.as_slice(),
                    [OpenAdmissionFault::ShapeChanged { changes, .. }]
                        if changes.len() == 1 && changes[0].field == "tools"
                ),
                "{escalated:?}: the fault must be the posture change: {:?}",
                refusal.channels[0].faults
            );
        }

        // The jail root is part of it too: moving a Workspace channel's jail
        // from /safe to / is the same escalation spelled differently.
        let jailed = acknowledged(
            true,
            &InboundPolicy {
                tools: ChannelToolPosture::Workspace,
                tool_workspace_root: Some("/safe".into()),
                ..open.clone()
            },
        );
        refuse_one(true, &jailed).expect("control: the jailed shape starts");
        let widened = InboundPolicy {
            tool_workspace_root: Some("/".into()),
            ..jailed.clone()
        };
        let refusal =
            refuse_one(true, &widened).expect_err("moving the jail root must demand fresh consent");
        assert!(
            refusal
                .to_string()
                .contains("tool_workspace_root: acknowledged 1:/safe, now 1:/"),
            "the refusal must show the jail root that moved; got: {refusal}"
        );
    }

    #[test]
    fn reordering_or_repeating_an_allowlist_entry_is_not_a_change() {
        // The other direction, and it is load-bearing: a gate that cries wolf
        // on a cosmetic edit teaches operators to stop reading refusals.
        let base = InboundPolicy {
            group_allowlist: vec!["G1".into(), "G2".into()],
            ..open_over_one_group()
        };
        let consented = acknowledged(true, &base);
        for cosmetic in [
            vec!["G2".to_string(), "G1".to_string()],
            vec!["G2".to_string(), "G1".to_string(), "G1".to_string()],
            vec!["G1".to_string(), "G1".to_string(), "G2".to_string()],
        ] {
            let reordered = InboundPolicy {
                group_allowlist: cosmetic.clone(),
                ..consented.clone()
            };
            refuse_one(true, &reordered).unwrap_or_else(|e| {
                panic!("{cosmetic:?} admits exactly the same principals and must not refuse: {e}")
            });
        }

        // A wildcard subsumes every other entry, so `["*"]` and `["*","U1"]`
        // are the same admitted set and must share a token. This is an
        // equivalence, not a convenience: `permits` returns true for every id
        // once the wildcard is present.
        let star = InboundPolicy {
            sender_allowlist: vec![WILDCARD.into()],
            ..open_over_one_group()
        };
        let star_plus = InboundPolicy {
            sender_allowlist: vec![WILDCARD.into(), "U1".into()],
            ..open_over_one_group()
        };
        assert_eq!(token_of(true, &star), token_of(true, &star_plus));
    }

    // ---- The consent covers `[options]`, where four adapters keep their OWN
    // ---- admission filter and an ABSENT one admits everyone ----

    /// The four adapter-level admission filters, as `(platform, the narrow
    /// `[options]` TOML, the WIDENED `[options]` TOML, the rendered path)`.
    ///
    /// The widened variant DELETES the key rather than emptying it, because
    /// that is the shape the verifier drove: for all four adapters an absent
    /// list is the most permissive state, so a deletion is a widening and it
    /// used to produce a byte-identical token.
    fn adapter_admission_filters() -> Vec<(&'static str, String, String, &'static str)> {
        vec![
            (
                "email",
                "from_address = \"bot@acme.test\"\n[smtp]\nhost = \"s\"\nuser_credential_handle = \
                 \"u\"\npassword_credential_handle = \"p\"\n[imap]\nhost = \
                 \"i\"\nuser_credential_handle = \"u\"\npassword_credential_handle = \
                 \"p\"\nallowed_senders = [\"boss@acme.test\"]\n"
                    .to_string(),
                "from_address = \"bot@acme.test\"\n[smtp]\nhost = \"s\"\nuser_credential_handle = \
                 \"u\"\npassword_credential_handle = \"p\"\n[imap]\nhost = \
                 \"i\"\nuser_credential_handle = \"u\"\npassword_credential_handle = \"p\"\n"
                    .to_string(),
                "options.imap.allowed_senders",
            ),
            (
                "discord",
                "credential_handle = \"d\"\nallowed_channel_ids = [\"111\"]\n".to_string(),
                "credential_handle = \"d\"\n".to_string(),
                "options.allowed_channel_ids",
            ),
            (
                "telegram",
                "credential_handle = \"t\"\nallowed_chat_ids = [\"-100\"]\n".to_string(),
                "credential_handle = \"t\"\n".to_string(),
                "options.allowed_chat_ids",
            ),
            (
                "imessage",
                "allowed_handles = [\"+15550000000\"]\n".to_string(),
                String::new(),
                "options.allowed_handles",
            ),
        ]
    }

    #[test]
    fn deleting_an_adapter_admission_filter_refuses() {
        // N1. THE DEFECT. `[inbound]` is not the only place a channel decides
        // who is admitted: four adapters carry their own filter in `[options]`,
        // and for every one of them an ABSENT list admits EVERYONE. Under a
        // token that rendered only `[inbound]` plus `enabled`, the narrow and
        // the widened configs produced a byte-identical token, so the widened
        // config STARTED on the narrow config's consent.
        for (platform, narrow, widened, path) in adapter_admission_filters() {
            let open = InboundPolicy {
                dm: DmPolicy::Open,
                ..Default::default()
            };
            let consented = acknowledged_config(config_with_options("c", platform, &narrow, &open));

            // CONTROL: the narrow config, acknowledged, starts. Without this
            // the leg below is satisfied by a gate that refuses everything.
            refuse_open_admission([&consented]).unwrap_or_else(|e| {
                panic!("{platform}: the acknowledged narrow config must start: {e}")
            });

            let widened = ChannelConfig {
                options: widened.parse().expect("fixture parses"),
                ..consented.clone()
            };
            let msg = match refuse_open_admission([&widened]) {
                Ok(()) => panic!(
                    "{platform}: deleting {path} widens the admitted set to everyone and MUST \
                     refuse on the narrow config's consent"
                ),
                Err(refusal) => refusal.to_string(),
            };
            assert!(
                msg.contains(&format!("{path}: acknowledged")) && msg.contains(ABSENT_FIELD),
                "{platform}: the refusal must name the deleted key and say it is now absent; got: \
                 {msg}"
            );

            // And the token the refusal advertises must be the one it accepts —
            // otherwise the operator has no way forward.
            let reconsented = acknowledged_config(widened);
            refuse_open_admission([&reconsented]).unwrap_or_else(|e| {
                panic!("{platform}: the advertised token must be accepted: {e}")
            });
        }
    }

    #[test]
    fn narrowing_an_adapter_admission_filter_refuses_too() {
        // The other direction of the same binding. A consent names ONE
        // configuration; when the file changes it stops applying, whether the
        // change widened or narrowed. Without this the key would be a one-way
        // ratchet an operator could not reason about.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let wide = acknowledged_config(config_with_options(
            "c",
            "discord",
            "credential_handle = \"d\"\n",
            &open,
        ));
        refuse_open_admission([&wide]).expect("control: the acknowledged wide config starts");

        let narrowed = ChannelConfig {
            options: "credential_handle = \"d\"\nallowed_channel_ids = [\"111\"]\n"
                .parse()
                .expect("fixture parses"),
            ..wide
        };
        let msg = refuse_open_admission([&narrowed])
            .expect_err("adding a filter is still a change to the shape the consent names")
            .to_string();
        assert!(
            msg.contains(&format!(
                "options.allowed_channel_ids: acknowledged {ABSENT_FIELD}, now"
            )),
            "the refusal must say the key was absent and now is not; got: {msg}"
        );
    }

    #[test]
    fn an_empty_option_list_is_not_the_same_as_an_absent_one() {
        // The empty-means-everyone inversion, at the rendering level. For all
        // four adapters `[]` and "key deleted" happen to admit the same set —
        // everyone — but `["x"]`, `[]` and absent are THREE distinct states of
        // the file, and collapsing any two of them lets one be edited into
        // another under a live consent.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let token = |options: &str| {
            admission_shape_token(
                &config_with_options("c", "discord", options, &open).admission_shape(),
            )
        };
        let named = token("allowed_channel_ids = [\"111\"]\n");
        let empty = token("allowed_channel_ids = []\n");
        let absent = token("");
        assert_ne!(
            named, empty,
            "a named list and an empty one admit different sets"
        );
        assert_ne!(
            empty, absent,
            "an EMPTY list and an ABSENT key are different states of the file and must render \
             differently, or one can be edited into the other under a live consent"
        );
        assert_ne!(named, absent);

        // A present-but-empty SUB-TABLE is likewise not an absent one:
        // `[options.imap]` present with no keys is an inbound leg configured
        // and empty; absent is no inbound leg at all.
        let empty_table = token("[imap]\n");
        assert_ne!(
            empty_table,
            token(""),
            "a present-but-empty [options.imap] must not render as an absent one"
        );
    }

    #[test]
    fn reordering_or_repeating_an_option_list_entry_is_not_a_change() {
        // The over-refusal direction, and it is load-bearing for the same
        // reason it is on the `[inbound]` allowlists: a gate that cries wolf on
        // a cosmetic edit teaches operators to stop reading refusals. Every
        // adapter option list is an exact-membership ID SET, so a reorder or a
        // repeat admits exactly the same principals.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let consented = acknowledged_config(config_with_options(
            "c",
            "discord",
            "credential_handle = \"d\"\nallowed_channel_ids = [\"111\", \"222\"]\n",
            &open,
        ));
        for cosmetic in [
            "credential_handle = \"d\"\nallowed_channel_ids = [\"222\", \"111\"]\n",
            "credential_handle = \"d\"\nallowed_channel_ids = [\"222\", \"111\", \"111\"]\n",
        ] {
            let reordered = ChannelConfig {
                options: cosmetic.parse().expect("fixture parses"),
                ..consented.clone()
            };
            refuse_open_admission([&reordered]).unwrap_or_else(|e| {
                panic!("{cosmetic:?} admits exactly the same principals and must not refuse: {e}")
            });
        }

        // But the WILDCARD is NOT collapsed in `[options]`, unlike in the
        // `[inbound]` allowlists. No adapter honours `"*"` there — they are
        // exact-membership tests — so `["*"]` and `["*","111"]` admit DIFFERENT
        // sets and must not share a token.
        let star = ChannelConfig {
            options: "allowed_channel_ids = [\"*\"]\n".parse().expect("parses"),
            ..consented.clone()
        };
        let star_plus = ChannelConfig {
            options: "allowed_channel_ids = [\"*\", \"111\"]\n"
                .parse()
                .expect("parses"),
            ..consented.clone()
        };
        assert_ne!(
            admission_shape_token(&star.admission_shape()),
            admission_shape_token(&star_plus.admission_shape()),
            "an option list is an exact-membership test, so \"*\" subsumes nothing and the two \
             lists admit different sets"
        );
    }

    #[test]
    fn an_options_key_cannot_forge_a_token_field() {
        // `[options]` is operator-authored and schemaless, so its keys and
        // values are the one part of the token an attacker-shaped config file
        // controls. A key holding the field separator, the field/value
        // separator or the path separator must not be able to inject, rename or
        // merge a field.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let token = |options: &str| {
            admission_shape_token(
                &config_with_options("c", "slack", options, &open).admission_shape(),
            )
        };

        // A dotted key must not read as a nested path, or `{"a.b" = 1}` and
        // `{a = {b = 1}}` would render alike.
        assert_ne!(token("\"a.b\" = 1\n"), token("[a]\nb = 1\n"));

        // A key or value holding a space must not add a field to the token.
        for options in [
            "\"tools=full\" = 1\n",
            "\"x y\" = 1\n",
            "x = \"tools=full enabled=false\"\n",
        ] {
            let rendered = token(options);
            let parsed =
                parse_shape_token(&rendered).unwrap_or_else(|| panic!("must parse: {rendered}"));
            assert_eq!(
                parsed.len(),
                SHAPE_FIELDS.len() + 1,
                "{options:?} must contribute exactly ONE options field: {rendered}"
            );
            assert!(
                parsed
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .take(SHAPE_FIELDS.len())
                    .eq(SHAPE_FIELDS),
                "and must not have displaced a fixed field: {rendered}"
            );
        }

        // Type is part of the value: a schemaless table would otherwise let
        // `"1"` become `1` under a live consent.
        assert_ne!(token("x = 1\n"), token("x = \"1\"\n"));
        assert_ne!(token("x = true\n"), token("x = \"true\"\n"));
    }

    #[test]
    fn a_key_nobody_enumerated_is_inside_the_token() {
        // THE STRUCTURAL CLAIM, and the reason the token renders the WHOLE
        // `[options]` table rather than a declared key list. This gate has been
        // bypassed three times by a category nobody enumerated. A key that no
        // adapter in this tree has ever had, on a platform string nobody
        // registered, must already be inside the consent — with no
        // registration step to forget.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let consented = acknowledged_config(config_with_options(
            "c",
            "adapter-eleven",
            "admit_everyone_from = [\"someone\"]\n",
            &open,
        ));
        refuse_open_admission([&consented])
            .expect("control: the acknowledged shape of an unknown platform starts");

        let widened = ChannelConfig {
            options: "admit_everyone_from = []\n".parse().expect("parses"),
            ..consented
        };
        let msg = refuse_open_admission([&widened])
            .expect_err(
                "a key this crate has never heard of must still be bound by the consent, or the \
                 next adapter ships its admission filter outside the token",
            )
            .to_string();
        assert!(
            msg.contains("options.admit_everyone_from: acknowledged"),
            "and the refusal must name it; got: {msg}"
        );
    }

    #[test]
    fn a_v1_shape_token_is_named_as_such_rather_than_being_cryptic() {
        // An operator upgrading has an `admission-v1` token in a file that is
        // still open. That token was correct in its day but named neither
        // `platform` nor `[options]`, so honouring it would be honouring a
        // consent that cannot see an adapter filter being widened. It must be
        // refused, and the refusal must say why.
        let stale_v1 = "admission-v1 enabled=true dm=open dm_allowlist=0 group=disabled \
                        group_allowlist=0 sender_allowlist=0 require_mention=true \
                        tools=conversational tool_workspace_root=0";
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            acknowledge_open_admission: vec![stale_v1.to_string()],
            ..Default::default()
        };
        assert_eq!(
            faults_of(true, &open),
            vec![OpenAdmissionFault::Unreadable {
                token: stale_v1.to_string(),
                legacy: Some(LegacyToken::ShapeV1),
            }]
        );
        let msg = refuse_one(true, &open)
            .expect_err("a v1 token must not be honoured")
            .to_string();
        assert!(
            msg.contains("admission-v1") && msg.contains("allowed_senders"),
            "the refusal must name the old version and what it could not see; got: {msg}"
        );
        assert!(
            msg.contains(&format!(
                "acknowledge_open_admission = [{:?}]",
                token_of(true, &open)
            )),
            "and must print the token that replaces it; got: {msg}"
        );
    }

    /// Every field `T` DECLARES, read off the type rather than off a value.
    ///
    /// The first cut of this helper serialized a fixture and read the keys
    /// back, which is reflection over a VALUE and is defeated by serde's own
    /// omission attributes: a field carrying `#[serde(default,
    /// skip_serializing_if = "Vec::is_empty")]` is simply absent from the JSON
    /// whenever the fixture leaves it empty, so it was invisible to BOTH `..`
    /// guards below and a widening through it was unseeable — measured, not
    /// theorised. Populating the fixture harder is not a fix either: the next
    /// author adding the field writes `Vec::new()` to make it compile and the
    /// hole is back.
    ///
    /// So this asks the TYPE. Both structs are `#[serde(deny_unknown_fields)]`,
    /// which makes serde's derived `Deserialize` reject an unknown key with an
    /// error that enumerates the accepted ones — an enumeration generated from
    /// the definition, at the definition, with no value involved and therefore
    /// nothing an attribute on a field can hide. Delete `deny_unknown_fields`
    /// and the probe stops erroring: the `expect` below fails and the guards
    /// go red rather than quiet.
    fn declared_field_names<T: serde::de::DeserializeOwned>() -> Vec<String> {
        // A key no struct can plausibly declare, so the error is always the
        // unknown-field one and never a collision.
        const PROBE: &str = "__not_a_declared_field__";

        let err = serde_json::from_value::<T>(serde_json::json!({ PROBE: null }))
            .err()
            .map(|e| e.to_string())
            .expect(
                "the probe key must be REJECTED — if it deserialized, the struct lost its \
                 `#[serde(deny_unknown_fields)]` and this guard can no longer see its fields",
            );
        let tail = err
            .split_once("expected one of ")
            .or_else(|| err.split_once("expected "))
            .map(|(_, tail)| tail)
            .unwrap_or_else(|| {
                panic!("serde must enumerate the accepted fields; got: {err}");
            });
        // `` `a`, `b`, `c` `` -> the odd segments of a split on the backtick.
        let mut names: Vec<String> = tail
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        names.sort();
        names.dedup();
        assert!(
            names.len() > 1,
            "the field enumeration parsed out of serde's error is empty or degenerate, so the \
             guards below would pass vacuously; serde's message was: {err}"
        );
        names
    }

    /// POSITIVE CONTROL for [`declared_field_names`].
    ///
    /// The guards below compare a hand-maintained list against whatever this
    /// returns, so a parse that silently produced the WRONG list — or a list
    /// that happened to match — would make both of them decorative. This pins
    /// the mechanism against two structs whose fields are known here, and
    /// pins the negative direction too: a field that is NOT declared must not
    /// appear.
    #[test]
    fn the_field_enumeration_reads_the_type_not_a_value() {
        let inbound = declared_field_names::<InboundPolicy>();
        assert!(
            inbound.contains(&"acknowledge_open_admission".to_string())
                && inbound.contains(&"dm".to_string())
                && inbound.contains(&"ack".to_string()),
            "the enumeration must name InboundPolicy's own fields; got {inbound:?}"
        );
        assert!(
            !inbound.contains(&"__not_a_declared_field__".to_string()),
            "the probe key must not be reported as a declared field; got {inbound:?}"
        );

        let config = declared_field_names::<ChannelConfig>();
        assert!(
            config.contains(&"name".to_string()) && config.contains(&"inbound".to_string()),
            "the enumeration must name ChannelConfig's own fields; got {config:?}"
        );

        // The defeat this helper exists to close: a field whose VALUE serde
        // omits is still DECLARED, so it must still be enumerated. `ack`
        // serializes unconditionally today, so the control is built instead
        // from a local struct carrying the exact attribute that hid a field
        // from the previous, value-based helper.
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Omitted {
            kept: bool,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            hidden_when_empty: Vec<String>,
        }
        let fixture = Omitted {
            kept: true,
            hidden_when_empty: Vec::new(),
        };
        let serialized: Vec<String> = serde_json::to_value(&fixture)
            .expect("fixture must serialize")
            .as_object()
            .expect("a struct serializes to an object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            serialized,
            vec!["kept".to_string()],
            "control is wrong: this field is supposed to be INVISIBLE to value reflection"
        );
        assert_eq!(
            declared_field_names::<Omitted>(),
            vec!["hidden_when_empty".to_string(), "kept".to_string()],
            "reading the TYPE must see the field that reading the value cannot"
        );
    }

    fn sorted(values: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
        let mut out: Vec<String> = values.into_iter().map(Into::into).collect();
        out.sort();
        out
    }

    /// THE `..` GUARD, config side.
    ///
    /// `ChannelConfig::admission_shape` destructures exhaustively, so adding a
    /// sixth field is an E0027. That is the compile-time half — and rustc's own
    /// third suggestion for E0027 is "or always ignore missing fields here",
    /// i.e. add `..`, which compiles, keeps every other test in this module
    /// green, and silently re-opens the widening. A comment telling the next
    /// author not to take that suggestion is not a guard. This is.
    ///
    /// It reads the field set off serde and demands that every field be
    /// ACCOUNTED FOR: either proven to move the token, or named in the
    /// exclusion list below with its reason written at the binding. A field
    /// silenced by `..` is in neither, and this fails naming it.
    #[test]
    fn a_field_added_to_channel_config_cannot_be_left_out_of_the_consent() {
        // EXCLUSIONS. Empty, and that is the finding: `name` was the one
        // plausible candidate, and it turned out to be the first component of
        // every session key this channel's turns resume under. See the `name`
        // entry in `SHAPE_FIELDS`' documentation.
        const OUTSIDE_THE_SHAPE: [&str; 0] = [];

        let base = config("t", true, &open_over_one_group());
        let base_token = admission_shape_token(&base.admission_shape());

        // One mutation per field, each to a different but still-valid value.
        // A field is inside the consent iff its mutation moves the token.
        let bound: Vec<(&str, ChannelConfig)> = vec![
            (
                "name",
                ChannelConfig {
                    name: "renamed".into(),
                    ..base.clone()
                },
            ),
            (
                "platform",
                ChannelConfig {
                    platform: "discord".into(),
                    ..base.clone()
                },
            ),
            (
                "enabled",
                ChannelConfig {
                    enabled: false,
                    ..base.clone()
                },
            ),
            (
                "options",
                ChannelConfig {
                    options: "allowed_channel_ids = [\"C1\"]"
                        .parse()
                        .expect("fixture options must parse"),
                    ..base.clone()
                },
            ),
            (
                "inbound",
                ChannelConfig {
                    inbound: InboundPolicy {
                        group_allowlist: vec!["G2".into()],
                        ..base.inbound.clone()
                    },
                    ..base.clone()
                },
            ),
        ];

        assert_eq!(
            sorted(
                bound
                    .iter()
                    .map(|(field, _)| *field)
                    .chain(OUTSIDE_THE_SHAPE)
            ),
            declared_field_names::<ChannelConfig>(),
            "a field was added to or removed from ChannelConfig. Decide whether a consent is \
             bound to it: give it a mutation above if it is, or add it to OUTSIDE_THE_SHAPE and \
             write WHY it cannot widen reach or confidentiality at its binding in \
             `admission_shape`. Do not silence it with `..`"
        );

        for (field, mutated) in &bound {
            assert_ne!(
                admission_shape_token(&mutated.admission_shape()),
                base_token,
                "`{field}` is claimed to be inside the consent, but changing it did not change \
                 the token — so a consent written before the change silently covers after it"
            );
        }
    }

    /// THE `..` GUARD, `[inbound]` side. Same reasoning as its config sibling:
    /// `shape_fields` destructures `InboundPolicy` exhaustively, and this is
    /// what catches the `..` that would silence the E0027.
    #[test]
    fn a_field_added_to_inbound_policy_cannot_be_left_out_of_the_consent() {
        // EXCLUSIONS, with the reasons written at the bindings in
        // `shape_fields` and restated in `SHAPE_FIELDS`' documentation. The
        // negative direction is asserted below: an excluded field must NOT
        // move the token, or the exclusion is a lie.
        const OUTSIDE_THE_SHAPE: [&str; 2] = ["ack", "acknowledge_open_admission"];

        let base = open_over_one_group();
        let base_token = token_of(true, &base);

        // The `[inbound]` half of SHAPE_FIELDS is everything except the three
        // fields that come from `ChannelConfig`. Derived rather than retyped,
        // so the two lists cannot drift apart.
        let from_channel_config = ["name", "platform", "enabled"];
        let rendered_inbound: Vec<&str> = SHAPE_FIELDS
            .iter()
            .copied()
            .filter(|field| !from_channel_config.contains(field))
            .collect();

        assert_eq!(
            sorted(rendered_inbound.iter().copied().chain(OUTSIDE_THE_SHAPE)),
            declared_field_names::<InboundPolicy>(),
            "a field was added to or removed from InboundPolicy. Decide whether a consent is \
             bound to it: render it in `shape_fields` and add it to SHAPE_FIELDS if it is, or \
             discard it there with a written reason and add it to OUTSIDE_THE_SHAPE. Do not \
             silence it with `..`"
        );

        // The exclusions really are inert. `every_field_in_the_shape_is_load_\
        // bearing` proves the positive direction for the rendered ones.
        let noisier_ack = InboundPolicy {
            ack: AckMode::Reactions,
            ..base.clone()
        };
        assert_eq!(
            token_of(true, &noisier_ack),
            base_token,
            "`ack` is excluded, so it must not move the token"
        );
        let already_acked = InboundPolicy {
            acknowledge_open_admission: vec![base_token.clone()],
            ..base.clone()
        };
        assert_eq!(
            token_of(true, &already_acked),
            base_token,
            "`acknowledge_open_admission` is excluded, so it must not move the token"
        );
    }

    #[test]
    fn every_field_in_the_shape_is_load_bearing() {
        // The property that makes "consent is a function of the WHOLE shape"
        // true rather than aspirational: change any ONE field and the token
        // changes. A mutation that drops a field from the rendering — the exact
        // shape of the original defect — fails here.
        let base = open_over_one_group();
        let base_token = token_of(true, &base);

        // `platform` and `name` are not on `InboundPolicy`, so they are varied
        // through the config rather than through the variant list below.
        let other_platform = ChannelConfig {
            platform: "discord".to_string(),
            ..config("t", true, &base)
        };
        assert_ne!(
            admission_shape_token(&other_platform.admission_shape()),
            base_token,
            "changing `platform` must change the token: it decides which adapter reads [options], \
             and therefore what every key in it means"
        );

        let renamed = config("t2", true, &base);
        assert_ne!(
            admission_shape_token(&renamed.admission_shape()),
            base_token,
            "changing `name` must change the token: it is the first component of every session \
             key this channel's turns run under, so a rename re-points them at another channel's \
             persisted history"
        );

        let variants: Vec<(&str, bool, InboundPolicy)> = vec![
            ("enabled", false, base.clone()),
            (
                "dm",
                true,
                InboundPolicy {
                    dm: DmPolicy::Disabled,
                    ..base.clone()
                },
            ),
            (
                "dm_allowlist",
                true,
                InboundPolicy {
                    dm_allowlist: vec!["U2".into()],
                    ..base.clone()
                },
            ),
            (
                "group",
                true,
                InboundPolicy {
                    group: GroupPolicy::Open,
                    ..base.clone()
                },
            ),
            (
                "group_allowlist",
                true,
                InboundPolicy {
                    group_allowlist: vec!["G2".into()],
                    ..base.clone()
                },
            ),
            (
                "sender_allowlist",
                true,
                InboundPolicy {
                    sender_allowlist: vec!["U9".into()],
                    ..base.clone()
                },
            ),
            (
                "require_mention",
                true,
                InboundPolicy {
                    require_mention: false,
                    ..base.clone()
                },
            ),
            (
                "tools",
                true,
                InboundPolicy {
                    tools: ChannelToolPosture::Full,
                    ..base.clone()
                },
            ),
            (
                "tool_workspace_root",
                true,
                InboundPolicy {
                    tool_workspace_root: Some("/jail".into()),
                    ..base.clone()
                },
            ),
            (
                "group_sessions_per_user",
                true,
                InboundPolicy {
                    group_sessions_per_user: false,
                    ..base.clone()
                },
            ),
            (
                "thread_sessions_per_user",
                true,
                InboundPolicy {
                    thread_sessions_per_user: true,
                    ..base.clone()
                },
            ),
        ];
        assert_eq!(
            variants.len() + 2,
            SHAPE_FIELDS.len(),
            "every field in SHAPE_FIELDS must have a variant here (+2 for `name` and `platform`, \
             varied above), or a field could be dropped from the rendering with nothing to catch \
             it"
        );

        let mut seen = vec![base_token.clone()];
        for (field, enabled, policy) in &variants {
            let token = token_of(*enabled, policy);
            assert_ne!(
                token, base_token,
                "changing {field} must change the token, or a consent written before the change \
                 silently covers after it"
            );
            assert!(
                !seen.contains(&token),
                "two different shapes produced the same token at {field}: {token}"
            );
            seen.push(token);
        }
    }

    /// EVERY FIELD RENDERS UNDER ITS OWN KEY.
    ///
    /// `every_field_in_the_shape_is_load_bearing` proves each field moves the
    /// token, and `token_field_order_is_pinned` proves the key list and its
    /// order. Neither one binds a key to the field it is supposed to carry:
    /// swap the two rendered values in `shape_fields`
    ///
    /// ```ignore
    /// ("name".into(), escape_entry(platform)),
    /// ("platform".into(), escape_entry(name)),
    /// ```
    ///
    /// and the key list, the arity and the order are all untouched, every
    /// field still moves the token, and the whole module stays green — an
    /// adversarial seat did exactly this and scored 39 passed / 0 failed. That
    /// is a live hole and not a cosmetic one: the refusal diff prints
    /// `platform: acknowledged <x>, now <y>` for a change to `name`, so the
    /// operator re-consents to the wrong sentence.
    ///
    /// This closes it by mutating ONE field at a time and demanding that the
    /// set of token entries that changed is EXACTLY the one key that field is
    /// declared to render. A swap fails immediately: mutating `name` changes
    /// the entry under `platform`.
    #[test]
    fn every_field_renders_under_its_own_key() {
        let base = config("t", true, &open_over_one_group());

        /// The entries of a token, by key. The token is produced by
        /// `admission_shape_token`, so it must parse.
        fn entries(cfg: &ChannelConfig) -> std::collections::BTreeMap<String, String> {
            parse_shape_token(&admission_shape_token(&cfg.admission_shape()))
                .expect("a token this crate just wrote must parse")
                .into_iter()
                .collect()
        }

        let inbound = |f: fn(&mut InboundPolicy)| {
            let mut policy = base.inbound.clone();
            f(&mut policy);
            ChannelConfig {
                inbound: policy,
                ..base.clone()
            }
        };

        // One mutation per SHAPE_FIELDS key, each touching ONLY the field that
        // key is supposed to carry.
        let cases: Vec<(&str, ChannelConfig)> = vec![
            (
                "name",
                ChannelConfig {
                    name: "renamed".into(),
                    ..base.clone()
                },
            ),
            (
                "platform",
                ChannelConfig {
                    platform: "discord".into(),
                    ..base.clone()
                },
            ),
            (
                "enabled",
                ChannelConfig {
                    enabled: false,
                    ..base.clone()
                },
            ),
            ("dm", inbound(|p| p.dm = DmPolicy::Disabled)),
            (
                "dm_allowlist",
                inbound(|p| p.dm_allowlist = vec!["U2".into()]),
            ),
            ("group", inbound(|p| p.group = GroupPolicy::Open)),
            (
                "group_allowlist",
                inbound(|p| p.group_allowlist = vec!["G2".into()]),
            ),
            (
                "sender_allowlist",
                inbound(|p| p.sender_allowlist = vec!["U9".into()]),
            ),
            (
                "require_mention",
                inbound(|p| p.require_mention = !p.require_mention),
            ),
            ("tools", inbound(|p| p.tools = ChannelToolPosture::Full)),
            (
                "tool_workspace_root",
                inbound(|p| p.tool_workspace_root = Some("/jail".into())),
            ),
            (
                "group_sessions_per_user",
                inbound(|p| p.group_sessions_per_user = !p.group_sessions_per_user),
            ),
            (
                "thread_sessions_per_user",
                inbound(|p| p.thread_sessions_per_user = !p.thread_sessions_per_user),
            ),
        ];
        assert!(
            cases.iter().map(|(key, _)| *key).eq(SHAPE_FIELDS),
            "every SHAPE_FIELDS key needs a case here, in order, or a key could be bound to the \
             wrong field with nothing to catch it"
        );

        let base_entries = entries(&base);
        for (key, mutated) in &cases {
            let mutated_entries = entries(mutated);
            // The union, so a mutation that ADDS or REMOVES a key counts as a
            // change to it rather than disappearing from the comparison.
            let changed: Vec<&str> = base_entries
                .keys()
                .chain(mutated_entries.keys())
                .map(String::as_str)
                .filter(|k| base_entries.get(*k) != mutated_entries.get(*k))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            assert_eq!(
                changed,
                vec![*key],
                "mutating the field behind `{key}` must change the `{key}` entry and nothing \
                 else. It changed {changed:?} instead, so at least one key renders another \
                 field's value — the refusal diff then names the wrong setting and the operator \
                 re-consents to the wrong sentence"
            );
        }
    }

    #[test]
    fn the_two_excluded_fields_are_excluded_on_purpose() {
        // `shape_fields` destructures `InboundPolicy` without `..`, so the two
        // discarded bindings are a written decision rather than an oversight.
        // This pins the decision from the other side: including either one
        // would be a defect, not an improvement.
        let base = open_over_one_group();
        let base_token = token_of(true, &base);

        // `ack` is outbound presentation — a reaction or a typing indicator on
        // a message the bot is already working on. It moves neither reach nor
        // authority, so charging the operator a re-acknowledgement for it would
        // be pure over-refusal.
        let noisier_ack = InboundPolicy {
            ack: AckMode::Reactions,
            ..base.clone()
        };
        assert_eq!(
            token_of(true, &noisier_ack),
            base_token,
            "`ack` must NOT be in the token: it cannot change who is admitted, what an admitted \
             turn may read, or what it may do to this host"
        );

        // `acknowledge_open_admission` is the field that HOLDS the token. If it
        // were rendered, the token would have to contain itself: writing the
        // token an operator was told to write would immediately change the
        // token they now need, and no acknowledgement could ever converge.
        let already_acked = InboundPolicy {
            acknowledge_open_admission: vec![base_token.clone()],
            ..base.clone()
        };
        assert_eq!(
            token_of(true, &already_acked),
            base_token,
            "`acknowledge_open_admission` must NOT be in the token it holds, or acknowledging a \
             channel would invalidate the acknowledgement that was just written"
        );

        // And the pair really is the whole exclusion set: everything else on
        // the struct is load-bearing, which the sibling test proves field by
        // field. Two excluded + `name`, `platform` and `enabled` coming from
        // `ChannelConfig` rather than `InboundPolicy` is the arithmetic.
        assert_eq!(
            SHAPE_FIELDS.len(),
            13,
            "SHAPE_FIELDS changed size: re-decide the exclusions above rather than editing this \
             number"
        );
    }

    #[test]
    fn a_token_cannot_be_forged_by_an_allowlist_entry_that_looks_like_a_separator() {
        // Without escaping, `["a,b"]` and `["a","b"]` would render alike while
        // admitting different senders — a collision that would let a widening
        // pass as unchanged.
        let one_weird_entry = InboundPolicy {
            group_allowlist: vec!["a,b".into()],
            ..open_over_one_group()
        };
        let two_plain_entries = InboundPolicy {
            group_allowlist: vec!["a".into(), "b".into()],
            ..open_over_one_group()
        };
        assert_ne!(
            token_of(true, &one_weird_entry),
            token_of(true, &two_plain_entries)
        );

        // An empty list permits nobody; a list holding the empty string permits
        // the sender whose id is "". Different sets, different tokens.
        let empty = InboundPolicy {
            group_allowlist: vec![],
            ..open_over_one_group()
        };
        let empty_string = InboundPolicy {
            group_allowlist: vec![String::new()],
            ..open_over_one_group()
        };
        assert_ne!(token_of(true, &empty), token_of(true, &empty_string));

        // A space in an id must not be readable as the FIELD separator, or a
        // crafted id could inject a field the operator never wrote.
        let spaced = InboundPolicy {
            group_allowlist: vec!["G1 sender_allowlist=0".into()],
            ..open_over_one_group()
        };
        let token = token_of(true, &spaced);
        assert_eq!(
            token.split(' ').count(),
            SHAPE_FIELDS.len() + 1,
            "an id must never add a field to the token: {token}"
        );
        assert!(
            parse_shape_token(&token).is_some(),
            "and the escaped token must still round-trip: {token}"
        );
    }

    #[test]
    fn a_token_round_trips_and_a_near_miss_never_acknowledges_anything() {
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let token = token_of(true, &open);
        let parsed = parse_shape_token(&token).expect("a token this module wrote must parse back");
        assert!(
            parsed.iter().map(|(k, _)| k.as_str()).eq(SHAPE_FIELDS),
            "the parse must recover every field, in order: {parsed:?}"
        );

        // ATTACK VARIANT. If the match were fuzzy — trimmed, case-folded,
        // prefix-based, or tolerant of a missing field — an operator (or a
        // config generator) could write a consent that covers more than one
        // shape, and the binding would leak back to a field-name list.
        let mut near_misses: Vec<String> = vec![
            "dm=open".into(),                                       // the old per-field spelling
            "group=open".into(),                                    // another old spelling
            "".into(),                                              // empty
            WILDCARD.into(),                                        // a wildcard acknowledgement
            token.to_uppercase(),                                   // case
            format!(" {token}"),                                    // leading space
            format!("{token} "),                                    // trailing space
            format!("{token} extra=1"),                             // an extra field
            token.replacen("enabled=true", "", 1),                  // a field removed
            token.replace(ADMISSION_SHAPE_VERSION, "admission-v0"), // wrong version
        ];
        // The same token with ONE field's value altered is not a near-miss
        // spelling but a different shape; it must refuse too, by the other arm.
        near_misses.push(token.replace("dm=open", "dm=disabled"));

        for spelling in near_misses {
            let policy = InboundPolicy {
                acknowledge_open_admission: vec![spelling.clone()],
                ..open.clone()
            };
            let faults = faults_of(true, &policy);
            assert!(
                !faults.is_empty(),
                "{spelling:?} must NOT acknowledge this channel"
            );
        }

        // POSITIVE CONTROL: the exact token, and only the exact token, passes.
        assert!(faults_of(true, &acknowledged(true, &open)).is_empty());
    }

    #[test]
    fn narrowing_a_channel_refuses_until_the_stale_consent_is_removed() {
        // Fails CLOSED, deliberately, even though the new configuration is
        // SAFER. Accepting a leftover consent silently is exactly how a token
        // written for a bounded moment ends up covering an open one.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        };
        let token = token_of(true, &open);
        let narrowed = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U-NAMED".into()],
            acknowledge_open_admission: vec![token.clone()],
            ..Default::default()
        };
        let refusal = refuse_one(true, &narrowed)
            .expect_err("a leftover consent must be cleared deliberately, not honoured silently");
        assert_eq!(
            refusal.channels[0].faults,
            vec![OpenAdmissionFault::Stale {
                token: token.clone()
            }]
        );
        let msg = refusal.to_string();
        assert!(
            msg.contains(&format!("{token:?}"))
                && msg.contains("nothing in this channel admits an unbounded set of senders"),
            "the refusal must quote the consent that no longer matches; got: {msg}"
        );
    }

    #[test]
    fn the_legacy_per_field_token_is_named_as_such_rather_than_being_cryptic() {
        // An operator upgrading from the first cut has `["dm=open"]` in a file
        // that is still open. The refusal must explain what happened, not just
        // reject a string.
        let open = InboundPolicy {
            dm: DmPolicy::Open,
            acknowledge_open_admission: vec!["dm=open".into()],
            ..Default::default()
        };
        let refusal = refuse_one(true, &open).expect_err("the old spelling must not be honoured");
        assert_eq!(
            refusal.channels[0].faults,
            vec![OpenAdmissionFault::Unreadable {
                token: "dm=open".into(),
                legacy: Some(LegacyToken::PerField)
            }]
        );
        let msg = refusal.to_string();
        assert!(
            msg.contains("older per-field spelling"),
            "the refusal must say what the old token was; got: {msg}"
        );
        assert!(
            msg.contains(&format!(
                "acknowledge_open_admission = [{:?}]",
                token_of(true, &open)
            )),
            "and must print the token that replaces it; got: {msg}"
        );
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
                                // And every flagged config must be SATISFIABLE:
                                // the token the gate demands must be the token
                                // the gate accepts. A shape the operator cannot
                                // acknowledge is a wedge, not a gate.
                                for enabled in [false, true] {
                                    refuse_one(enabled, &acknowledged(enabled, &policy))
                                        .unwrap_or_else(|e| {
                                            panic!(
                                                "the advertised token must be accepted for \
                                             {policy:?} (enabled={enabled}): {e}"
                                            )
                                        });
                                }
                            } else {
                                assert!(
                                    open.is_empty(),
                                    "this config admits NOBODY unnamed, so flagging it would be \
                                     an over-refusal an operator cannot satisfy: {policy:?} -> \
                                     {open:?}"
                                );
                                // A bounded config needs no consent at all, at
                                // either `enabled` setting.
                                for enabled in [false, true] {
                                    assert!(
                                        required_of(enabled, &policy).is_empty(),
                                        "a bounded config must demand no acknowledgement: \
                                         {policy:?}"
                                    );
                                }
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
    fn every_policy_variant_has_a_named_verdict_under_the_most_open_lists() {
        // The sweep above walks the space; this pins each VARIANT's verdict by
        // name, under the widest lists it could be paired with, so the reason
        // for each answer is written down where the next person reads it.
        //
        // It exists because `open_admissions` used to end each match with
        // `_ => {}`. Under a catch-all, a new posture — however permissive —
        // was silently classified "bounded", and no test in this file could
        // have noticed: the sweep only enumerates the variants that exist when
        // it is written. The `matches!` tripwires below stop compiling when a
        // variant is added, which is the part a runtime assertion cannot do.
        let wide = |dm: DmPolicy, group: GroupPolicy| InboundPolicy {
            dm,
            group,
            dm_allowlist: vec![WILDCARD.into()],
            group_allowlist: vec!["G1".into()],
            sender_allowlist: vec![WILDCARD.into()],
            ..Default::default()
        };

        // DM axis, group held Disabled so only the DM verdict can speak.
        for (dm, expect) in [
            (DmPolicy::Open, vec!["dm"]),
            (DmPolicy::Allowlist, vec!["dm_allowlist"]),
            // The variant the note is about. A pairing channel admits the
            // senders who redeemed an operator-issued code — a named set — so
            // it is NOT an open admission, in this tree where it is
            // fail-closed and on the branch where the handshake is live. The
            // inert `"*"` in `dm_allowlist` must not be flagged either.
            (DmPolicy::Pairing, vec![]),
            (DmPolicy::Disabled, vec![]),
        ] {
            assert!(
                matches!(
                    dm,
                    DmPolicy::Open | DmPolicy::Allowlist | DmPolicy::Pairing | DmPolicy::Disabled
                ),
                "tripwire: a new DmPolicy variant must be classified in `open_admissions` and \
                 given a row here, not absorbed by a catch-all"
            );
            let policy = wide(dm.clone(), GroupPolicy::Disabled);
            let got: Vec<&str> = open_admissions(&policy).iter().map(|o| o.field).collect();
            assert_eq!(got, expect, "dm = {}", dm_policy_name(&dm));
        }

        // Group axis, DM held Disabled for the same reason.
        for (group, expect) in [
            (GroupPolicy::Open, vec!["group"]),
            (GroupPolicy::Allowlist, vec!["sender_allowlist"]),
            (GroupPolicy::Disabled, vec![]),
        ] {
            assert!(
                matches!(
                    group,
                    GroupPolicy::Open | GroupPolicy::Allowlist | GroupPolicy::Disabled
                ),
                "tripwire: a new GroupPolicy variant must be classified in `open_admissions` and \
                 given a row here, not absorbed by a catch-all"
            );
            let policy = wide(DmPolicy::Disabled, group.clone());
            let got: Vec<&str> = open_admissions(&policy).iter().map(|o| o.field).collect();
            assert_eq!(got, expect, "group = {}", group_policy_name(&group));
        }
    }

    #[test]
    fn every_open_shape_is_reported_and_the_advertised_token_is_the_accepted_one() {
        // Drive every open shape through `open_admissions` rather than listing
        // the constants, so a new shape added without a remedy cannot slip past.
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
        let mut tokens: Vec<String> = Vec::new();
        for policy in &shapes {
            let found = open_admissions(policy);
            assert_eq!(found.len(), 1, "each fixture is exactly one open shape");

            // The refusal must NAME what is open, whatever the fault is — an
            // operator asked to re-consent has to see what they consent to.
            let refusal =
                refuse_one(true, policy).expect_err("an unacknowledged open shape refuses");
            let msg = refusal.to_string();
            assert!(
                msg.contains(&found[0].found) && msg.contains(found[0].consequence()),
                "the refusal must quote the setting and its consequence; got: {msg}"
            );
            assert!(
                msg.contains("nothing acknowledges this configuration"),
                "and say the consent is absent; got: {msg}"
            );

            let token = token_of(true, policy);
            assert!(
                !tokens.contains(&token),
                "each shape must have its own token: {token:?}"
            );
            tokens.push(token.clone());

            // The token the refusal advertises must be the one the gate accepts.
            assert!(
                msg.contains(&format!("acknowledge_open_admission = [{token:?}]")),
                "the refusal must advertise the token; got: {msg}"
            );
            assert!(
                faults_of(true, &acknowledged(true, policy)).is_empty(),
                "the advertised token must be the one the gate accepts: {token:?}"
            );
        }
        assert_eq!(tokens.len(), 4);
    }
}
