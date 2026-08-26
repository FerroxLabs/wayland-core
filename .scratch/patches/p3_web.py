import io

# ---------------------------------------------------------------- web_fetch
p = "crates/wcore-tools/src/web_fetch.rs"
s = io.open(p, encoding="utf-8").read()
old = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Remote fetches can affect external rate limits and lack a durable reconciler.
        ToolEffectContract::default()
    }
"""
assert s.count(old) == 1, s.count(old)
new = '''    /// `WebFetch` issues one HTTP GET and never a request body.
    ///
    /// That is a property of the request this process builds, not a hope about
    /// the origin: [`FetchRequest`] carries no method and no body for a
    /// backend to vary, and the only production backend
    /// (`HttpFetchBackend::fetch_inner`) calls `client.get(...)`. GET is the
    /// method HTTP defines as safe — the request does not ask the origin to
    /// change state — so an interrupted fetch left nothing behind for an
    /// operator to have an opinion about.
    ///
    /// What this does NOT claim: an origin is free to violate its own method
    /// contract, and a fetch does consume a remote rate limit. Neither moves
    /// the class. `Read` and `Grep` consume host I/O and are repeat-safe for
    /// exactly the same reason — what is certified is that no state change was
    /// REQUESTED, and the alternative on offer is asking a human about a GET.
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        wcore_types::tool::repeat_safe_contract(wcore_types::tool::READ_ONLY_NETWORK_RECONCILER)
    }
'''
s = s.replace(old, new, 1)

old_test = """    #[test]
    fn effect_contract_remains_opaque() {
        let contract = WebFetchTool::default().effect_contract(&json!({
            "url": "https://example.com"
        }));
        assert_eq!(contract.kind, ToolEffectKind::Opaque);
        assert!(contract.reconciler.is_none());
    }
"""
assert s.count(old_test) == 1, s.count(old_test)
new_test = '''    /// A GET is repeat-safe, and it must say WHICH reconciler certifies it —
    /// recovery acts on the name, not on the kind.
    #[test]
    fn a_fetch_is_certified_repeat_safe_by_the_network_reconciler() {
        let contract = WebFetchTool::default().effect_contract(&json!({
            "url": "https://example.com"
        }));
        assert_eq!(contract.kind, ToolEffectKind::RepeatSafe);
        assert_eq!(
            contract.reconciler.as_deref(),
            Some(wcore_types::tool::READ_ONLY_NETWORK_RECONCILER)
        );
        assert!(
            wcore_types::tool::repeat_safe_reconciler_is_registered(
                contract.reconciler.as_deref().expect("a named reconciler")
            ),
            "a name recovery does not recognise resolves nothing"
        );
    }
'''
s = s.replace(old_test, new_test, 1)
io.open(p, "w", encoding="utf-8").write(s)

# ---------------------------------------------------------------- web_tools
p = "crates/wcore-tools/src/web_tools.rs"
s = io.open(p, encoding="utf-8").read()
old = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // External search, extraction, and crawl backends expose no replay reconciler.
        ToolEffectContract::default()
    }
"""
assert s.count(old) == 1, s.count(old)
new = '''    /// Only `search` is classified, and only because the operation IS a query.
    ///
    /// `extract` and `crawl` stay opaque. No backend in this tree implements
    /// either one — every `WebBackend` impl returns an error for both — so
    /// there is no evidence about what a future one would do, and a crawl at a
    /// real provider creates a server-side job with its own lifecycle and
    /// billing. That is a state change, and guessing it away would be strictly
    /// worse than the operator question it replaced.
    fn effect_contract(&self, input: &Value) -> ToolEffectContract {
        match input
            .get("operation")
            .and_then(Value::as_str)
            .and_then(WebOperation::parse_str)
        {
            Some(WebOperation::Search) => wcore_types::tool::repeat_safe_contract(
                wcore_types::tool::READ_ONLY_NETWORK_RECONCILER,
            ),
            _ => ToolEffectContract::default(),
        }
    }
'''
s = s.replace(old, new, 1)

old_test = """    #[test]
    fn effect_contract_remains_opaque() {
        let contract = WebTool::default().effect_contract(&json!({ "operation": "search" }));
        assert_eq!(contract.kind, ToolEffectKind::Opaque);
        assert!(contract.reconciler.is_none());
    }
"""
assert s.count(old_test) == 1, s.count(old_test)
new_test = '''    /// Search is a query and is certified as one; the two operations that
    /// could create a remote resource keep the recovery they had.
    #[test]
    fn only_search_is_certified_repeat_safe() {
        let contract = WebTool::default().effect_contract(&json!({ "operation": "search" }));
        assert_eq!(contract.kind, ToolEffectKind::RepeatSafe);
        assert_eq!(
            contract.reconciler.as_deref(),
            Some(wcore_types::tool::READ_ONLY_NETWORK_RECONCILER)
        );

        for operation in ["extract", "crawl", "not-an-operation"] {
            let contract = WebTool::default().effect_contract(&json!({ "operation": operation }));
            assert_eq!(
                contract.kind,
                ToolEffectKind::Opaque,
                "`{operation}` may create a remote resource and is not classified"
            );
            assert!(contract.reconciler.is_none());
        }
        let missing = WebTool::default().effect_contract(&json!({}));
        assert_eq!(missing.kind, ToolEffectKind::Opaque);
    }
'''
s = s.replace(old_test, new_test, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("p3 ok")
