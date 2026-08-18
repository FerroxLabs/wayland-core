//! macOS TCC (Transparency, Consent & Control) permission layer.
//!
//! The macOS backend synthesizes input with `CGEvent` and captures the
//! display with `CGDisplayCreateImageForRect`. Both are TCC-protected:
//! without the grant the calls do not fail loudly, they simply do
//! nothing (input) or hand back a null image (capture). Every such
//! failure used to reach the caller as an opaque
//! [`CuaError::Backend`](crate::error::CuaError::Backend).
//!
//! This module makes the permission state an explicit value:
//!
//! - [`probe`] answers "do we hold this grant?" and **never shows a
//!   dialog**. Safe on any path, including an unattended agent run and
//!   `wayland-core --doctor`.
//! - [`prime`] **does** show the system consent dialog. It must only be
//!   reached from an explicit, user-initiated command
//!   (`wayland-core --request-permissions`). An agent run must never
//!   surprise a user with a TCC prompt, so nothing on the op-dispatch
//!   path may call it — locked in by
//!   `the_prompting_apis_never_appear_on_the_dispatch_path`.
//! - [`permission_gate`] turns an observed status into the typed
//!   [`CuaError::PermissionDenied`](crate::error::CuaError::PermissionDenied),
//!   which names the exact System Settings pane.
//!
//! The module compiles on every target (like `backends::linux_wayland`)
//! so the gate, the op-to-capability mapping and the test-grading
//! helpers are auditable and runnable on non-macOS hosts. Only the two
//! `sys` calls are `#[cfg(target_os = "macos")]`.

use serde::{Deserialize, Serialize};

use crate::error::CuaError;
use crate::op::CuaOp;

/// A macOS TCC-protected capability the CUA backend depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TccCapability {
    /// Synthesized input — `CGEvent` click / move / scroll / type /
    /// key. Probed with `AXIsProcessTrusted()`.
    Accessibility,
    /// Display capture — `CGDisplayCreateImageForRect`. Probed with
    /// `CGPreflightScreenCaptureAccess()`.
    ScreenRecording,
}

impl TccCapability {
    /// Every capability, for callers that report on all of them.
    pub const ALL: [TccCapability; 2] = [Self::Accessibility, Self::ScreenRecording];

    /// The exact System Settings pane the user must visit. On macOS the
    /// pane name and the capability name coincide, so this doubles as
    /// the human label.
    pub const fn settings_pane(self) -> &'static str {
        match self {
            Self::Accessibility => "Accessibility",
            Self::ScreenRecording => "Screen Recording",
        }
    }

    /// Actionable remediation naming the exact pane, phrased to match
    /// `IMessageError::AutomationDenied`.
    pub fn remediation(self) -> String {
        format!(
            "{} permission denied — grant in System Settings → Privacy & Security → {}, \
             then restart wayland-core",
            self.settings_pane(),
            self.settings_pane()
        )
    }
}

/// The observed grant state of a [`TccCapability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TccStatus {
    /// The grant is held; the guarded API will do real work.
    Granted,
    /// The grant is absent. The guarded API would silently no-op.
    Denied,
    /// Not a macOS build — TCC does not exist on this platform.
    NotApplicable,
}

/// Which capability an op needs before it may touch the platform API.
///
/// Deliberately platform-independent so the mapping is gradable on
/// every host. `Wait` touches nothing. `AxTree` is not implemented on
/// the macOS backend yet, and `FrontmostApp` goes through `osascript`,
/// which is guarded by the **Automation** pane rather than either
/// capability here — both stay ungated until they have a real
/// implementation to gate (issue #114 covers Accessibility and Screen
/// Recording only).
pub fn required_capability(op: &CuaOp) -> Option<TccCapability> {
    match op {
        CuaOp::LeftClick { .. }
        | CuaOp::RightClick { .. }
        | CuaOp::DoubleClick { .. }
        | CuaOp::MouseMove { .. }
        | CuaOp::Scroll { .. }
        | CuaOp::Type { .. }
        | CuaOp::Key { .. } => Some(TccCapability::Accessibility),
        CuaOp::Screenshot { .. } => Some(TccCapability::ScreenRecording),
        CuaOp::Wait { .. } | CuaOp::AxTree {} | CuaOp::FrontmostApp {} => None,
    }
}

/// The single place a missing grant becomes an error.
///
/// It always produces the typed [`CuaError::PermissionDenied`] carrying
/// the capability — never a generic `Backend` error. Pure, so the
/// contract is verifiable without a Mac.
pub fn permission_gate(capability: TccCapability, status: TccStatus) -> Result<(), CuaError> {
    match status {
        TccStatus::Granted | TccStatus::NotApplicable => Ok(()),
        TccStatus::Denied => Err(CuaError::PermissionDenied { capability }),
    }
}

/// Non-prompting probe: "do we hold this grant?".
///
/// Calls `AXIsProcessTrusted()` / `CGPreflightScreenCaptureAccess()`,
/// neither of which raises a dialog, so this is safe to call from any
/// path including op dispatch and `--doctor`.
pub fn probe(capability: TccCapability) -> TccStatus {
    #[cfg(target_os = "macos")]
    {
        sys::probe(capability)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capability;
        TccStatus::NotApplicable
    }
}

/// Prompting request: ask macOS to show the consent dialog, then report
/// the resulting state.
///
/// Calls `AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt:
/// true])` / `CGRequestScreenCaptureAccess()`. **This shows UI.** Only
/// call it from an explicit user-initiated command — never from op
/// dispatch, bootstrap, or `--doctor`.
///
/// Accessibility consent is not granted in-process: macOS opens the
/// Settings pane and the grant only takes effect after the user adds
/// the binary and it is restarted, so a `Denied` return here is the
/// normal first-run answer, not a failure.
pub fn prime(capability: TccCapability) -> TccStatus {
    #[cfg(target_os = "macos")]
    {
        sys::prime(capability)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capability;
        TccStatus::NotApplicable
    }
}

// -- test-grading helpers ---------------------------------------------
// These live in the crate (not in a test module) so the macOS backend
// tests and the always-compiled tests below grade against exactly the
// same rule. The point is that "permission denied" must stop being a
// universal pass.

/// How a dispatch outcome measured against a capability that was
/// observed **denied before the call**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeniedOutcome {
    /// The op returned exactly [`CuaError::PermissionDenied`] for this
    /// capability. The ONLY tolerated shape.
    TypedPermissionError,
    /// The op reported the denial as a generic backend failure — the
    /// defect the previous tests could not see, because they accepted
    /// any `Backend` error as a pass.
    SwallowedIntoBackend(String),
    /// The op failed some other way.
    OtherError(String),
    /// The op claimed success while the capability was denied.
    UnexpectedSuccess,
}

/// Classify a dispatch outcome taken while `capability` was denied.
///
/// This is the narrow, explicit tolerance that replaces the old blanket
/// "a `Backend` error is fine" rule.
pub fn classify_denied_outcome<T>(
    capability: TccCapability,
    outcome: &Result<T, CuaError>,
) -> DeniedOutcome {
    match outcome {
        Ok(_) => DeniedOutcome::UnexpectedSuccess,
        Err(CuaError::PermissionDenied { capability: got }) if *got == capability => {
            DeniedOutcome::TypedPermissionError
        }
        Err(CuaError::Backend(msg)) => DeniedOutcome::SwallowedIntoBackend(msg.clone()),
        Err(e) => DeniedOutcome::OtherError(e.to_string()),
    }
}

/// Guard for a test that can only mean anything under a specific
/// permission state.
///
/// Returns `true` when this host actually holds `required`. Otherwise
/// it prints a greppable `SKIP:` line naming the test and the reason
/// and returns `false` — the caller must return without asserting. A
/// test that needs a grant this host lacks SKIPS with a reason; it does
/// not pass by pretending the unauthorised result was the authorised
/// one.
#[must_use]
pub fn skip_unless(capability: TccCapability, required: TccStatus, test_name: &str) -> bool {
    let actual = probe(capability);
    if actual == required {
        return true;
    }
    eprintln!(
        "SKIP: {test_name} — needs {} to be {required:?} on this host, observed {actual:?}. \
         Grant/revoke it in System Settings → Privacy & Security → {} and re-run.",
        capability.settings_pane(),
        capability.settings_pane()
    );
    false
}

// -- macOS FFI --------------------------------------------------------

#[cfg(target_os = "macos")]
mod sys {
    use core_foundation::base::{Boolean, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::access::ScreenCaptureAccess;

    use super::{TccCapability, TccStatus};

    // `AXIsProcessTrusted` and friends live in ApplicationServices
    // (HIServices). `kAXTrustedCheckOptionPrompt` is the only option
    // key the trust check accepts; passing it as `true` is what makes
    // the call prompting rather than silent.
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrusted() -> Boolean;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    }

    fn status(granted: bool) -> TccStatus {
        if granted {
            TccStatus::Granted
        } else {
            TccStatus::Denied
        }
    }

    /// Non-prompting. Both calls are documented as dialog-free.
    pub(super) fn probe(capability: TccCapability) -> TccStatus {
        match capability {
            // SAFETY: no arguments, no out-params; returns a `Boolean`
            // (`u8`) by value. Callable from any thread.
            TccCapability::Accessibility => status(unsafe { AXIsProcessTrusted() } != 0),
            TccCapability::ScreenRecording => status(ScreenCaptureAccess.preflight()),
        }
    }

    /// Prompting. Raises the TCC dialog / opens the Settings pane.
    pub(super) fn prime(capability: TccCapability) -> TccStatus {
        match capability {
            TccCapability::Accessibility => {
                // SAFETY: `kAXTrustedCheckOptionPrompt` is a framework
                // global CFStringRef; we wrap it under the get rule so
                // CF retain/release stays balanced. The dictionary
                // outlives the call.
                let key: CFString =
                    unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
                let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
                // SAFETY: passes a valid, live CFDictionaryRef; the
                // callee does not take ownership.
                let trusted =
                    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) } != 0;
                status(trusted)
            }
            TccCapability::ScreenRecording => status(ScreenCaptureAccess.request()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{KeyMods, MouseButton, Region, ScreenshotFormat};

    fn click() -> CuaOp {
        CuaOp::LeftClick {
            x: 1,
            y: 1,
            button: MouseButton::Left,
            mods: KeyMods::default(),
        }
    }

    fn shot() -> CuaOp {
        CuaOp::Screenshot {
            region: Region::Full,
            format: ScreenshotFormat::Png,
            redact: false,
        }
    }

    /// The remediation must name the exact pane, not a generic
    /// "check your settings". This is the whole user-facing value of
    /// the typed error.
    #[test]
    fn remediation_names_the_exact_settings_pane() {
        for cap in TccCapability::ALL {
            let msg = cap.remediation();
            assert!(
                msg.contains("System Settings → Privacy & Security → "),
                "remediation must name the settings path: {msg}"
            );
            assert!(
                msg.contains(cap.settings_pane()),
                "remediation must name the pane {}: {msg}",
                cap.settings_pane()
            );
        }
        assert_ne!(
            TccCapability::Accessibility.remediation(),
            TccCapability::ScreenRecording.remediation(),
            "the two capabilities must not send the user to the same pane"
        );
    }

    /// The typed error's Display is what an agent shows the user, so the
    /// remediation must survive the trip through `CuaError`.
    #[test]
    fn the_typed_error_renders_the_remediation() {
        for cap in TccCapability::ALL {
            let e = CuaError::PermissionDenied { capability: cap };
            assert_eq!(e.to_string(), cap.remediation());
        }
    }

    /// Input ops need Accessibility, capture needs Screen Recording,
    /// and the mapping must not collapse the two — a click that
    /// reported the Screen Recording pane would send the user to the
    /// wrong place.
    #[test]
    fn ops_map_to_the_capability_that_actually_guards_them() {
        assert_eq!(
            required_capability(&click()),
            Some(TccCapability::Accessibility)
        );
        assert_eq!(
            required_capability(&shot()),
            Some(TccCapability::ScreenRecording)
        );
        assert_eq!(required_capability(&CuaOp::Wait { duration_ms: 0 }), None);
    }

    /// Every input variant must be gated. A new op added to `CuaOp`
    /// that synthesizes input and is left ungated would ship an
    /// unguarded TCC call, so the mapping is asserted variant by
    /// variant rather than by sampling one click.
    #[test]
    fn every_input_synthesizing_op_is_gated_on_accessibility() {
        let input_ops = [
            click(),
            CuaOp::RightClick {
                x: 1,
                y: 1,
                mods: KeyMods::default(),
            },
            CuaOp::DoubleClick {
                x: 1,
                y: 1,
                button: MouseButton::Left,
            },
            CuaOp::MouseMove { x: 1, y: 1 },
            CuaOp::Scroll {
                x: 1,
                y: 1,
                dx: 0,
                dy: 1,
            },
            CuaOp::Type { text: "x".into() },
            CuaOp::Key {
                keys: "a".into(),
                mods: KeyMods::default(),
            },
        ];
        for op in input_ops {
            assert_eq!(
                required_capability(&op),
                Some(TccCapability::Accessibility),
                "input op left ungated: {op:?}"
            );
        }
    }

    #[test]
    fn the_gate_passes_a_granted_capability_and_is_inert_off_macos() {
        assert!(permission_gate(TccCapability::Accessibility, TccStatus::Granted).is_ok());
        assert!(permission_gate(TccCapability::Accessibility, TccStatus::NotApplicable).is_ok());
    }

    /// The gate must emit the typed error carrying the capability —
    /// not a `Backend` string. This is the assertion the red arm
    /// mutates.
    #[test]
    fn the_gate_emits_the_typed_permission_error_for_a_denied_capability() {
        for cap in TccCapability::ALL {
            match permission_gate(cap, TccStatus::Denied) {
                Err(CuaError::PermissionDenied { capability }) => assert_eq!(capability, cap),
                other => panic!("denied {cap:?} must produce PermissionDenied, got {other:?}"),
            }
        }
    }

    /// **The vacuity guard.** The old suite accepted any `Backend`
    /// error mentioning `CGDisplay` as a pass, so a permission failure
    /// swallowed into a generic backend error was indistinguishable
    /// from a healthy run — on every host, forever.
    ///
    /// This asserts both halves: the old rule DOES accept the swallowed
    /// error (so the vacuity was real), and the replacement rule
    /// REFUSES it.
    #[test]
    fn a_permission_failure_swallowed_into_a_backend_error_is_no_longer_tolerated() {
        let swallowed: Result<(), CuaError> = Err(CuaError::Backend(
            "CGDisplayCreateImageForRect returned null".into(),
        ));

        // The retired rule, verbatim in behaviour: "a Backend error
        // mentioning CGDisplay is fine".
        let old_rule_accepts =
            matches!(&swallowed, Err(CuaError::Backend(m)) if m.contains("CGDisplay"));
        assert!(
            old_rule_accepts,
            "the old tolerance really did accept a swallowed permission failure"
        );

        // The replacement rule refuses it, and says why.
        assert_eq!(
            classify_denied_outcome(TccCapability::ScreenRecording, &swallowed),
            DeniedOutcome::SwallowedIntoBackend(
                "CGDisplayCreateImageForRect returned null".to_string()
            ),
        );
    }

    /// The narrow tolerance: exactly one outcome shape is accepted
    /// under a denied capability.
    #[test]
    fn only_the_typed_permission_error_is_tolerated_under_a_denied_capability() {
        let typed: Result<(), CuaError> = Err(CuaError::PermissionDenied {
            capability: TccCapability::ScreenRecording,
        });
        assert_eq!(
            classify_denied_outcome(TccCapability::ScreenRecording, &typed),
            DeniedOutcome::TypedPermissionError
        );

        // Right shape, WRONG capability — would send the user to the
        // wrong Settings pane, so it is not tolerated either.
        let wrong_cap: Result<(), CuaError> = Err(CuaError::PermissionDenied {
            capability: TccCapability::Accessibility,
        });
        assert!(matches!(
            classify_denied_outcome(TccCapability::ScreenRecording, &wrong_cap),
            DeniedOutcome::OtherError(_)
        ));

        // A success while denied is a false green, not a pass.
        let bogus: Result<(), CuaError> = Ok(());
        assert_eq!(
            classify_denied_outcome(TccCapability::ScreenRecording, &bogus),
            DeniedOutcome::UnexpectedSuccess
        );
    }

    /// Off macOS there is no TCC, so probing must be inert and must NOT
    /// masquerade as a grant or a denial.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn probing_off_macos_is_not_applicable_rather_than_granted_or_denied() {
        for cap in TccCapability::ALL {
            assert_eq!(probe(cap), TccStatus::NotApplicable);
            assert_eq!(prime(cap), TccStatus::NotApplicable);
        }
    }

    /// `skip_unless` must refuse to let a test proceed under the wrong
    /// permission state. Off macOS nothing is ever `Granted`, so the
    /// authorised arm of every backend test skips instead of passing.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn skip_unless_refuses_the_authorised_arm_when_the_grant_is_absent() {
        assert!(!skip_unless(
            TccCapability::ScreenRecording,
            TccStatus::Granted,
            "self-test"
        ));
    }

    /// Strip `//` comments so a source-level guard grades CODE, not
    /// prose. `macos.rs` carries no `//` inside a string literal, so
    /// splitting each line on the first `//` is exact here; the caller
    /// positive-controls that the strip actually removed something.
    fn code_only(src: &str) -> String {
        src.lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **The prompt must never fire implicitly.** An agent run that
    /// raised a TCC dialog would be a surprise the user never asked
    /// for, so the prompting APIs are barred from the dispatch path by
    /// construction: they live only in this module's `sys` block and
    /// are reachable only through [`prime`].
    ///
    /// The scan is over comment-stripped source. A guard that matched
    /// prose would fire on the dispatch gate's own explanatory comment
    /// — and, worse, could be satisfied by a comment while the real
    /// call sat one line below.
    #[test]
    fn the_prompting_apis_never_appear_on_the_dispatch_path() {
        let raw = include_str!("backends/macos.rs");
        let code = code_only(raw);

        // Positive control: the stripper must actually have removed
        // comment text, otherwise this test grades nothing.
        assert!(
            raw.contains("macOS TCC gate."),
            "expected the dispatch gate comment in the source under test"
        );
        assert!(
            !code.contains("macOS TCC gate."),
            "comment stripping did not run — the guard below would be scanning prose"
        );

        for forbidden in [
            "AXIsProcessTrustedWithOptions",
            "kAXTrustedCheckOptionPrompt",
            "CGRequestScreenCaptureAccess",
            "ScreenCaptureAccess",
            "permissions::prime",
        ] {
            assert!(
                !code.contains(forbidden),
                "the macOS backend must never reach a prompting TCC API ({forbidden}); \
                 dispatch may only call the non-prompting `permissions::probe`"
            );
        }

        // And the gate really is wired: the non-prompting probe is
        // called from code, not merely described in a comment.
        assert!(
            code.contains("permissions::probe"),
            "the macOS backend must gate dispatch on the non-prompting probe"
        );
        assert!(
            code.contains("permissions::permission_gate"),
            "the macOS backend must convert a denied grant through the typed gate"
        );
    }
}
