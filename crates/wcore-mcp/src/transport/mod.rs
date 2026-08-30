pub mod sse;
pub mod stdio;
pub mod stdio_readiness;
pub mod streamable_http;

use async_trait::async_trait;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// The MCP notification a server sends when its tool list changes.
pub(crate) const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

/// Is this inbound frame the `tools/list_changed` notification?
///
/// Parsed as a generic JSON value rather than a typed notification struct:
/// the only field that matters is `method`, and a malformed or unrelated
/// notification must be a plain `false`, never an error that kills the reader.
///
/// FerroxLabs/wayland#1175 — lives here rather than in `stdio` because all
/// THREE transports need it. It was stdio-private while `take_tools_changed`
/// was stdio-only, which is exactly the defect: a server attached over SSE or
/// Streamable HTTP had its announcement discarded for the life of the session.
pub(crate) fn notified_tools_changed(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(|method| method.as_str())
                .map(|method| method == TOOLS_LIST_CHANGED)
        })
        .unwrap_or(false)
}

/// Transport abstraction for MCP communication
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive the response
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;

    /// Send a notification (no response expected)
    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError>;

    /// Close the transport
    async fn close(&self) -> Result<(), McpError>;

    /// Take-and-clear the "the server told us its tool list changed" flag.
    ///
    /// MCP servers may register or drop tools mid-session and announce it
    /// with `notifications/tools/list_changed` (declared via the
    /// `tools.listChanged` capability). That notification carries no `id`,
    /// so it is not a response to any request and can only be observed by
    /// whatever owns the inbound stream — the transport. This is how the
    /// manager learns to re-issue `tools/list`.
    ///
    /// Returns `true` at most once per notification burst: the flag is
    /// cleared by reading it, so a poller cannot re-refresh forever off one
    /// signal. Transports that do not observe server-initiated
    /// notifications always return `false`.
    fn take_tools_changed(&self) -> bool {
        false
    }

    /// Whether the transport is still believed to be usable.
    ///
    /// Audit C4/C7: a server that dies (child process exits) or that the
    /// engine deliberately tears down on a cancelled wedged call should
    /// stop being treated as live, so the manager can prune it and stop
    /// advertising its tools. Transports without a backing process
    /// (HTTP-style) are always considered live — each request is
    /// independent and self-bounded by its own timeout.
    fn is_alive(&self) -> bool {
        true
    }
}

/// Errors from MCP transport and protocol
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Tool not found: {server}/{tool}")]
    ToolNotFound { server: String, tool: String },

    #[error("Initialization failed: {0}")]
    InitFailed(String),

    #[error("MCP connect timed out after {after:?}{cleanup}")]
    ConnectTimedOut {
        after: std::time::Duration,
        cleanup: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// FerroxLabs/wayland#1137 — the pre-spawn malware gate refused the
    /// launch. Its own variant, not an `InitFailed`, so a host can render a
    /// supply-chain refusal differently from a server that merely failed to
    /// start: one is a security decision the user should see, the other is an
    /// operational error they can retry.
    #[error("MCP server launch refused: {0}")]
    MalwareBlocked(String),
}

#[cfg(test)]
mod tests {
    /// FerroxLabs/wayland#1175 — `take_tools_changed` has a trait DEFAULT of
    /// `false`, and for two of the three transports nobody overrode it. That
    /// default is silent: an SSE or Streamable HTTP server announced a tool and
    /// `McpManager::refresh_signalled_tools` skipped it for the life of the
    /// session, with no warning anywhere.
    ///
    /// A behavioural test per transport cannot catch the FOURTH transport
    /// somebody adds next year, so this grades the class: every
    /// `impl McpTransport` in this module tree must say what it does about
    /// server-initiated tool-list changes.
    #[test]
    fn every_transport_decides_take_tools_changed_for_itself() {
        // This lint used to `include_str!` a hardcoded list of the three
        // transports that existed when #1175 was fixed. That is a regression
        // guard, not a class guard: a FOURTH transport would simply not be in
        // the list, would inherit the `false` default, and this test would
        // stay green while its tools/list_changed was discarded for the life
        // of the session — the exact defect, in a new file. So the set is
        // DISCOVERED from the tree instead of written down.
        //
        // A production transport impl is an `impl ... McpTransport for ...`
        // header starting at column zero in a file under some crate's `src/`.
        // The indentation is the discriminator: every mock in the tree lives
        // inside an inline `#[cfg(test)] mod tests`, so its impl is indented,
        // and a mock is legitimately allowed to inherit the default — it
        // observes no server-initiated stream to report on. Integration-test
        // mocks are at column zero but live under a `tests/` directory, which
        // is excluded.
        //
        // What this walk does and does not see is stated exactly in the
        // #1175 ledger note; the escapes it deliberately still has are
        // recorded there rather than implied away here.
        let crates_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/<crate> has a parent")
            .to_path_buf();

        let mut stack = vec![crates_dir];
        let mut production: Vec<(String, Vec<String>)> = Vec::new();
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).expect("read crates tree");
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    // `target/` is build output, `tests/` is integration-test
                    // mocks, and neither ships a transport an operator uses.
                    if name != "target" && name != "tests" && !name.starts_with('.') {
                        stack.push(path);
                    }
                    continue;
                }
                if !name.ends_with(".rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read rust source");
                let blocks = transport_impl_blocks(&source);
                if !blocks.is_empty() {
                    production.push((path.display().to_string(), blocks));
                }
            }
        }

        // POSITIVE CONTROL. The walk is the whole test; if it silently found
        // nothing (wrong root, renamed trait, changed `impl` spelling) every
        // assertion below would vacuously pass. Pin the three transports that
        // are known to exist — as a control on the DISCOVERY, not as the set
        // being graded, which is whatever the walk actually returns.
        for known in ["stdio.rs", "sse.rs", "streamable_http.rs"] {
            assert!(
                production.iter().any(|(path, _)| path.ends_with(known)),
                "the transport walk did not find {known} — discovery is broken \
                 and this lint is grading an empty set. Found: {:?}",
                production.iter().map(|(p, _)| p).collect::<Vec<_>>()
            );
        }

        for (path, blocks) in &production {
            for block in blocks {
                assert!(
                    declares_take_tools_changed(block),
                    "{path} implements McpTransport but inherits the `false` \
                     default for take_tools_changed, so a tools/list_changed \
                     it receives is discarded and the tool stays uncallable \
                     for the session (FerroxLabs/wayland#1175)"
                );
            }
        }
    }

    /// Every column-zero `impl … McpTransport for …` BODY in `source`.
    ///
    /// Returns the body text, not the whole file, because the override has to
    /// be graded inside the impl block: a file-wide `contains` is satisfied by
    /// any mention of the method name anywhere in the file — including a
    /// comment saying the method is not needed here.
    fn transport_impl_blocks(source: &str) -> Vec<String> {
        let lines: Vec<&str> = source.lines().collect();
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if !lines[i].starts_with("impl") {
                i += 1;
                continue;
            }
            // The header may wrap: rustfmt breaks a long
            // `impl<T: …> Trait for Type` across lines. Join from `impl` up to
            // and including the line that opens the body.
            let mut header = String::new();
            let mut j = i;
            while j < lines.len() {
                header.push_str(lines[j].trim_end());
                header.push(' ');
                if lines[j].contains('{') || lines[j].trim_end().ends_with(';') {
                    break;
                }
                j += 1;
            }
            if j >= lines.len() || !header_names_mcp_transport(&header) {
                i += 1;
                continue;
            }
            // Body: brace-count from the opening line. Line comments are
            // stripped first so a `//` mentioning a brace cannot skew it. An
            // unbalanced brace inside a string literal would end the block
            // early and make this lint FAIL loudly on a real transport, which
            // is the safe direction for a guard.
            let mut depth = 0i32;
            let mut body = String::new();
            let mut k = j;
            while k < lines.len() {
                let code = match lines[k].find("//") {
                    Some(at) => &lines[k][..at],
                    None => lines[k],
                };
                depth += code.matches('{').count() as i32;
                depth -= code.matches('}').count() as i32;
                body.push_str(lines[k]);
                body.push('\n');
                if depth <= 0 && code.contains('}') {
                    break;
                }
                k += 1;
            }
            blocks.push(body);
            i = k + 1;
        }
        blocks
    }

    /// True when `header` is an `impl` of `McpTransport`, in any spelling
    /// rustc accepts: bare, generic (`impl<T: Send> McpTransport for W<T>`) or
    /// path-qualified (`impl crate::transport::McpTransport for W`).
    fn header_names_mcp_transport(header: &str) -> bool {
        let Some(rest) = header.strip_prefix("impl") else {
            return false;
        };
        // `impl` has to be its own token — `implicit_thing` is not an impl.
        let rest = match rest.chars().next() {
            Some('<') => {
                // Skip a balanced generic parameter list. `->` inside an `Fn`
                // bound is not a closing angle bracket.
                let mut depth = 0usize;
                let mut prev = ' ';
                let mut after = None;
                for (at, ch) in rest.char_indices() {
                    match ch {
                        '<' => depth += 1,
                        '>' if prev != '-' => {
                            depth -= 1;
                            if depth == 0 {
                                after = Some(at + 1);
                                break;
                            }
                        }
                        _ => {}
                    }
                    prev = ch;
                }
                match after {
                    Some(at) => &rest[at..],
                    None => return false,
                }
            }
            Some(ch) if ch.is_whitespace() => rest,
            _ => return false,
        };
        // Everything before ` for ` is the trait path.
        let Some((trait_path, _)) = rest.trim_start().split_once(" for ") else {
            return false;
        };
        // Drop `crate::`, `super::`, `wcore_mcp::transport::` and friends.
        trait_path.trim().rsplit("::").next().map(str::trim) == Some("McpTransport")
    }

    /// True when `block` DECLARES `take_tools_changed`, as opposed to merely
    /// mentioning it. A comment is not a declaration; this is the same
    /// mutation-hits-a-comment trap, on a guard's positive side.
    fn declares_take_tools_changed(block: &str) -> bool {
        block.lines().any(|line| {
            let mut rest = line.trim_start();
            for modifier in ["pub(crate)", "pub", "async", "unsafe", "extern"] {
                if let Some(stripped) = rest.strip_prefix(modifier) {
                    if stripped.starts_with(char::is_whitespace) {
                        rest = stripped.trim_start();
                    }
                }
            }
            rest.starts_with("fn take_tools_changed")
        })
    }

    /// The matcher is itself the thing that can silently stop matching, so it
    /// is graded directly: each spelling below was a MEASURED escape of the
    /// previous `line.starts_with("impl McpTransport for")` prefix test.
    #[test]
    fn the_impl_matcher_sees_every_spelling_of_the_header() {
        for accepted in [
            "impl McpTransport for StdioTransport { ",
            "impl crate::transport::McpTransport for WsTransport { ",
            "impl<T: Send> McpTransport for W<T> { ",
            "impl<F: Fn() -> u8> McpTransport for W<F> { ",
            "impl wcore_mcp::transport::McpTransport for W { ",
        ] {
            assert!(
                header_names_mcp_transport(accepted),
                "the matcher misses a real header spelling: {accepted:?}"
            );
        }
        // NEGATIVE CONTROL: a matcher that returns true for everything would
        // pass the block above and grade nothing.
        for rejected in [
            "impl Transport for W { ",
            "impl McpTransportish for W { ",
            "implMcpTransport for W { ",
            "impl<T McpTransport for W { ",
            "impl McpTransport { ",
        ] {
            assert!(
                !header_names_mcp_transport(rejected),
                "the matcher accepts something that is not an McpTransport \
                 impl header: {rejected:?}"
            );
        }
    }

    /// A comment naming the method must not satisfy the guard.
    #[test]
    fn a_comment_is_not_a_take_tools_changed_declaration() {
        assert!(
            !declares_take_tools_changed(
                "impl McpTransport for W {\n    // we do not need fn take_tools_changed here\n}\n"
            ),
            "a comment mentioning the method satisfies the guard"
        );
        assert!(
            !declares_take_tools_changed(
                "impl McpTransport for W {\n    /// fn take_tools_changed is inherited\n}\n"
            ),
            "a doc comment mentioning the method satisfies the guard"
        );
        // POSITIVE CONTROL: the real declaration still counts, or the check
        // above is satisfied by rejecting everything.
        assert!(
            declares_take_tools_changed(
                "impl McpTransport for W {\n    fn take_tools_changed(&self) -> bool { false }\n}\n"
            ),
            "a real declaration is not recognised"
        );
        assert!(
            declares_take_tools_changed(
                "impl McpTransport for W {\n    pub async fn take_tools_changed(&self) -> bool { false }\n}\n"
            ),
            "a real `pub async` declaration is not recognised"
        );
    }

    /// The block splitter must scope the search to the impl body, or a file
    /// with a compliant transport and a non-compliant one grades green.
    #[test]
    fn the_override_is_graded_inside_its_own_impl_block() {
        let source = "impl McpTransport for Good {\n    fn take_tools_changed(&self) -> bool {\n        true\n    }\n}\n\nimpl McpTransport for Bad {\n    fn close(&self) {}\n}\n";
        let blocks = transport_impl_blocks(source);
        assert_eq!(blocks.len(), 2, "expected two impl blocks: {blocks:?}");
        assert!(declares_take_tools_changed(&blocks[0]));
        assert!(
            !declares_take_tools_changed(&blocks[1]),
            "the second impl block borrowed the first one's override"
        );
        // NEGATIVE CONTROL on discovery: an indented (mock) impl is not a
        // production transport and must not be collected at all.
        assert!(
            transport_impl_blocks("    impl McpTransport for Mock {\n    }\n").is_empty(),
            "an indented mock impl was collected as a production transport"
        );
    }
}
