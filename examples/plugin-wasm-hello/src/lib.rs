//! Canonical Wayland WASM Tool plugin — returns `Hello, {input}!`.
//!
//! Built with `cargo-component` against the `wayland-tool` world
//! (`wit/tool.wit`). Imports the shared host interface
//! (`wayland:host/host`) — though this hello plugin doesn't actually
//! invoke any host capability — and exports the `tool` interface.
//!
//! Why this exists:
//!   - canonical example for `templates/plugin-wasm/` consumers,
//!   - source-of-truth for regenerating
//!     `crates/wcore-plugin-wasm/tests/fixtures/hello.wasm`.
//!
//! Regenerate the fixture from this crate:
//!
//! ```bash
//! cargo install cargo-component             # one-time
//! cd examples/plugin-wasm-hello
//! cargo component build --release
//! cp target/wasm32-wasip1/release/plugin_wasm_hello.wasm \
//!    ../../crates/wcore-plugin-wasm/tests/fixtures/hello.wasm
//! ```

#[allow(warnings)]
mod bindings;

use bindings::exports::wayland::host::tool::{
    Guest as ToolGuest, GuestTool, Request, Response, ToolCategory, ToolMetadata,
};

struct Tool;

impl ToolGuest for Tool {
    type Tool = Tool;
}

impl GuestTool for Tool {
    fn execute(req: Request) -> Result<Response, String> {
        // The "input" field is a JSON-encoded value — for this example we
        // accept either a raw JSON string ("\"world\"") OR a bare string.
        let trimmed = req.input.trim();
        let name = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };
        Ok(Response {
            stdout: format!("Hello, {name}!"),
            structured: None,
            is_error: false,
        })
    }

    fn schema() -> String {
        r#"{"type":"string","description":"name to greet"}"#.into()
    }

    fn description() -> String {
        "Greets the input name. Canonical Wayland WASM Tool example.".into()
    }

    fn metadata() -> ToolMetadata {
        ToolMetadata {
            name: "hello".into(),
            description: "Greets the input name.".into(),
            input_schema: r#"{"type":"string"}"#.into(),
            category: ToolCategory::Utility,
            is_deferred: false,
            max_result_size: 4096,
            caps_version: 1,
        }
    }
}

bindings::export!(Tool with_types_in bindings);
