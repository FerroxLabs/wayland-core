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
    /// the exact list and for what is deliberately outside it. Any change to
    /// any of those settings changes the token and demands fresh consent.
    ///
    /// # What is cosmetic, precisely
    ///
    /// Allowlists are normalised before rendering: sorted, de-duplicated, and
    /// collapsed to `*` when they hold the wildcard (which subsumes every other
    /// entry). `["G1","G2"]` and `["G2","G1","G2"]` therefore produce the same
    /// token, because they admit the same principals. `["G1"]` and `["*"]` do
    /// not. Refusing a reordering would teach operators that the gate cries
    /// wolf, and an operator who stops reading refusals reaches for the
    /// nuclear option — which is the failure mode this key exists to prevent.
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
pub const ADMISSION_SHAPE_VERSION: &str = "admission-v1";

/// Every setting a consent is bound to, in token order.
///
/// # The rule for what is in here
///
/// A field belongs in the shape iff it decides **who is admitted, what an
/// admitted principal can trigger without addressing the bot, or what the
/// resulting turn is allowed to do to this host**:
///
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
///
/// # What is deliberately OUT, and why
///
/// `ack` (how the bot signals it is working) and the two session-shaping flags
/// (`group_sessions_per_user`, `thread_sessions_per_user`). None of them change
/// who reaches the agent or what the turn may do, and every field in here is a
/// field whose edit costs the operator a re-acknowledgement — so the line is
/// drawn at reach and authority, not at "everything on the struct".
pub const SHAPE_FIELDS: [&str; 9] = [
    "enabled",
    "dm",
    "dm_allowlist",
    "group",
    "group_allowlist",
    "sender_allowlist",
    "require_mention",
    "tools",
    "tool_workspace_root",
];

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

/// Percent-escape the characters that would otherwise make a token ambiguous:
/// the entry separator, the escape character itself, and anything whitespace or
/// control (the field separator is a space).
///
/// Without this, `["a,b"]` and `["a","b"]` would render identically while
/// admitting different senders — a collision that would let a widening pass as
/// unchanged.
fn escape_entry(entry: &str) -> String {
    let mut out = String::with_capacity(entry.len());
    for ch in entry.chars() {
        match ch {
            '%' => out.push_str("%25"),
            ',' => out.push_str("%2C"),
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

/// `(field, canonical value)` for every field in [`SHAPE_FIELDS`], in order.
fn shape_fields(enabled: bool, policy: &InboundPolicy) -> Vec<(&'static str, String)> {
    let fields = vec![
        ("enabled", enabled.to_string()),
        ("dm", dm_policy_name(&policy.dm).to_string()),
        ("dm_allowlist", canonical_list(&policy.dm_allowlist)),
        ("group", group_policy_name(&policy.group).to_string()),
        ("group_allowlist", canonical_list(&policy.group_allowlist)),
        ("sender_allowlist", canonical_list(&policy.sender_allowlist)),
        ("require_mention", policy.require_mention.to_string()),
        ("tools", posture_name(policy.tools).to_string()),
        (
            "tool_workspace_root",
            canonical_opt(policy.tool_workspace_root.as_deref()),
        ),
    ];
    debug_assert!(
        fields.iter().map(|(k, _)| *k).eq(SHAPE_FIELDS),
        "the rendered fields and SHAPE_FIELDS must not drift: the parser trusts the order"
    );
    fields
}

/// The canonical token for one channel's admission shape.
///
/// This is what [`InboundPolicy::acknowledge_open_admission`] must hold, and it
/// is a rendering rather than a hash on purpose: a refusal has to be able to
/// print what CHANGED, and nothing can be recovered from a digest. An operator
/// told only "hash mismatch" learns nothing and reaches for the nuclear option.
pub fn admission_shape_token(enabled: bool, policy: &InboundPolicy) -> String {
    let mut out = String::from(ADMISSION_SHAPE_VERSION);
    for (field, value) in shape_fields(enabled, policy) {
        out.push(' ');
        out.push_str(field);
        out.push('=');
        out.push_str(&value);
    }
    out
}

/// Parse a token back into `(field, value)` pairs, or `None` if it is not a
/// token this version wrote. Field names and their order must match
/// [`SHAPE_FIELDS`] exactly — a token missing a field would otherwise be a
/// consent that silently says nothing about it.
fn parse_shape_token(token: &str) -> Option<Vec<(&'static str, String)>> {
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
        out.push((expected, value.to_string()));
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// The per-field tokens the FIRST cut of this key used. Recognised only so an
/// operator upgrading from that shape is told what happened instead of being
/// handed a bare "this is not a token".
const LEGACY_TOKENS: [&str; 4] = [
    "dm=open",
    "dm_allowlist=*",
    "group=open",
    "sender_allowlist=*",
];

/// One field whose value differs between the acknowledged shape and the live
/// one. Carried so the refusal can name the change rather than the mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionFieldChange {
    /// The setting that changed, spelled as in [`SHAPE_FIELDS`].
    pub field: &'static str,
    /// Its canonical value in the token the operator wrote.
    pub acknowledged: String,
    /// Its canonical value in the configuration as it now stands.
    pub current: String,
}

/// The `acknowledge_open_admission` value that matches this channel EXACTLY as
/// it stands: one [`admission_shape_token`] when anything is open, and nothing
/// otherwise.
///
/// Empty when nothing is open — in which case the correct file has no
/// `acknowledge_open_admission` key at all, because a token present while
/// nothing is open is a stale consent, not a harmless leftover.
pub fn required_acknowledgement(enabled: bool, policy: &InboundPolicy) -> Vec<String> {
    if open_admissions(policy).is_empty() {
        Vec::new()
    } else {
        vec![admission_shape_token(enabled, policy)]
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
        /// True when it is one of the per-field tokens the first cut of this
        /// key used, so the refusal can say so rather than being cryptic.
        legacy: bool,
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
pub fn open_admission_faults(enabled: bool, policy: &InboundPolicy) -> Vec<OpenAdmissionFault> {
    let open = open_admissions(policy);
    let acks = &policy.acknowledge_open_admission;

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
                    legacy: LEGACY_TOKENS.contains(&token.as_str()),
                }];
            };
            let changes: Vec<AdmissionFieldChange> = acknowledged
                .into_iter()
                .zip(shape_fields(enabled, policy))
                .filter(|((_, was), (_, now))| was != now)
                .map(|((field, was), (_, now))| AdmissionFieldChange {
                    field,
                    acknowledged: was,
                    current: now,
                })
                .collect();
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
                                change.field, change.acknowledged, change.current
                            )?;
                        }
                    }
                    OpenAdmissionFault::Unreadable { token, legacy } => {
                        write!(
                            f,
                            "  channel {channel:?}: acknowledge_open_admission names {token:?}, \
                             which is not an admission-shape token."
                        )?;
                        if *legacy {
                            write!(
                                f,
                                " That is the older per-field spelling. Consent is now bound to \
                                 this channel's WHOLE admission shape, because a per-field list \
                                 could not see a widening of a field it did not name."
                            )?;
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
            "The token names this channel's ENTIRE admission shape — {} — so changing any of \
             them refuses again instead of being covered by the old consent. Reordering or \
             repeating an allowlist entry admits exactly the same principals and is NOT a change. \
             This key is read only from the profile-scoped channel file; a project-local \
             .wayland-core.toml cannot set it.",
            SHAPE_FIELDS.join(", ")
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
/// Each item is `(channel name, enabled, policy)`. `enabled` is threaded in
/// rather than read off the policy because it lives on
/// [`crate::config::ChannelConfig`], one level up.
pub fn refuse_open_admission<'a>(
    channels: impl IntoIterator<Item = (&'a str, bool, &'a InboundPolicy)>,
) -> Result<(), OpenAdmissionRefusal> {
    let refused: Vec<ChannelOpenAdmission> = channels
        .into_iter()
        .filter_map(|(name, enabled, policy)| {
            let faults = open_admission_faults(enabled, policy);
            if faults.is_empty() {
                return None;
            }
            Some(ChannelOpenAdmission {
                channel: name.to_string(),
                open: open_admissions(policy),
                faults,
                required_acknowledgement: required_acknowledgement(enabled, policy),
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

    /// `policy` with its own required acknowledgement written in.
    fn acknowledged(enabled: bool, policy: &InboundPolicy) -> InboundPolicy {
        InboundPolicy {
            acknowledge_open_admission: required_acknowledgement(enabled, policy),
            ..policy.clone()
        }
    }

    #[test]
    fn a_blanket_pre_acknowledgement_cannot_be_written_in_the_first_place() {
        // This is the whole defect, at the level of the pure gate: if a bounded
        // channel accepts a consent for shapes it does not have, that file is a
        // standing consent to every future opening.
        let refusal = refuse_open_admission([("preacked", true, &preacked_but_bounded())])
            .expect_err("pre-arming a bounded channel must refuse, not be silently accepted");
        let refused = &refusal.channels[0];
        assert_eq!(refused.channel, "preacked");
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
        let required = required_acknowledgement(true, &open);
        assert_eq!(
            required,
            vec![admission_shape_token(true, &open)],
            "an open channel requires exactly its own shape token"
        );
        let exact = acknowledged(true, &open);
        refuse_open_admission([("exact", true, &exact)])
            .expect("an acknowledgement that names the live shape exactly must be accepted");

        // The SAME token twice. Under the first cut this was accepted, which
        // made the list a bag rather than the set the docs claim.
        let doubled = InboundPolicy {
            acknowledge_open_admission: vec![required[0].clone(), required[0].clone()],
            ..open.clone()
        };
        let refusal = refuse_open_admission([("doubled", true, &doubled)])
            .expect_err("a repeated consent is not an exact match");
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
            refuse_open_admission([("extra", true, &extra)])
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
        refuse_open_admission([("narrow", true, &narrow)])
            .expect("control: the narrow shape, acknowledged, is accepted");

        let widened = InboundPolicy {
            group_allowlist: vec![WILDCARD.into()],
            ..narrow.clone()
        };
        let refusal = refuse_open_admission([("widened", true, &widened)])
            .expect_err("widening a field the consent did not single out must still refuse");
        let faults = &refusal.channels[0].faults;
        assert!(
            matches!(
                faults.as_slice(),
                [OpenAdmissionFault::ShapeChanged { changes, .. }]
                    if changes.len() == 1
                        && changes[0].field == "group_allowlist"
                        && changes[0].acknowledged == "1:G1"
                        && changes[0].current == WILDCARD
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
        refuse_open_admission([("off", false, &off)])
            .expect("control: the switched-off shape, acknowledged, is accepted");

        let refusal = refuse_open_admission([("on", true, &off)])
            .expect_err("flipping enabled must be a new decision");
        assert!(
            matches!(
                refusal.channels[0].faults.as_slice(),
                [OpenAdmissionFault::ShapeChanged { changes, .. }]
                    if changes.len() == 1
                        && changes[0].field == "enabled"
                        && changes[0].acknowledged == "false"
                        && changes[0].current == "true"
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
            refuse_open_admission([("bounded", enabled, &bounded)])
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
        refuse_open_admission([("floor", true, &consented)])
            .expect("control: the acknowledged floor starts");

        for escalated in [ChannelToolPosture::Workspace, ChannelToolPosture::Full] {
            let policy = InboundPolicy {
                tools: escalated,
                ..consented.clone()
            };
            let refusal = match refuse_open_admission([("escalated", true, &policy)]) {
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
        refuse_open_admission([("jailed", true, &jailed)])
            .expect("control: the jailed shape starts");
        let widened = InboundPolicy {
            tool_workspace_root: Some("/".into()),
            ..jailed.clone()
        };
        let refusal = refuse_open_admission([("widened", true, &widened)])
            .expect_err("moving the jail root must demand fresh consent");
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
            refuse_open_admission([("cosmetic", true, &reordered)]).unwrap_or_else(|e| {
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
        assert_eq!(
            admission_shape_token(true, &star),
            admission_shape_token(true, &star_plus)
        );
    }

    #[test]
    fn every_field_in_the_shape_is_load_bearing() {
        // The property that makes "consent is a function of the WHOLE shape"
        // true rather than aspirational: change any ONE field and the token
        // changes. A mutation that drops a field from the rendering — the exact
        // shape of the original defect — fails here.
        let base = open_over_one_group();
        let base_token = admission_shape_token(true, &base);

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
        ];
        assert_eq!(
            variants.len(),
            SHAPE_FIELDS.len(),
            "every field in SHAPE_FIELDS must have a variant here, or a field could be dropped \
             from the rendering with nothing to catch it"
        );

        let mut seen = vec![base_token.clone()];
        for (field, enabled, policy) in &variants {
            let token = admission_shape_token(*enabled, policy);
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
            admission_shape_token(true, &one_weird_entry),
            admission_shape_token(true, &two_plain_entries)
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
        assert_ne!(
            admission_shape_token(true, &empty),
            admission_shape_token(true, &empty_string)
        );

        // A space in an id must not be readable as the FIELD separator, or a
        // crafted id could inject a field the operator never wrote.
        let spaced = InboundPolicy {
            group_allowlist: vec!["G1 sender_allowlist=0".into()],
            ..open_over_one_group()
        };
        let token = admission_shape_token(true, &spaced);
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
        let token = admission_shape_token(true, &open);
        let parsed = parse_shape_token(&token).expect("a token this module wrote must parse back");
        assert!(
            parsed.iter().map(|(k, _)| *k).eq(SHAPE_FIELDS),
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
            let faults = open_admission_faults(true, &policy);
            assert!(
                !faults.is_empty(),
                "{spelling:?} must NOT acknowledge this channel"
            );
        }

        // POSITIVE CONTROL: the exact token, and only the exact token, passes.
        assert!(open_admission_faults(true, &acknowledged(true, &open)).is_empty());
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
        let token = admission_shape_token(true, &open);
        let narrowed = InboundPolicy {
            dm: DmPolicy::Allowlist,
            dm_allowlist: vec!["U-NAMED".into()],
            acknowledge_open_admission: vec![token.clone()],
            ..Default::default()
        };
        let refusal = refuse_open_admission([("narrowed", true, &narrowed)])
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
        let refusal = refuse_open_admission([("upgraded", true, &open)])
            .expect_err("the old spelling must not be honoured");
        assert_eq!(
            refusal.channels[0].faults,
            vec![OpenAdmissionFault::Unreadable {
                token: "dm=open".into(),
                legacy: true
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
                admission_shape_token(true, &open)
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
                                    refuse_open_admission([(
                                        "sat",
                                        enabled,
                                        &acknowledged(enabled, &policy),
                                    )])
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
                                        required_acknowledgement(enabled, &policy).is_empty(),
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
            let refusal = refuse_open_admission([("s", true, policy)])
                .expect_err("an unacknowledged open shape refuses");
            let msg = refusal.to_string();
            assert!(
                msg.contains(&found[0].found) && msg.contains(found[0].consequence()),
                "the refusal must quote the setting and its consequence; got: {msg}"
            );
            assert!(
                msg.contains("nothing acknowledges this configuration"),
                "and say the consent is absent; got: {msg}"
            );

            let token = admission_shape_token(true, policy);
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
                open_admission_faults(true, &acknowledged(true, policy)).is_empty(),
                "the advertised token must be the one the gate accepts: {token:?}"
            );
        }
        assert_eq!(tokens.len(), 4);
    }
}
