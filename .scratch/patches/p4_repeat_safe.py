import io

OLD = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        ToolEffectContract {
            kind: ToolEffectKind::RepeatSafe,
            reconciler: None,
        }
    }
"""
NEW = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        wcore_types::tool::repeat_safe_contract(wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER)
    }
"""

for name in ["read.rs", "grep.rs", "glob.rs"]:
    p = "crates/wcore-tools/src/" + name
    s = io.open(p, encoding="utf-8").read()
    assert s.count(OLD) == 1, (name, s.count(OLD))
    s = s.replace(OLD, NEW, 1)
    io.open(p, "w", encoding="utf-8").write(s)

# ------------------------------------------------------------------- bash test
p = "crates/wcore-tools/src/bash/tests.rs"
s = io.open(p, encoding="utf-8").read()
old = """#[test]
fn effect_contract_remains_opaque() {
    let contract = BashTool.effect_contract(&json!({ "command": "true" }));
    assert_eq!(contract.kind, ToolEffectKind::Opaque);
    assert!(contract.reconciler.is_none());
}
"""
assert s.count(old) == 1, s.count(old)
new = '''/// The shell classification is per-INVOCATION, which is the whole point: the
/// tool is opaque, a provably read-only command through it is not.
#[test]
fn a_provably_read_only_command_is_certified_and_everything_else_stays_opaque() {
    let certified = BashTool.effect_contract(&json!({ "command": "ls -la" }));
    assert_eq!(certified.kind, ToolEffectKind::RepeatSafe);
    assert_eq!(
        certified.reconciler.as_deref(),
        Some(wcore_types::tool::READ_ONLY_SHELL_RECONCILER)
    );

    for command in ["rm -rf /", "cat a > b", "git status", "ls; rm -rf /"] {
        let contract = BashTool.effect_contract(&json!({ "command": command }));
        assert_eq!(
            contract.kind,
            ToolEffectKind::Opaque,
            "`{command}` must keep the opaque recovery it had"
        );
        assert!(contract.reconciler.is_none());
    }

    let missing = BashTool.effect_contract(&json!({}));
    assert_eq!(missing.kind, ToolEffectKind::Opaque);
}
'''
s = s.replace(old, new, 1)
io.open(p, "w", encoding="utf-8").write(s)

# ------------------------------------------------------------- Tool trait doc
p = "crates/wcore-tools/src/lib.rs"
s = io.open(p, encoding="utf-8").read()
old = """    /// The reachable exceptions are exactly two, and both earn it:
    ///
    /// * `Read`, `Grep`, `Glob` are [`wcore_types::tool::ToolEffectKind::RepeatSafe`] because
    ///   they mutate nothing.
"""
assert s.count(old) == 1, s.count(old)
new = """    /// The reachable exceptions all earn it, and each names the reconciler
    /// that certifies it — recovery acts on the NAME, so an unregistered one
    /// resolves nothing (see
    /// [`wcore_types::tool::repeat_safe_reconciler_is_registered`]):
    ///
    /// * `Read`, `Grep`, `Glob` are [`wcore_types::tool::ToolEffectKind::RepeatSafe`] because
    ///   they mutate nothing.
    /// * A `Bash` call whose command a static classifier proves cannot mutate
    ///   anything is repeat-safe for that invocation only; every other command
    ///   through the same tool stays opaque.
    /// * `WebFetch` and the web tool's `search` operation are repeat-safe
    ///   because the request they build cannot ask for a state change. The
    ///   web tool's `extract` and `crawl` are not.
    /// * An MCP tool its own server declared `readOnlyHint: true` for is
    ///   repeat-safe on that declaration.
"""
s = s.replace(old, new, 1)

old2 = """    /// * `Bash` and `Script` — a shell command mutates arbitrary host state.
    /// * `WebFetch` and the web search/extract/crawl tools — a remote service's
    ///   state and rate limits are not re-readable.
"""
assert s.count(old2) == 1
new2 = """    /// * `Script`, and any `Bash` command outside the read-only classifier —
    ///   a shell command mutates arbitrary host state.
    /// * The web tool's `extract` and `crawl` — a crawl creates a remote job.
"""
s = s.replace(old2, new2, 1)

old3 = """    /// * MCP proxies, plugin closures and browser actions — untrusted or
    ///   remote effect surfaces the host cannot photograph.
"""
assert s.count(old3) == 1
new3 = """    /// * MCP proxies with no `readOnlyHint`, plugin closures and browser
    ///   actions — remote effect surfaces the host cannot photograph.
"""
s = s.replace(old3, new3, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("p4 ok")
