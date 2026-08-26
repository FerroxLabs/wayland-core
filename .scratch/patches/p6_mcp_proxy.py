import io

p = "crates/wcore-mcp/src/tool_proxy.rs"
s = io.open(p, encoding="utf-8").read()

old = """use super::config::McpServerConfig;
use super::manager::{McpManager, McpToolEffectIdentity};"""
assert s.count(old) == 1
s = s.replace(
    old,
    """use super::config::McpServerConfig;
use super::manager::{McpManager, McpToolEffectIdentity};
use super::protocol::McpToolAnnotations;""",
    1,
)

old = """    /// Whether this tool's schema should be deferred (sent as name-only stub).
    deferred: bool,
}
"""
assert s.count(old) == 1
s = s.replace(
    old,
    """    /// Whether this tool's schema should be deferred (sent as name-only stub).
    deferred: bool,
    /// What the server declared about this tool in `tools/list`. Default —
    /// nothing declared — is what every server that publishes no annotations
    /// gets, and it is the opaque recovery MCP tools have always had.
    annotations: McpToolAnnotations,
}
""",
    1,
)

old = """            manager,
            deferred,
        }
    }
"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """            manager,
            deferred,
            annotations: McpToolAnnotations::default(),
        }
    }

    /// Bind what the server declared about this tool in `tools/list`.
    ///
    /// Separate from [`Self::new`] because a proxy built without it must keep
    /// the opaque contract: an absent declaration is not a declaration of
    /// nothing-happens.
    #[must_use]
    pub fn with_annotations(mut self, annotations: Option<McpToolAnnotations>) -> Self {
        self.annotations = annotations.unwrap_or_default();
        self
    }
""",
    1,
)

old = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // MCP servers expose arbitrary external effects with no host reconciler.
        ToolEffectContract::default()
    }
"""
assert s.count(old) == 1
new = '''    /// Opaque unless the server declared this tool read-only.
    ///
    /// `readOnlyHint: true` is the server stating that its own tool does not
    /// modify its environment. That declaration is the only authority that
    /// exists about a remote effect surface — see [`McpToolAnnotations`] for
    /// why acting on it is sound in this product — and it is what turns an
    /// interrupted call from a question for a human into a receipt.
    ///
    /// Two things are deliberately NOT done here.
    ///
    /// A server that sends `readOnlyHint: true` alongside
    /// `destructiveHint: true` has contradicted itself; this refuses instead
    /// of picking the convenient half.
    ///
    /// `idempotentHint` is not mapped to repeat-safe. The spec's idempotent
    /// tool may mutate — it only promises the SECOND identical call adds
    /// nothing — and the receipt a repeat-safe reconciler writes says the
    /// effect never landed. Recording that for a mutating call would be a
    /// false claim, so an idempotent-but-mutating MCP tool keeps its operator
    /// question. Closing that case needs a resolution the journal does not
    /// have: "safe to re-issue under the same key".
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        if self.annotations.read_only_hint == Some(true)
            && self.annotations.destructive_hint != Some(true)
        {
            return wcore_types::tool::repeat_safe_contract(
                wcore_types::tool::READ_ONLY_MCP_RECONCILER,
            );
        }
        ToolEffectContract::default()
    }
'''
s = s.replace(old, new, 1)

# --------------------------------------------------- both registration sites
old = """        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.clone(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );
"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.clone(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        )
        .with_annotations(tool_def.annotations.clone());
""",
    1,
)

old = """        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        );
"""
assert s.count(old) == 1, s.count(old)
s = s.replace(
    old,
    """        let proxy = McpToolProxy::new(
            display_name,
            original_name.clone(),
            server_name.to_string(),
            tool_def.description.clone().unwrap_or_default(),
            tool_def.input_schema.clone(),
            Arc::clone(manager),
            deferred,
        )
        .with_annotations(tool_def.annotations.clone());
""",
    1,
)

# ------------------------------------------------------------------- tests
old = """    #[test]
    fn proxy_deferred_true_returns_true() {"""
assert s.count(old) == 1
new = '''    /// The proxy's contract is decided by what the SERVER declared, and by
    /// nothing else. The row that matters most is the last one: a server
    /// claiming both read-only and destructive has contradicted itself, and a
    /// receipt written on a contradiction would be worse than the question it
    /// replaced.
    #[test]
    fn only_a_read_only_declaration_certifies_an_mcp_tool() {
        use crate::protocol::McpToolAnnotations;

        let opaque = |proxy: &McpToolProxy| {
            let contract = proxy.effect_contract(&json!({}));
            assert_eq!(contract.kind, wcore_types::tool::ToolEffectKind::Opaque);
            assert!(contract.reconciler.is_none());
        };

        opaque(&make_proxy(false));
        opaque(&make_proxy(false).with_annotations(None));
        opaque(&make_proxy(false).with_annotations(Some(McpToolAnnotations::default())));
        opaque(
            &make_proxy(false).with_annotations(Some(McpToolAnnotations {
                read_only_hint: Some(false),
                ..McpToolAnnotations::default()
            })),
        );
        opaque(
            &make_proxy(false).with_annotations(Some(McpToolAnnotations {
                idempotent_hint: Some(true),
                ..McpToolAnnotations::default()
            })),
        );
        opaque(
            &make_proxy(false).with_annotations(Some(McpToolAnnotations {
                read_only_hint: Some(true),
                destructive_hint: Some(true),
                ..McpToolAnnotations::default()
            })),
        );

        let certified = make_proxy(false)
            .with_annotations(Some(McpToolAnnotations {
                read_only_hint: Some(true),
                ..McpToolAnnotations::default()
            }))
            .effect_contract(&json!({}));
        assert_eq!(
            certified.kind,
            wcore_types::tool::ToolEffectKind::RepeatSafe
        );
        assert_eq!(
            certified.reconciler.as_deref(),
            Some(wcore_types::tool::READ_ONLY_MCP_RECONCILER)
        );
    }

    #[test]
    fn proxy_deferred_true_returns_true() {'''
s = s.replace(old, new, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("p6 ok")
