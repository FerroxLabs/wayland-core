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
        "initialize" | "session/list" | "session/get" | "agents/list" => Role::Viewer,
        "session/create" | "message/send" | "a2a/handshake" | "a2a/message/send" => Role::Operator,
        "session/delete" | "support/bundle" => Role::Admin,
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

    #[test]
    fn role_parsing_is_case_and_whitespace_tolerant_but_not_permissive() {
        assert_eq!(Role::parse("  Admin "), Some(Role::Admin));
        assert_eq!(Role::parse("OPERATOR"), Some(Role::Operator));
        // Tolerance stops at recognisable names — no prefix matching, which
        // would make "admin-readonly" an admin.
        assert_eq!(Role::parse("admin-readonly"), None);
    }
}
