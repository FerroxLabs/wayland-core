//! Every channel config printed in `docs/channels.md` must actually load.
//!
//! # Why this file exists
//!
//! A live UAT of the shipped 0.12.25 binary tried to configure a Slack channel
//! by copying `docs/channels.md`'s own "Recommended deployment baseline"
//! verbatim into `~/.wayland/channels/slack.toml`. It took **four**
//! edit-and-rerun round trips to get a channel to load:
//!
//! ```text
//! missing field `name`
//! missing field `platform`
//! config parse: missing field `workspace_name`
//! ```
//!
//! and **none** of the four fields it had to discover (`name`, `platform`,
//! `workspace_name`, the `credential_handle_*` keys) appeared anywhere in the
//! document. The published block was an `[inbound]` fragment; the document
//! never said so, and nothing anywhere checked.
//!
//! That is the ordinary fate of a configuration example: it is prose, it is
//! never executed, and it rots the first time a required field is added. The
//! repair is to stop treating it as prose. This test lifts the TOML straight
//! out of the shipped document and runs it through **the same two layers the
//! product does**:
//!
//! 1. [`wcore_channels::parse_channel_config`] — the loader `gateway run`,
//!    `channel list` and `channel probe` all reach; and
//! 2. [`channel_factory_for`] — the real per-platform constructor, which is
//!    what parses `[options]` into `SlackConfig` / `DiscordConfig` /
//!    `MatrixConfig` and is therefore the layer that produced round trip 4.
//!
//! Layer 2 is the load-bearing one. A block can satisfy `ChannelConfig`
//! perfectly and still be useless, because `[options]` is a free-form
//! `toml::Table` at that level — which is exactly why round trip 3 looked like
//! success and round trip 4 was not.
//!
//! Construction is offline. `SlackChannel::new` and friends take a
//! `CredentialsStore` and do not touch it, or the network, until `start()`;
//! this test never calls `start()` and passes an in-memory store, so it makes
//! no network call and reads no real credential.
//!
//! # Which blocks are checked
//!
//! A fenced ```` ```toml ```` block opts in by making its first line the
//! destination path comment the document already uses:
//!
//! ```text
//! # ~/.wayland/channels/slack.toml
//! ```
//!
//! That marker does double duty: it declares "this is a complete file" and it
//! supplies the **file stem**, so the test also enforces the `name`-must-match-
//! the-stem rule that is itself a load error. Fragments (the `[inbound]`
//! snippets that illustrate one setting) carry no such comment and are
//! deliberately not checked — they are not claimed to be complete.
//!
//! # It runs in BOTH directions
//!
//! A gate that cannot fail proves nothing, and a gate that cannot pass proves
//! less. [`problems_with`] is a pure function of a document string, so the
//! tests below feed it doctored documents:
//!
//! - [`the_checker_rejects_a_block_missing_a_channelconfig_field`] and
//!   [`the_checker_rejects_a_block_missing_a_platform_option`] reconstruct the
//!   two failures the UAT actually hit and assert each is reported — the gate
//!   can fail, on the real defect;
//! - [`the_checker_rejects_a_name_that_does_not_match_the_stem`] covers the
//!   third load error;
//! - [`the_checker_rejects_a_document_with_no_complete_configs_at_all`] stops
//!   the gate from going quiet if someone deletes the section, which is the
//!   way a doc test most often dies;
//! - [`the_checker_passes_a_hand_written_correct_document`] constructs a
//!   *different* document that is correct, proving the pass state is reachable
//!   from something other than the file already in the tree.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wcore_channels_registry::channel_factory_for;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

const CHANNELS_DOC: &str = include_str!("../../../docs/channels.md");

/// Opening line that marks a fenced block as a complete channel config and
/// names the file it belongs at.
const DEST_PREFIX: &str = "# ~/.wayland/channels/";

/// The document must keep at least this many complete configs. Three is the
/// MVP channel set (Slack / Discord / Matrix) documented in §1 and §3, plus
/// the deployment baseline — the count is a floor, not an equality, so adding
/// a platform does not redden this.
const MIN_COMPLETE_CONFIGS: usize = 4;

/// In-memory store so no test touches the real keyring or writes a plaintext
/// file. Never read: construction does not resolve handles.
#[derive(Default)]
struct MemStore(Mutex<HashMap<String, String>>);

impl CredentialsStore for MemStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// One complete config lifted out of the document: the stem its path comment
/// declares, and the TOML body.
#[derive(Debug)]
struct DocConfig {
    stem: String,
    body: String,
}

/// Pull every fenced `toml` block whose first line is a
/// `# ~/.wayland/channels/<stem>.toml` comment.
fn complete_configs(doc: &str) -> Vec<DocConfig> {
    let mut out = Vec::new();
    let mut lines = doc.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_end() != "```toml" {
            continue;
        }
        let mut body = String::new();
        let mut stem: Option<String> = None;
        let mut first = true;
        for inner in lines.by_ref() {
            if inner.trim_end() == "```" {
                break;
            }
            if first {
                first = false;
                if let Some(rest) = inner.trim().strip_prefix(DEST_PREFIX) {
                    stem = rest.strip_suffix(".toml").map(str::to_string);
                }
                // The marker comment is a TOML comment, so it is kept in the
                // body: what the test parses is byte-for-byte what a reader
                // copies.
            }
            body.push_str(inner);
            body.push('\n');
        }
        if let Some(stem) = stem {
            out.push(DocConfig { stem, body });
        }
    }
    out
}

/// Every way `doc`'s complete configs disagree with the real schema. Empty
/// means the document is honest. Pure — no filesystem, no network.
fn problems_with(doc: &str) -> Vec<String> {
    let configs = complete_configs(doc);
    let mut problems = Vec::new();

    if configs.len() < MIN_COMPLETE_CONFIGS {
        problems.push(format!(
            "docs/channels.md carries {} complete channel config(s) (a fenced \
             ```toml block whose first line is `{DEST_PREFIX}<stem>.toml`), \
             expected at least {MIN_COMPLETE_CONFIGS}. If the onboarding \
             section was removed, this test is no longer checking anything.",
            configs.len()
        ));
    }

    for DocConfig { stem, body } in &configs {
        let file = format!("{stem}.toml");

        // Layer 1: the loader every product surface goes through.
        let cfg = match wcore_channels::parse_channel_config(&file, body) {
            Ok(cfg) => cfg,
            Err(e) => {
                problems.push(format!(
                    "the documented config for {file} does not load: {e}"
                ));
                continue;
            }
        };

        // The stem rule is itself a load error, and the document is the only
        // place that states the intended filename.
        if cfg.name != *stem {
            problems.push(format!(
                "the documented config for {file} sets name = {:?}, which does \
                 not match the file stem {stem:?} its own path comment declares \
                 — this is a load error",
                cfg.name
            ));
        }

        // Layer 2: the adapter's own required options. This is the layer that
        // produced the UAT's fourth round trip.
        let Some(factory) = channel_factory_for(&cfg.platform) else {
            // imessage is macOS-only, so an absent factory is only a problem
            // when the platform is one this host should know.
            if cfg.platform != "imessage" {
                problems.push(format!(
                    "the documented config for {file} names platform {:?}, which \
                     no factory knows — the channel cannot load",
                    cfg.platform
                ));
            }
            continue;
        };
        let creds: Arc<dyn CredentialsStore> = Arc::new(MemStore::default());
        if let Err(e) = factory(cfg.name.clone(), &cfg.options, creds) {
            problems.push(format!(
                "the documented config for {file} parses as a ChannelConfig but \
                 the {} adapter rejects its [options]: {e}",
                cfg.platform
            ));
        }
    }

    problems
}

// ---------------------------------------------------------------------------
// The assertion against the real document
// ---------------------------------------------------------------------------

#[test]
fn every_documented_channel_config_loads_and_constructs() {
    let problems = problems_with(CHANNELS_DOC);
    assert!(
        problems.is_empty(),
        "docs/channels.md publishes {} config(s) that would not work:\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    );
}

/// The extractor must find the configs it claims to. Asserted separately from
/// the check above so "the document is honest" and "the test read the
/// document" cannot be confused — a broken extractor finds nothing and an
/// empty problem list looks like a pass.
#[test]
fn the_extractor_finds_the_documented_configs() {
    let found = complete_configs(CHANNELS_DOC);
    let stems: Vec<&str> = found.iter().map(|c| c.stem.as_str()).collect();
    assert!(
        found.len() >= MIN_COMPLETE_CONFIGS,
        "expected at least {MIN_COMPLETE_CONFIGS} complete configs, found {}: {stems:?}",
        found.len()
    );
    for want in ["slack", "discord", "matrix"] {
        assert!(
            stems.contains(&want),
            "docs/channels.md documents no complete config for {want:?}; found {stems:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Both directions: the checker must fail on the real defects, and must pass
// on a correct document it has never seen.
// ---------------------------------------------------------------------------

/// A minimal correct document, used as the base for the doctored ones so each
/// negative test differs from a passing input by exactly the defect it names.
fn correct_doc() -> String {
    format!(
        "intro\n\n\
         ```toml\n\
         {DEST_PREFIX}slack.toml\n\
         name = \"slack\"\n\
         platform = \"slack\"\n\n\
         [options]\n\
         workspace_name = \"acme\"\n\
         credential_handle_bot_token = \"slack.acme.bot_token\"\n\
         credential_handle_signing_secret = \"slack.acme.signing_secret\"\n\
         ```\n\n\
         ```toml\n\
         {DEST_PREFIX}discord.toml\n\
         name = \"discord\"\n\
         platform = \"discord\"\n\n\
         [options]\n\
         credential_handle = \"discord.acme.bot_token\"\n\
         ```\n\n\
         ```toml\n\
         {DEST_PREFIX}matrix.toml\n\
         name = \"matrix\"\n\
         platform = \"matrix\"\n\n\
         [options]\n\
         homeserver_url = \"https://matrix.org\"\n\
         user_id = \"@bot:matrix.org\"\n\
         credential_handle_access_token = \"matrix.acme.access_token\"\n\
         ```\n\n\
         ```toml\n\
         {DEST_PREFIX}tg.toml\n\
         name = \"tg\"\n\
         platform = \"telegram\"\n\n\
         [options]\n\
         credential_handle = \"telegram.acme.bot_token\"\n\
         ```\n"
    )
}

#[test]
fn the_checker_passes_a_hand_written_correct_document() {
    let problems = problems_with(&correct_doc());
    assert!(
        problems.is_empty(),
        "a correct document must produce no problems, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// Round trips 1 and 2 of the UAT: the published block was an `[inbound]`
/// fragment with no `name` and no `platform`.
#[test]
fn the_checker_rejects_a_block_missing_a_channelconfig_field() {
    let doc = correct_doc().replace("name = \"slack\"\n", "");
    let problems = problems_with(&doc);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("slack.toml") && p.contains("name")),
        "dropping `name` from the documented Slack config must be reported, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// Round trip 4, and the reason layer 2 exists: the block satisfies
/// `ChannelConfig` and the *adapter* still rejects it.
#[test]
fn the_checker_rejects_a_block_missing_a_platform_option() {
    let doc = correct_doc().replace("workspace_name = \"acme\"\n", "");
    let problems = problems_with(&doc);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("slack.toml") && p.contains("workspace_name")),
        "dropping `workspace_name` must be reported by the adapter layer, got:\n  - {}",
        problems.join("\n  - ")
    );
    // And prove it got there through layer 2 rather than layer 1 — otherwise
    // this test would pass even if the factory were never called.
    assert!(
        problems
            .iter()
            .any(|p| p.contains("parses as a ChannelConfig but")),
        "the failure must come from the adapter, not the loader, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// Round trip 3's sibling: `name` present but disagreeing with the filename
/// the document itself tells the reader to use.
#[test]
fn the_checker_rejects_a_name_that_does_not_match_the_stem() {
    let doc = correct_doc().replace("name = \"discord\"", "name = \"disco\"");
    let problems = problems_with(&doc);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("discord.toml") && p.contains("file stem")),
        "a name/stem mismatch must be reported, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// The removed `[secrets]` table must not be publishable either — this is the
/// syntax that shipped in a doc comment for a whole release cycle while
/// resolving to nothing.
#[test]
fn the_checker_rejects_a_documented_legacy_secrets_table() {
    let doc = correct_doc().replace(
        "credential_handle = \"discord.acme.bot_token\"\n",
        "credential_handle = \"discord.acme.bot_token\"\n\n[secrets]\nbot_token = \"keychain:d:t\"\n",
    );
    let problems = problems_with(&doc);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("discord.toml") && p.contains("[secrets]")),
        "a documented [secrets] table must be reported, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// The way a doc test usually dies: the section it guards is deleted and the
/// test goes quiet instead of red.
#[test]
fn the_checker_rejects_a_document_with_no_complete_configs_at_all() {
    let problems = problems_with("intro\n\n```toml\n[inbound]\ndm = \"open\"\n```\n");
    assert!(
        problems.iter().any(|p| p.contains("expected at least")),
        "a document with no complete configs must be reported, got:\n  - {}",
        problems.join("\n  - ")
    );
}

/// The extractor must ignore fragments. Without this, adding the marker
/// comment to an `[inbound]` snippet would silently start failing the suite
/// for the wrong reason — and, worse, a fragment counted toward
/// `MIN_COMPLETE_CONFIGS` would let the real section be deleted unnoticed.
#[test]
fn the_extractor_ignores_fenced_blocks_without_the_path_marker() {
    let doc = "```toml\n[inbound]\ndm = \"open\"\n```\n\
               ```console\n$ wayland-core channel list\n```\n";
    assert!(
        complete_configs(doc).is_empty(),
        "only blocks carrying the destination-path comment are complete configs"
    );
}
