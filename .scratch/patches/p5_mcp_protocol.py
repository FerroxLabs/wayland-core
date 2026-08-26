import io
import re
import glob as globmod

# ------------------------------------------------------------------ protocol
p = "crates/wcore-mcp/src/protocol.rs"
s = io.open(p, encoding="utf-8").read()
old = """/// MCP tool definition returned by tools/list
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}
"""
assert s.count(old) == 1, s.count(old)
new = '''/// Behavioural hints a server publishes about ITS OWN tool, from the
/// `annotations` object of a `tools/list` entry.
///
/// Every field is three-valued on purpose. The MCP spec gives each hint a
/// default, but "the server said false" and "the server said nothing" are
/// different pieces of evidence, and only the first is a declaration anyone
/// can act on.
///
/// These are the server's claims about itself, not proofs. Acting on
/// `read_only_hint` is sound here for a reason specific to this product: an
/// MCP server is user-configured, a stdio one executes as the user with the
/// user's full authority, and a server willing to lie about `readOnlyHint`
/// could simply perform the mutation from a tool it never annotated at all.
/// The declaration is the only authority that exists, and the alternative on
/// offer is asking a human after every crash.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct McpToolAnnotations {
    /// The tool does not modify its environment.
    #[serde(default, rename = "readOnlyHint")]
    pub read_only_hint: Option<bool>,
    /// The tool may perform destructive updates. Meaningless when
    /// `read_only_hint` is true — a server asserting both has contradicted
    /// itself, and this crate refuses rather than picking one.
    #[serde(default, rename = "destructiveHint")]
    pub destructive_hint: Option<bool>,
    /// Repeated calls with the same arguments have no additional effect.
    #[serde(default, rename = "idempotentHint")]
    pub idempotent_hint: Option<bool>,
}

/// MCP tool definition returned by tools/list
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// Absent for every server that publishes no `annotations` object, which
    /// is most of them, and which is exactly the opaque recovery MCP tools had
    /// before this field existed.
    #[serde(default)]
    pub annotations: Option<McpToolAnnotations>,
}
'''
s = s.replace(old, new, 1)

old_test = """        let tool: McpToolDef = serde_json::from_str(json_str).unwrap();

        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description.as_deref(), Some("Read a file from disk"));
        assert_eq!(tool.input_schema["type"], "object");
    }
"""
assert s.count(old_test) == 1
new_test = '''        let tool: McpToolDef = serde_json::from_str(json_str).unwrap();

        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description.as_deref(), Some("Read a file from disk"));
        assert_eq!(tool.input_schema["type"], "object");
        assert!(
            tool.annotations.is_none(),
            "a server that publishes no annotations must not read as having declared anything"
        );
    }

    /// The three hints this crate reads, and the distinction that matters:
    /// an ABSENT hint is not a `false` hint.
    #[test]
    fn tool_annotations_deserialize_and_keep_absence_distinct_from_false() {
        let tool: McpToolDef = serde_json::from_str(
            r#"{
                "name": "search",
                "inputSchema": {"type": "object"},
                "annotations": {"readOnlyHint": true, "idempotentHint": false}
            }"#,
        )
        .unwrap();
        let annotations = tool.annotations.expect("annotations were published");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.idempotent_hint, Some(false));
        assert_eq!(
            annotations.destructive_hint, None,
            "a hint the server omitted is not a hint the server denied"
        );
    }

    /// An unknown annotation key is normal — the spec keeps adding them — and
    /// must not fail the whole `tools/list` parse.
    #[test]
    fn an_unknown_annotation_key_does_not_reject_the_tool() {
        let tool: McpToolDef = serde_json::from_str(
            r#"{
                "name": "search",
                "inputSchema": {"type": "object"},
                "annotations": {"title": "Search", "openWorldHint": true}
            }"#,
        )
        .expect("an unknown annotation key must not reject the tool");
        assert_eq!(
            tool.annotations.expect("annotations parsed").read_only_hint,
            None
        );
    }
'''
s = s.replace(old_test, new_test, 1)
io.open(p, "w", encoding="utf-8").write(s)

# --------------------------------------------------- every McpToolDef literal
# Field order is irrelevant in a Rust struct literal, so the new field is
# inserted at the head of every construction site.
LIT = re.compile(r"(?<!struct )McpToolDef\s*\{")
files = []
for pattern in ["crates/*/src/**/*.rs", "crates/*/tests/**/*.rs", "crates/*/src/*.rs", "crates/*/tests/*.rs"]:
    files.extend(globmod.glob(pattern, recursive=True))
touched = []
for f in sorted(set(files)):
    if f.endswith("crates/wcore-mcp/src/protocol.rs"):
        continue
    s = io.open(f, encoding="utf-8").read()
    n = len(LIT.findall(s))
    if n == 0:
        continue
    s = LIT.sub("McpToolDef {\nannotations: None,", s)
    io.open(f, "w", encoding="utf-8").write(s)
    touched.append((f, n))
for f, n in touched:
    print("literal", f, n)
print("p5 ok")
