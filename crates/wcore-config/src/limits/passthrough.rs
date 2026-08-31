//! #1176 -- the provider-native passthrough ids the `if`-chain families must
//! resolve, and the vendor figures they must resolve to.
//!
//! WHY THIS TABLE EXISTS
//! ---------------------
//! Two guards protect `model_output_ceiling`, and they shared one blind spot:
//!
//!   * `scripts/check-model-limits-freshness.py` graded only the ordered
//!     `CATALOGUE_CEILINGS` table and said so in its own output -- the older
//!     `if`-chain families (Claude, GPT-4.x/5.x, Grok, Gemini, DeepSeek,
//!     MiniMax) "cannot be evaluated by a text parser; verify those by hand".
//!   * `every_routed_catalog_model_has_a_known_window` walks
//!     `wcore_types::model_aliases::models_for_provider()` -- ROUTED ALIASES
//!     ONLY. None of `opus-5`, `gemini-flash-latest` or `gpt-4o-2024-` is in
//!     that catalogue.
//!
//! An id in an `if`-chain family that reaches users through provider-native
//! `--model` passthrough was covered by NEITHER, which is #165's exact shape:
//! a frontier model ships, nobody adds its limits, the miss produces no error,
//! and a run dies against a fake ceiling. It cost three real defects in one
//! cycle -- `claude-opus-5` with no arm at all (32,768 substituted for a
//! 1,000,000 window, output clamped to 8,192), `gpt-4o-2024-05-13`
//! over-claiming output 4x, and the `gemini-*-latest` aliases 32x undersized.
//! All three were found BY HAND.
//!
//! THE CHAIN OF CUSTODY
//! --------------------
//! This table is graded from both ends, so neither end can rot alone:
//!
//!   * against the CODE -- `every_passthrough_vendor_model_resolves_its_arm`
//!     (in `limits.rs`) asserts the real `model_output_ceiling` returns
//!     exactly these figures for every id here. Delete an arm and that test
//!     goes red in plain CI, on every PR. This is the half a text parser
//!     could never do: the chain evaluates itself.
//!   * against the WORLD -- `check-model-limits-freshness.py` parses this same
//!     const at release time and grades it against a live models.dev pull:
//!     an in-scope vendor id with no row here FAILS, and a row that
//!     over-claims context or output against the vendor floor FAILS.
//!
//! WHAT MAY GO IN
//! --------------
//! Vendor-operated rows only, per AGENTS.md. Aggregators publish junk
//! (`ctx=0`, `out=1010000`, dropped digits) and resellers of open weights
//! serve the same id at wildly different limits. `output == context` is
//! models.dev saying UNKNOWN, never a ceiling -- `grok-4.5` / `grok-4.6`
//! report 500,000 both ways and their OUTPUT is therefore ungraded (the
//! context is not in doubt).
//!
//! OPEN WEIGHTS, STATED HONESTLY. This module used to claim "no open-weights
//! family is listed here". That was false on its own contents: the MiniMax M2
//! and M3 rows and the DeepSeek V4 rows below are open-weights ids, served by
//! third parties at their own limits, and `model_output_ceiling` is keyed on
//! the model id ALONE, so each arm is handed to every host serving that name.
//!
//! PROVENANCE, MEASURED -- an earlier draft of this paragraph said the twelve
//! rows "predate this table's guards", and that conflated the ROWS with the
//! ARMS. The rows are new: `git log --oneline -- limits/passthrough.rs` shows
//! this file has two commits and both are #1176's. The ARMS they assert are
//! older, and that is what predates the guards:
//! `git log --oneline -S "minimax-m2.5" -- limits.rs` -> `e17a33b22` (the #165
//! fix), `-S "deepseek-v4-pro"` -> `30dad572c` then `9d3f33c3e`, and
//! `git show beb335953^:crates/wcore-config/src/limits.rs | grep minimax`
//! finds `minimax-m2.5` already in the chain one commit before this file
//! existed (control: `-S "claude"` over the same path returns 4 commits, so
//! the query is not silently empty). #1176 added no arm; it pinned arms that
//! were already shipping.
//!
//! Twelve of the rows below are open-weights ids. On the 2026-08-30 pull SEVEN
//! of them violate AGENTS.md's third rule -- `deepseek-v4-flash` (8.0x across
//! 61 hosts), `deepseek-v4-flash-0731` (5.1x/35), `deepseek-v4-pro` (8.2x/64),
//! `minimax-m2` (5.1x/19), `minimax-m2.1` (5.1x/24), `minimax-m2.5`
//! (65,536-228,700, 3.5x/44) and `minimax-m3` (4.0x/43). The other five have
//! hosts that agree and are not violations.
//!
//! THE DECISION, RECORDED (FerroxLabs/wayland#1232). All twelve rows STAY.
//! The seven violating ids are now PROVIDER-SCOPED rather than removed --
//! `OPEN_WEIGHTS_HOST_SPREAD` in `limits.rs` gates them to the vendor that
//! operates each, so `model_output_ceiling` answers with the verified figures
//! on `deepseek` / `minimax` (and their tenant spellings) and with `None`
//! everywhere else. The five agreeing ids are NOT gated and keep their global
//! arm.
//!
//! Removal was the obvious reading of rule 3 and it is the wrong one here, for
//! a reason that is easy to miss: an arm revokes `should_omit_max_tokens`, but
//! that omission only exists on an OMIT-SAFE preset (`gemini`, `openrouter`,
//! `flux-router`). DeepSeek's and MiniMax's own endpoints are plain
//! `openai_compat_provider` presets, which are not omit-safe -- so on the
//! vendor's own API a deleted arm restores no natural ceiling at all. It drops
//! output to `UNKNOWN_CAP` (8,192) and the window to
//! `UNVERIFIED_CONTEXT_WINDOW` (32,768): the 47x cut #1157 was filed to fix.
//! Scoping gets rule 3's intended outcome on every OTHER route -- `None` on an
//! omit-safe reseller restores the host's natural ceiling, and `None` anywhere
//! else errs LOW (32,768 / 8,192, which IS the measured host floor for both
//! families) instead of the ceiling-death direction of #165.
//!
//! Enforced by a test, not by this paragraph:
//! `host_variable_open_weights_arms_are_provider_scoped` fails if a later
//! change re-globalises one of the seven OR deletes it outright, and
//! `open_weights_rows_are_longest_fragment_first` pins the substring ordering
//! that keeps the five agreeing rows out of the gate.
//!
//! Both directions ARE enforced by `check-model-limits-freshness.py`:
//! `scan_passthrough` will not DEMAND an arm for a new open-weights id whose
//! hosts disagree by 2x or more, and `scan_open_weights_arms` reads this
//! table's own contents on every run, so a new host-variable open-weights arm
//! that is ADDED here FAILS the release. The seven above are no longer debt:
//! `provider_scoped_arms` parses `OPEN_WEIGHTS_HOST_SPREAD` out of the Rust
//! source on every run, so un-scoping an arm makes rule 3 fail it again with
//! no edit to the script. `OPEN_WEIGHTS_ARM_DEBT` is now empty and kept, keyed
//! on the exact model id so that listing one could never excuse the next.
//!
//! Ids are recorded in their CANONICAL vendor spelling. The lookup is a
//! substring match, so the provider-specific dressings -- `anthropic.`,
//! `us.`/`eu.`/`jp.`/`au.`/`global.`, `@default`, `@20250929`, `-v1:0` --
//! all resolve through the same arm and do not need their own rows.
//!
//! Snapshot: models.dev, 2026-08-28 (HTTP 200, 4,424,885 bytes).

/// `(provider-native model id, expected output ceiling, expected context window)`.
///
/// Every entry is an id a user can pass to `--model` on a vendor-operated
/// endpoint. See the module docs for what may be added and how the table is
/// graded.
pub(crate) const PASSTHROUGH_VENDOR_MODELS: &[(&str, u32, u32)] = &[
    // --- Anthropic Claude 4.x / 5 - vendor rows: anthropic, google-vertex,
    // amazon-bedrock (incl. the us./eu./jp./au./global. regional spellings and
    // the anthropic. prefix, all of which the substring lookup covers).
    ("claude-fable-5", 128_000, 1_000_000),
    ("claude-haiku-4-5", 64_000, 200_000),
    ("claude-haiku-4-5-20251001", 64_000, 200_000),
    ("claude-opus-4", 32_000, 200_000),
    ("claude-opus-4-1", 32_000, 200_000),
    ("claude-opus-4-1-20250805", 32_000, 200_000),
    ("claude-opus-4-5", 64_000, 200_000),
    ("claude-opus-4-5-20251101", 64_000, 200_000),
    ("claude-opus-4-6", 128_000, 1_000_000),
    ("claude-opus-4-7", 128_000, 1_000_000),
    ("claude-opus-4-8", 128_000, 1_000_000),
    ("claude-opus-5", 128_000, 1_000_000),
    ("claude-sonnet-4", 64_000, 200_000),
    ("claude-sonnet-4-5", 64_000, 200_000),
    ("claude-sonnet-4-5-20250929", 64_000, 200_000),
    ("claude-sonnet-4-6", 128_000, 1_000_000),
    ("claude-sonnet-5", 128_000, 1_000_000),
    // --- OpenAI GPT-4o / 4.1 / 5.x - vendor rows: openai, azure.
    ("gpt-4.1", 32_768, 1_000_000),
    ("gpt-4.1-mini", 32_768, 1_000_000),
    ("gpt-4.1-nano", 32_768, 1_000_000),
    ("gpt-4o", 16_384, 128_000),
    ("gpt-4o-2024-05-13", 4_096, 128_000),
    ("gpt-4o-2024-08-06", 16_384, 128_000),
    ("gpt-4o-2024-11-20", 16_384, 128_000),
    ("gpt-4o-mini", 16_384, 128_000),
    ("gpt-5", 128_000, 400_000),
    ("gpt-5-codex", 128_000, 400_000),
    ("gpt-5-mini", 128_000, 400_000),
    ("gpt-5-nano", 128_000, 400_000),
    ("gpt-5-pro", 128_000, 400_000),
    ("gpt-5.1", 128_000, 400_000),
    ("gpt-5.1-codex", 128_000, 400_000),
    ("gpt-5.1-codex-max", 128_000, 400_000),
    ("gpt-5.1-codex-mini", 128_000, 400_000),
    ("gpt-5.2", 128_000, 400_000),
    ("gpt-5.2-chat-latest", 16_384, 128_000),
    ("gpt-5.2-codex", 128_000, 400_000),
    ("gpt-5.2-pro", 128_000, 400_000),
    ("gpt-5.3-chat-latest", 16_384, 128_000),
    ("gpt-5.3-codex", 128_000, 400_000),
    ("gpt-5.3-codex-spark", 32_000, 128_000),
    ("gpt-5.4", 128_000, 1_050_000),
    ("gpt-5.4-mini", 128_000, 400_000),
    ("gpt-5.4-nano", 128_000, 400_000),
    ("gpt-5.4-pro", 128_000, 1_050_000),
    ("gpt-5.5", 128_000, 1_050_000),
    ("gpt-5.5-pro", 128_000, 1_050_000),
    ("gpt-5.6", 128_000, 1_050_000),
    ("gpt-5.6-luna", 128_000, 1_050_000),
    ("gpt-5.6-sol", 128_000, 1_050_000),
    ("gpt-5.6-terra", 128_000, 1_050_000),
    // --- xAI Grok 3.x / 4.x - vendor rows: xai, amazon-bedrock (xai. prefix).
    ("grok-4.20-0309-non-reasoning", 30_000, 1_000_000),
    ("grok-4.20-0309-reasoning", 30_000, 1_000_000),
    ("grok-4.20-multi-agent-0309", 30_000, 1_000_000),
    ("grok-4.3", 30_000, 1_000_000),
    ("grok-4.5", 500_000, 500_000),
    ("grok-4.6", 500_000, 500_000),
    // --- Google Gemini text tiers - vendor rows: google, google-vertex. The
    // -image / -tts / -native-audio / -live variants are deliberately excluded
    // from the chain and so are excluded here.
    ("gemini-2.5-flash", 65_536, 1_048_576),
    ("gemini-2.5-flash-lite", 65_536, 1_048_576),
    ("gemini-2.5-pro", 65_536, 1_048_576),
    ("gemini-3-flash-preview", 65_536, 1_048_576),
    ("gemini-3.1-flash-lite", 65_536, 1_048_576),
    ("gemini-3.1-flash-lite-preview", 65_536, 1_048_576),
    ("gemini-3.1-pro-preview", 65_536, 1_048_576),
    ("gemini-3.1-pro-preview-customtools", 65_536, 1_048_576),
    ("gemini-3.5-flash", 65_536, 1_048_576),
    ("gemini-3.5-flash-lite", 65_536, 1_048_576),
    ("gemini-3.6-flash", 65_536, 1_048_576),
    ("gemini-3.7-flash", 65_536, 1_048_576),
    ("gemini-flash-latest", 65_536, 1_048_576),
    ("gemini-flash-lite-latest", 65_536, 1_048_576),
    // --- DeepSeek V4 - vendor rows: deepseek, alibaba, alibaba-cn,
    // alibaba-token-plan. The V3 / R1 generation is a deliberate exclusion.
    ("deepseek-v4-flash", 384_000, 1_000_000),
    ("deepseek-v4-flash-0731", 384_000, 1_000_000),
    ("deepseek-v4-flash-vision-exp", 384_000, 1_000_000),
    ("deepseek-v4-pro", 384_000, 1_000_000),
    ("deepseek-v4-pro-0813", 384_000, 1_000_000),
    // --- MiniMax M2 / M3 - vendor rows: minimax, minimax-cn,
    // minimax-coding-plan.
    ("minimax-m2", 128_000, 196_608),
    ("minimax-m2.1", 128_000, 204_800),
    ("minimax-m2.5", 128_000, 204_800),
    ("minimax-m2.5-highspeed", 128_000, 204_800),
    ("minimax-m2.7", 128_000, 204_800),
    ("minimax-m2.7-highspeed", 128_000, 204_800),
    ("minimax-m3", 512_000, 1_048_576),
];
