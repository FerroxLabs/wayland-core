//! Role-based command authorization for the typed client.
//!
//! Phase 24 Success Criterion 4, threat T-24-03-02.
//!
//! # A refusal is not an authentication failure
//!
//! The whole point of this module is that `AcpError::Forbidden` and
//! `AcpError::Auth` are DIFFERENT outcomes and stay different all the way to
//! the operator. They call for opposite responses:
//!
//! - `Auth` — "I do not know who you are." Check the credential.
//! - `Forbidden` — "I know exactly who you are, and you may not do this."
//!   Check the ROLE. The credential is fine and rotating it will not help.
//!
//! Collapsing them into one 401-shaped answer is how an operator spends an
//! afternoon regenerating a working API key.
//!
//! # Roles gate at the SERVER, and the default is deny
//!
//! Authorization is decided here, from the principal the server itself
//! verified — never from anything the client sent. A principal carrying no
//! recognised role gets [`Role::none`] semantics: zero capabilities, refused
//! for everything including reads. That is stricter than defaulting to the
//! lowest real role, and deliberately so: an unrecognised role string is
//! usually a typo in an operator's configuration, and the safe reading of "I
//! do not understand this role" is "grant nothing", not "grant the minimum".

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::auth::Principal;

/// What a principal is allowed to do. Ordered least to most privileged.
///
/// The ordering is real and is used by [`Role::satisfies`] — `Admin` satisfies
/// a requirement for `Operator`. Do NOT reorder these variants without
/// re-reading every `satisfies` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Role {
    /// Read-only: list and inspect sessions, subscribe to events.
    Viewer,
    /// Everything a viewer may do, plus driving turns and creating sessions.
    Operator,
    /// Everything an operator may do, plus deleting sessions and producing a
    /// support bundle.
    Admin,
}

impl Role {
    /// Parse a configured role string. An unrecognised string is `None`,
    /// which the authorization path treats as DENY-ALL — see the module docs.
    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(Role::Viewer),
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }

    /// The absence of a role. Not a variant, because "no role" must never be
    /// storable as if it were a grant.
    pub fn none() -> Option<Role> {
        None
    }

    /// Whether holding `self` satisfies a requirement for `required`.
    pub fn satisfies(self, required: Role) -> bool {
        self >= required
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

/// A verified principal together with the role the SERVER assigned it.
///
/// Wraps rather than replaces [`Principal`]: the existing verifiers keep
/// producing principals exactly as before, and the role is attached
/// afterwards from server-side configuration. Nothing a client sends can
/// influence this field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoledPrincipal {
    pub principal: Principal,
    /// `None` means no recognised role — deny everything.
    pub role: Option<Role>,
}

impl RoledPrincipal {
    pub fn new(principal: Principal, role: Option<Role>) -> Self {
        Self { principal, role }
    }
}

/// The minimum role each ACP method requires.
///
/// # An unknown method requires `Admin`, not `Viewer`
///
/// A method this table does not know about is a method added without being
/// classified. Falling through to the least privilege would silently expose
/// every new endpoint to every principal; falling through to the most
/// privilege makes the omission LOUD — the new endpoint stops working for
/// ordinary callers, someone notices, and the table gets an entry.
pub fn required_role(method: &str) -> Role {
    match method {
        // `session/events` is the RESUME read — a disconnected client asking
        // for the events it missed. It reveals exactly what a live subscriber
        // already received, so it is classified with the other reads rather
        // than falling through to the Admin default.
        "initialize"
        | "session/list"
        | "session/get"
        | "session/events"
        | "agents/list"
        | "tools/list"
        | "health"
        | "approvals/projects/list" => Role::Viewer,
        "session/create"
        | "message/send"
        | "session/approval/resolve"
        | "a2a/handshake"
        | "a2a/message/send" => Role::Operator,
        // #305 c2: editing the project allowlist GRANTS (or revokes)
        // unattended tool execution for a directory tree. That is an operator
        // authority decision, not a session operation, so it sits with the
        // other Admin methods rather than with `session/create`.
        "session/delete"
        | "support/bundle"
        | "approvals/projects/set"
        | "approvals/projects/delete" => Role::Admin,
        _ => Role::Admin,
    }
}

/// The outcome of an authorization decision. A distinct type rather than a
/// bare `bool` so a caller cannot accidentally treat "denied" as "ok" by
/// ignoring a return value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum AuthzOutcome {
    Allowed,
    Denied {
        method: String,
        required: Role,
        held: Option<Role>,
    },
}

impl AuthzOutcome {
    pub fn is_allowed(&self) -> bool {
        matches!(self, AuthzOutcome::Allowed)
    }

    /// Render the refusal as the crate error, which the transports map to a
    /// FORBIDDEN status — never to an authentication challenge.
    ///
    /// The message names the required role and the held one, because the
    /// operator's next action is to change a role and they need to know to
    /// what. It does NOT name the principal id: the refusal travels back to a
    /// caller who may not be entitled to learn whose credential this is.
    pub fn into_error(self) -> Option<crate::error::AcpError> {
        match self {
            AuthzOutcome::Allowed => None,
            AuthzOutcome::Denied {
                method,
                required,
                held,
            } => Some(crate::error::AcpError::Forbidden(format!(
                "{method} requires role {}; caller holds {}",
                required.as_str(),
                held.map(Role::as_str).unwrap_or("no recognised role")
            ))),
        }
    }
}

/// The server-side assignment of roles to verified principals.
///
/// # Why this type exists rather than a bare `HashMap`
///
/// [`authorize`] needs a [`RoledPrincipal`], and nothing was producing one:
/// the verifiers return a bare [`Principal`] and the role had no source. That
/// is the entire reason the authorization contract sat unreachable. This is
/// the missing half — the SERVER's own statement of who holds what, consulted
/// after verification and never influenced by the request.
///
/// # An unnamed principal gets no role, not the lowest one
///
/// [`Self::role_for`] returns `None` for a principal the policy does not name,
/// unless a default was set EXPLICITLY with [`Self::with_default_role`]. A
/// policy is an operator's enumeration of who may do what; a principal missing
/// from it is an omission, and the safe reading of an omission is "grant
/// nothing". Granting `Viewer` on an omission is a grant nobody wrote down.
///
/// # Installing no policy at all is a DIFFERENT state, and it is observable
///
/// [`crate::AcpServer::has_role_policy`] reports whether one is installed. With
/// none, the server performs NO role gating and every authenticated principal
/// reaches every method — which is exactly the pre-role behaviour, kept so
/// installing this crate's update cannot lock an existing operator out of their
/// own gateway. That state is not a green and must never be reported as one:
/// it is "role gating is not configured", and the distinction is asserted by
/// `an_uninstalled_policy_is_reported_as_absent_not_as_a_deny_all`.
#[derive(Debug, Clone, Default)]
pub struct RolePolicy {
    by_principal: HashMap<String, Role>,
    default_role: Option<Role>,
}

impl RolePolicy {
    /// An empty policy: every principal is unnamed, therefore denied.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a principal id to a role.
    pub fn grant(mut self, principal_id: impl Into<String>, role: Role) -> Self {
        self.by_principal.insert(principal_id.into(), role);
        self
    }

    /// Set the role an unnamed principal receives. Deliberately explicit —
    /// there is no way to reach a non-`None` default by accident.
    pub fn with_default_role(mut self, role: Role) -> Self {
        self.default_role = Some(role);
        self
    }

    /// The role held by `principal_id`, or the explicit default, or `None`.
    pub fn role_for(&self, principal_id: &str) -> Option<Role> {
        self.by_principal
            .get(principal_id)
            .copied()
            .or(self.default_role)
    }

    /// Attach this policy's verdict to a verified principal.
    pub fn attach(&self, principal: &Principal) -> RoledPrincipal {
        RoledPrincipal::new(principal.clone(), self.role_for(&principal.id))
    }

    /// Decide `method` for `principal`, returning the crate error on refusal.
    ///
    /// This is the call site the transports use, so the refusal a client sees
    /// and the decision this module makes are the same object rather than two
    /// implementations that can drift.
    pub fn authorize(
        &self,
        principal: &Principal,
        method: &str,
    ) -> Result<(), crate::error::AcpError> {
        match authorize(&self.attach(principal), method).into_error() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Decide whether `who` may issue `method`.
pub fn authorize(who: &RoledPrincipal, method: &str) -> AuthzOutcome {
    let required = required_role(method);
    match who.role {
        Some(held) if held.satisfies(required) => AuthzOutcome::Allowed,
        held => AuthzOutcome::Denied {
            method: method.to_string(),
            required,
            held,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthSchemeKind;
    use crate::error::AcpError;

    fn who(role: Option<Role>) -> RoledPrincipal {
        RoledPrincipal::new(
            Principal {
                id: "acct-1".into(),
                scheme: AuthSchemeKind::ApiKey,
            },
            role,
        )
    }

    #[test]
    fn a_role_refusal_is_a_different_error_from_an_authentication_failure() {
        // THE property of this module. If these ever collapse, an operator
        // rotates a working credential and the refusal does not move.
        let denied = authorize(&who(Some(Role::Viewer)), "session/delete");
        let err = denied.into_error().expect("a denial must carry an error");
        assert!(
            matches!(err, AcpError::Forbidden(_)),
            "a role refusal must be Forbidden, not Auth; got {err:?}"
        );
        assert!(
            !matches!(err, AcpError::Auth(_)),
            "Forbidden and Auth must stay distinct variants"
        );
    }

    #[test]
    fn the_refusal_names_the_required_role_and_the_held_one() {
        let err = authorize(&who(Some(Role::Viewer)), "session/delete")
            .into_error()
            .unwrap()
            .to_string();
        assert!(err.contains("admin"), "must name what is required: {err}");
        assert!(err.contains("viewer"), "must name what is held: {err}");
    }

    #[test]
    fn the_refusal_does_not_name_the_principal() {
        // The refusal travels to a caller who may not be entitled to learn
        // whose credential this is.
        let err = authorize(&who(Some(Role::Viewer)), "session/delete")
            .into_error()
            .unwrap()
            .to_string();
        assert!(!err.contains("acct-1"), "leaked the principal id: {err}");
    }

    #[test]
    fn no_recognised_role_is_denied_everything_including_reads() {
        // Deny-all, not least-privilege. An unrecognised role string is
        // normally a configuration typo, and granting the minimum on a typo is
        // a grant nobody wrote down.
        for method in [
            "initialize",
            "session/list",
            "session/create",
            "session/delete",
        ] {
            assert!(
                !authorize(&who(None), method).is_allowed(),
                "a principal with no recognised role must be denied {method}"
            );
        }
        assert_eq!(Role::parse("superuser"), None);
        assert_eq!(Role::parse(""), None);
        assert_eq!(Role::none(), None);
    }

    #[test]
    fn a_higher_role_satisfies_a_lower_requirement() {
        assert!(authorize(&who(Some(Role::Admin)), "session/list").is_allowed());
        assert!(authorize(&who(Some(Role::Admin)), "message/send").is_allowed());
        assert!(authorize(&who(Some(Role::Operator)), "session/list").is_allowed());
        assert!(authorize(&who(Some(Role::Viewer)), "session/list").is_allowed());
    }

    #[test]
    fn a_lower_role_does_not_satisfy_a_higher_requirement() {
        // The mirror of the case above. Without it, `satisfies` returning
        // `true` unconditionally would pass every other test here.
        assert!(!authorize(&who(Some(Role::Viewer)), "message/send").is_allowed());
        assert!(!authorize(&who(Some(Role::Operator)), "session/delete").is_allowed());
        assert!(!authorize(&who(Some(Role::Viewer)), "session/create").is_allowed());
    }

    #[test]
    fn an_unclassified_method_requires_admin_so_the_omission_is_loud() {
        // A method added without a table entry must FAIL for ordinary callers
        // rather than silently becoming world-readable.
        assert_eq!(required_role("some/method/added/later"), Role::Admin);
        assert!(!authorize(&who(Some(Role::Operator)), "some/method/added/later").is_allowed());
    }

    // ── RolePolicy: the half that was missing, so `authorize` had no caller ──

    #[test]
    fn a_policy_grants_the_named_role_and_denies_above_it() {
        let policy = RolePolicy::new().grant("acct-1", Role::Operator);
        let p = who(None).principal;
        // Positive control first: the grant really does open something, so the
        // refusal below is attributable to the requirement and not to the
        // policy failing to grant anything at all.
        assert!(policy.authorize(&p, "message/send").is_ok());
        let err = policy
            .authorize(&p, "session/delete")
            .expect_err("an operator may not delete");
        assert!(matches!(err, AcpError::Forbidden(_)), "got {err:?}");
    }

    #[test]
    fn a_principal_the_policy_does_not_name_is_denied_everything() {
        // Not "granted the lowest role". A policy is an enumeration; a missing
        // principal is an omission, and an omission must not become a grant.
        let policy = RolePolicy::new().grant("somebody-else", Role::Admin);
        let p = who(None).principal;
        assert_eq!(policy.role_for(&p.id), None);
        for method in ["initialize", "session/list", "message/send"] {
            assert!(
                policy.authorize(&p, method).is_err(),
                "an unnamed principal must be denied {method}"
            );
        }
    }

    #[test]
    fn an_explicit_default_role_applies_only_where_it_was_asked_for() {
        // The default exists so an operator can say "everyone authenticated is
        // a viewer" out loud. It must never appear without being written.
        let bare = RolePolicy::new();
        assert_eq!(bare.role_for("anyone"), None);
        let defaulted = RolePolicy::new().with_default_role(Role::Viewer);
        assert_eq!(defaulted.role_for("anyone"), Some(Role::Viewer));
        // An explicit grant still beats the default.
        let mixed = RolePolicy::new()
            .with_default_role(Role::Viewer)
            .grant("acct-1", Role::Admin);
        assert_eq!(mixed.role_for("acct-1"), Some(Role::Admin));
        assert_eq!(mixed.role_for("acct-2"), Some(Role::Viewer));
    }

    #[test]
    fn the_resume_read_is_classified_rather_than_falling_through_to_admin() {
        // If `session/events` were left unclassified it would require Admin and
        // an ordinary operator could never resume a stream it was allowed to
        // receive live — a refusal with no security meaning.
        assert_eq!(required_role("session/events"), Role::Viewer);
        assert!(authorize(&who(Some(Role::Viewer)), "session/events").is_allowed());
        // …and it is still a real gate: no role at all is still refused.
        assert!(!authorize(&who(None), "session/events").is_allowed());
    }

    #[test]
    fn role_parsing_is_case_and_whitespace_tolerant_but_not_permissive() {
        assert_eq!(Role::parse("  Admin "), Some(Role::Admin));
        assert_eq!(Role::parse("OPERATOR"), Some(Role::Operator));
        // Tolerance stops at recognisable names — no prefix matching, which
        // would make "admin-readonly" an admin.
        assert_eq!(Role::parse("admin-readonly"), None);
    }
}
