import pathlib
root = pathlib.Path("/root/w5/w1171")

DOC_OLD = """    /// Layer E1 regression guard: the serialized {arr} array must be
    /// byte-identical across two consecutive round-trips of one conversation
    /// — even when the input ToolDef order differs (registration vs curation
    /// order). The array is part of the cached prompt prefix; any byte drift
    /// silently busts prompt caching."""

DOC_NEW = """    /// Layer E1 regression guard: the serialized {arr} array must be
    /// byte-identical across two consecutive round-trips of one conversation,
    /// and — FerroxLabs/wayland#1171 — a tool ADMITTED on a later turn (a
    /// ToolSearch hydration, or MCP curation/cap union growth) must leave the
    /// earlier array as an exact serialized PREFIX. The array is part of the
    /// cached prompt prefix; any byte drift ahead of the growth point silently
    /// busts prompt caching and re-bills the whole prompt.
    ///
    /// The encoder therefore preserves the CALLER's order. It deliberately
    /// does NOT re-sort by name: a name sort is invariant to input order, but
    /// that invariance is bought by turning every append into a mid-array
    /// insert, which is the defect #1171 records."""

TAIL_OLD = """        // A build from a reordered input (e.g. a curation pass shuffled the
        // registry order mid-conversation) must STILL be byte-identical.
        let reordered = {reordered};
        assert_eq!(
            turn1, reordered,
            "reordered input must serialize byte-identically (deterministic name sort)"
        );

        // DUPLICATE names must not reintroduce input-order dependence: the
        // registry does not forbid duplicate registration, and a stable
        // name-only sort keeps input order for equal names. The
        // schema/description tiebreak makes duplicates order-independent too.
        let dup_a = ToolDef {
            name: "Read".into(),
            description: "Read a file (duplicate registration)".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"offset": {"type": "integer"}}}),
            deferred: false,
            server: None,
        };
        let dup_b = ToolDef {
            name: "Read".into(),
            description: "Read a file".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
            server: None,
        };
        {dup_one}
        let other = {dup_other};
        assert_eq!(
            one, other,
            "duplicate names must serialize byte-identically regardless of input order"
        );
    }"""

TAIL_NEW = """        // #1171: the caller's order is preserved verbatim. A name sort would
        // emit Bash first here and, on the engine's append-only tool list,
        // scatter every later admission into the middle of the array.
        let wire = {build}(&defs);
        let names: Vec<&str> = wire.iter().map(|t| {namepath}.as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["Read", "Bash", "SpawnTool"],
            "the encoder must serialize in the caller's order, not name order"
        );

        // #1171: a tool ADMITTED on a later turn (ToolSearch hydration, or
        // curation/cap union growth) is appended at the tail by the engine.
        // The earlier turn's array must survive as an exact serialized prefix.
        let mut grown = defs.clone();
        grown.push(ToolDef {
            name: "Delegate".into(),
            description: "Delegate to a sub-agent".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"task": {"type": "string"}}}),
            deferred: false,
            server: None,
        });
        let after = {build}(&grown);
        assert_eq!(
            serde_json::to_string(&after[..defs.len()]).unwrap(),
            turn1,
            "an appended tool must not rewrite the cached {arr} prefix"
        );
    }"""

SPECS = [
    dict(
        path="crates/wcore-providers/src/anthropic_shared.rs",
        arr="tools[]",
        build="build_tools",
        namepath='t["name"]',
        reordered='serde_json::to_string(&build_tools(&[spawn, bash, read])).unwrap()',
        dup_one='let one = serde_json::to_string(&build_tools(&[dup_a.clone(), dup_b.clone()])).unwrap();',
        dup_other='serde_json::to_string(&build_tools(&[dup_b, dup_a])).unwrap()',
    ),
    dict(
        path="crates/wcore-providers/src/gemini.rs",
        arr="functionDeclarations",
        build="build_function_declarations",
        namepath='t["name"]',
        reordered='serde_json::to_string(&build_function_declarations(&[spawn, bash, read])).unwrap()',
        dup_one='let one = serde_json::to_string(&build_function_declarations(&[\n            dup_a.clone(),\n            dup_b.clone(),\n        ]))\n        .unwrap();',
        dup_other='serde_json::to_string(&build_function_declarations(&[dup_b, dup_a])).unwrap()',
    ),
    dict(
        path="crates/wcore-providers/src/openai_responses.rs",
        arr="tools[]",
        build="build_responses_tools",
        namepath='t["name"]',
        reordered='serde_json::to_string(&build_responses_tools(&[spawn, bash, read])).unwrap()',
        dup_one='let one =\n            serde_json::to_string(&build_responses_tools(&[dup_a.clone(), dup_b.clone()])).unwrap();',
        dup_other='serde_json::to_string(&build_responses_tools(&[dup_b, dup_a])).unwrap()',
    ),
    dict(
        path="crates/wcore-providers/src/openai.rs",
        arr="tools[]",
        build="OpenAIProvider::build_tools",
        namepath='t["function"]["name"]',
        reordered='serde_json::to_string(&OpenAIProvider::build_tools(&[spawn, bash, read])).unwrap()',
        dup_one='let one = serde_json::to_string(&OpenAIProvider::build_tools(&[\n            dup_a.clone(),\n            dup_b.clone(),\n        ]))\n        .unwrap();',
        dup_other='serde_json::to_string(&OpenAIProvider::build_tools(&[dup_b, dup_a])).unwrap()',
    ),
]

for spec in SPECS:
    p = root / spec["path"]
    s = p.read_text()

    doc_old = DOC_OLD.replace("{arr}", spec["arr"])
    assert doc_old in s, f"{spec['path']}: doc comment not found"
    s = s.replace(doc_old, DOC_NEW.replace("{arr}", spec["arr"]), 1)

    tail_old = (TAIL_OLD
                .replace("{reordered}", spec["reordered"])
                .replace("{dup_one}", spec["dup_one"])
                .replace("{dup_other}", spec["dup_other"]))
    assert tail_old in s, f"{spec['path']}: test tail not found"
    tail_new = (TAIL_NEW
                .replace("{build}", spec["build"])
                .replace("{namepath}", spec["namepath"])
                .replace("{arr}", spec["arr"]))
    s = s.replace(tail_old, tail_new, 1)
    p.write_text(s)
    print("patched", spec["path"])
