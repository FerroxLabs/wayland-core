import io

p = "crates/wcore-types/src/tool.rs"
s = io.open(p, encoding="utf-8").read()
anchor = "/// Maximum chars kept from a deferred tool's description."
assert s.count(anchor) == 1, s.count(anchor)

block = '''/// Reconcilers that certify one class of invocation could not have created an
/// external effect.
///
/// A [`ToolEffectKind::RepeatSafe`] declaration on its own is only a claim.
/// The recovery surfaces act on it — they write a durable `NotStarted` receipt
/// with no human asked — so the claim needs a NAMED certifier that recovery
/// recognises. That is exactly what [`ToolEffectContract::reconciler`] already
/// documents: `None` means no automatic reconciler is available.
///
/// Each constant names the evidence its class rests on, and each is checked by
/// [`repeat_safe_reconciler_is_registered`] before any receipt is written. An
/// unrecognised name resolves nothing and the effect stays in front of an
/// operator, so a tool cannot mint recovery authority by inventing a
/// reconciler identifier of its own.
///
/// `Read`, `Grep`, `Glob`: they open files and never write one.
pub const READ_ONLY_FILESYSTEM_RECONCILER: &str = "wcore.filesystem.read_only.v1";

/// A shell command a static classifier proved cannot mutate anything: one
/// simple command, no shell metacharacters at all, and a program drawn from a
/// small set with no write mode and no dispatch to a user-configured helper.
pub const READ_ONLY_SHELL_RECONCILER: &str = "wcore.shell.read_only.v1";

/// A retrieval whose request could not ask for a state change — `WebFetch`,
/// which issues an HTTP GET with no body, and the `search` operation of the
/// web tool, whose backend contract is a query.
pub const READ_ONLY_NETWORK_RECONCILER: &str = "wcore.network.read_only.v1";

/// An MCP tool whose own server declared `readOnlyHint: true` in `tools/list`.
pub const READ_ONLY_MCP_RECONCILER: &str = "wcore.mcp.read_only.v1";

const REPEAT_SAFE_RECONCILERS: &[&str] = &[
    READ_ONLY_FILESYSTEM_RECONCILER,
    READ_ONLY_SHELL_RECONCILER,
    READ_ONLY_NETWORK_RECONCILER,
    READ_ONLY_MCP_RECONCILER,
];

/// Does recovery know how to act on this repeat-safe reconciler name?
#[must_use]
pub fn repeat_safe_reconciler_is_registered(name: &str) -> bool {
    REPEAT_SAFE_RECONCILERS.contains(&name)
}

/// A repeat-safe contract certified by `reconciler`.
#[must_use]
pub fn repeat_safe_contract(reconciler: &str) -> ToolEffectContract {
    debug_assert!(
        repeat_safe_reconciler_is_registered(reconciler),
        "a repeat-safe contract must name a registered reconciler"
    );
    ToolEffectContract {
        kind: ToolEffectKind::RepeatSafe,
        reconciler: Some(reconciler.to_owned()),
    }
}

'''

s = s.replace(anchor, block + anchor, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("p1 ok")
