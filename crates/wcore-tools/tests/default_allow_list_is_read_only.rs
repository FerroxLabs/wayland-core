//! #946 A-10 (re-graded 2026-08-27) — the load-bearing half of the
//! default-allow-list invariant, enforced across the crate boundary.
//!
//! `default_allow_list()`'s own doc comment has always claimed "Anything that
//! writes, executes, or sends a message is NOT in this list", and the test
//! that was supposed to guard that claim asserted four HARDCODED absentee
//! names (`Bash`, `Write`, `Edit`, `send_message`). A negative assertion over
//! a fixed set of names can never fail on the property it claims to guard:
//! the widening it needs to catch is, by definition, a name the test does not
//! mention. It could not observe `doc_extract` — which writes
//! `$TMPDIR/wayland-doc-extract/<hash>.md` — being added to the list, and it
//! could not observe either extractor being handed to a REMOTE principal.
//!
//! This test DERIVES from the list instead. It walks every name in
//! `Config::default().tools.allow_list` (the local auto-approve set) and
//! every name that survives `Config::retain_default_tool_allow_list()` (the
//! set an ACP/A2A network session and a remote chat sender keep), resolves
//! each against the REAL registered tool, and asserts `ToolCategory::Info`
//! unless the name is pinned in `NON_INFO_BY_DESIGN` with its exact category
//! and a reason. Adding `Write` (`Edit`), `Bash` (`Exec`), or any other
//! mutating tool to either list fails here.
//!
//! It lives in `wcore-tools` and not in `wcore-config` because the ground
//! truth is `wcore_tools::Tool::category()`, and the dependency edge runs
//! `wcore-tools -> wcore-config`. `wcore-config` cannot see `ToolCategory` at
//! all, which is why its own tests can only pin the lists as sequences.

use std::sync::Arc;

use wcore_config::config::Config;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;

/// Every built-in tool this crate can construct without a live backend, so
/// the check is a real lookup rather than a curated list of expectations.
/// Deliberately includes mutating tools (`Write`, `Edit`, `Bash`) that must
/// NEVER appear in either allow list — they are the positive control for the
/// category check itself.
fn constructible_builtins() -> Vec<Box<dyn Tool>> {
    use wcore_tools::doc_tool::DocExtractTool;
    use wcore_tools::pdf_tool::PdfTool;
    use wcore_tools::tool_search::ToolSearchTool;
    use wcore_tools::transcription_tools::{
        NullAudioFetcher, NullTranscriptionBackend, TranscribeAudioTool,
    };
    use wcore_tools::vision_tools::{NullImageFetcher, NullVisionBackend, VisionAnalyzeTool};
    use wcore_tools::wayland_introspection::{
        NullWaylandIntrospectionBackend, WaylandStatusTool, WaylandTelemetryQueryTool,
    };
    use wcore_tools::web_fetch::{NullFetchBackend, WebFetchTool};
    use wcore_tools::web_tools::{NullWebBackend, WebTool};

    vec![
        Box::new(wcore_tools::read::ReadTool::new(None)),
        Box::new(wcore_tools::grep::GrepTool),
        Box::new(wcore_tools::glob::GlobTool),
        Box::new(WebTool::new(Arc::new(NullWebBackend))),
        Box::new(WebFetchTool::new(Arc::new(NullFetchBackend))),
        Box::new(VisionAnalyzeTool::new(
            Arc::new(NullVisionBackend),
            Arc::new(NullImageFetcher),
        )),
        Box::new(TranscribeAudioTool::new(
            Arc::new(NullTranscriptionBackend),
            Arc::new(NullAudioFetcher),
        )),
        Box::new(ToolSearchTool::new(Vec::new())),
        Box::new(WaylandStatusTool::new(Arc::new(
            NullWaylandIntrospectionBackend,
        ))),
        Box::new(WaylandTelemetryQueryTool::new(Arc::new(
            NullWaylandIntrospectionBackend,
        ))),
        Box::new(PdfTool::new()),
        Box::new(DocExtractTool::new()),
        // Mutators — the positive control. Present in the lookup, and
        // asserted absent from both allow lists.
        Box::new(wcore_tools::write::WriteTool::new(None)),
        Box::new(wcore_tools::edit::EditTool::new(None)),
        Box::new(wcore_tools::bash::BashTool),
    ]
}

/// Names this crate genuinely cannot resolve, each with the reason. The set
/// is asserted EXACTLY, so a new unresolvable name in either allow list fails
/// the test rather than being silently skipped — an unchecked skip is the
/// same hole as a hardcoded absentee list.
const UNRESOLVABLE_IN_THIS_CRATE: &[&str] = &[
    // `SkillTool` lives in `wcore-agent`, which is ABOVE this crate. Its
    // `category_for` is `Info` for an inline skill but `Exec` for a fork-mode
    // skill, and an inline skill body can carry `!shell:` directives (see
    // `wcore_tools::Tool::read_only_safe`'s doc, which records exactly that
    // escape). It is a pre-existing `GrantScope::Remote` row and is NOT
    // covered by this check.
    "Skill",
];

/// Allow-list entries whose registered category is deliberately NOT `Info`,
/// each pinned with its exact category and the reason. Anything else that is
/// not `Info` fails.
///
/// Found by this test on its first run, which is the point of deriving the
/// check from the list instead of hardcoding absentees.
const NON_INFO_BY_DESIGN: &[(&str, ToolCategory)] = &[
    // `WebFetchTool::category` returns `Mcp` on purpose, pinned by its own
    // unit test `web_fetch::tests::category_is_mcp_not_exec`. `ToolCategory`
    // drives the dispatch TIMEOUT and the protocol classification, not the
    // mutation question — `Mcp` buys a network-shaped timeout while keeping
    // the tool out of the `Exec` class. It neither writes nor spawns a
    // process. Note the inconsistency with `WebTool`, which is `Info` for the
    // same kind of work; that is pre-existing and out of scope here.
    ("WebFetch", ToolCategory::Mcp),
];

fn category_of(name: &str, builtins: &[Box<dyn Tool>]) -> Option<ToolCategory> {
    builtins
        .iter()
        .find(|t| t.name() == name)
        .map(|t| t.category())
}

fn assert_every_name_is_info(list: &[String], label: &str) {
    let builtins = constructible_builtins();
    let mut unresolved: Vec<String> = Vec::new();

    for name in list {
        match category_of(name, &builtins) {
            Some(ToolCategory::Info) => {}
            Some(other) => {
                let pinned = NON_INFO_BY_DESIGN
                    .iter()
                    .any(|(n, c)| n == name && *c == other);
                assert!(
                    pinned,
                    "{label} contains `{name}`, whose registered ToolCategory \
                     is {other:?}, not Info. Allow-list membership SKIPS the \
                     approval gate (ToolConfirmer::requires_confirmation_for \
                     short-circuits on allow_list.contains), so this is an \
                     ungated grant of a tool that may write, execute or send. \
                     If the category is deliberate, pin it in \
                     NON_INFO_BY_DESIGN with the reason."
                );
            }
            None => unresolved.push(name.clone()),
        }
    }

    assert_eq!(
        unresolved, UNRESOLVABLE_IN_THIS_CRATE,
        "{label} gained a name this test cannot resolve against a real tool. \
         A silent skip is exactly the hole the hardcoded four-name test had: \
         either make the tool constructible here, or add it to \
         UNRESOLVABLE_IN_THIS_CRATE with a written reason."
    );
}

#[test]
fn local_default_allow_list_holds_no_writer_or_exec_tool() {
    let local = Config::default().tools.allow_list;

    // Known-positive control: the list is non-empty and the lookup works, so
    // a clean pass is a real pass and not an empty iteration.
    assert!(
        !local.is_empty(),
        "control: Config::default() produced an empty allow list"
    );
    let builtins = constructible_builtins();
    assert_eq!(
        category_of("Read", &builtins),
        Some(ToolCategory::Info),
        "control: the lookup resolves a name that IS in the list"
    );
    assert_eq!(
        category_of("Write", &builtins),
        Some(ToolCategory::Edit),
        "control: the lookup reports a NON-Info category for a writer, so a \
         widening would actually be observed"
    );

    assert_every_name_is_info(&local, "the LOCAL default allow list");

    // The extractors are the point of #946 A-10: present locally.
    for tool in ["pdf_extract", "doc_extract"] {
        assert!(
            local.contains(&tool.to_string()),
            "{tool} must be auto-approved for the local operator; without it \
             .pdf/.docx/.xlsx/.pptx are unreadable on a stock install"
        );
    }
}

#[test]
fn remote_retained_allow_list_holds_no_writer_or_exec_tool_and_no_extractor() {
    let mut config = Config::default();
    config.tools.allow_list = Config::default().tools.allow_list;
    config.retain_default_tool_allow_list();
    let remote = config.tools.allow_list;

    assert!(
        remote.contains(&"Read".to_string()),
        "control: the retain step ran and left the audited read-only tools"
    );

    assert_every_name_is_info(&remote, "the REMOTE retained allow list");

    // #946, the regression the refuter caught: the extractors are LOCAL-ONLY.
    // `doc_extract` writes $TMPDIR/wayland-doc-extract/<hash>.md, and neither
    // tool claims `Tool::read_only_safe` (default-deny; only
    // Read/Grep/Glob/render claim it). Under `ChannelToolPosture::Full` these
    // were previously stripped and, with no TTY, denied.
    for tool in ["pdf_extract", "doc_extract"] {
        assert!(
            !remote.contains(&tool.to_string()),
            "{tool} survived retain_default_tool_allow_list(). That hands it \
             to every ACP/A2A network session (acp_engine::network_session_config) \
             and every remote chat sender (channel_dispatch::remote_channel_config) \
             with no approval gate and no TTY to refuse one."
        );
    }

    // And nothing that mutates, under any spelling this crate can construct.
    for tool in ["Bash", "Write", "Edit"] {
        assert!(
            !remote.contains(&tool.to_string()),
            "{tool} must never be network authority"
        );
    }
}
