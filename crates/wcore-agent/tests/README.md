# `wcore-agent` Tests

Integration tests live in `crates/wcore-agent/tests/`. Each top-level file
(`engine_test.rs`, `bootstrap_test.rs`, …) is a Cargo integration test binary.
The `e2e/` and `acceptance/` subdirectories are also test binaries, wired via
`Cargo.toml` `[[test]]` entries pointing at their `mod.rs`.

## Test-model constants

Model identifiers used in test code live in `tests/common/models.rs`. **Do not
hard-code provider model names inline** — route through an accessor:

```rust
let config = Config {
    model: crate::common::models::anthropic_haiku(),
    ..
};
```

### Adding a new test model

Add a function to `tests/common/models.rs`. Name by *role* (`anthropic_haiku`,
`openai_gpt4o_mini`) not by version, so callers never change when a provider
rolls a new minor version. The version pin lives in exactly one place.

### Overriding for CI

Each accessor honours a matching environment variable so CI can pin to a
known-good model without touching source:

```sh
E2E_ANTHROPIC_HAIKU=claude-haiku-4-6-preview cargo test --workspace
```

The env var name appears next to the default in each function. To upgrade a
default permanently, edit the default argument in `models.rs`.

### Why this exists

Hardcoded model names are a maintenance treadmill: every time a provider
deprecates a model, every test referencing it breaks, and the fix is scattered
across dozens of files. The single-source-of-truth pattern means deprecation
upgrades are one-line file edits.

The pattern was introduced after the `claude-haiku-4-20250514` deprecation
caused the two `anthropic::test_anthropic_*` e2e tests to fail at
`engine/main@097fe46` with a HTTP 404.

## Module layout

Each `[[test]]` binary (`e2e`, `acceptance`) has its own crate root, so the
standard `mod common;` resolves to a non-existent sibling. The common module
is re-hosted via `#[path = "../common/mod.rs"] mod common;` at the top of each
binary's `mod.rs`. Top-level integration test files (`engine_test.rs` etc.)
use the plain `mod common;` form because Cargo discovers `tests/common/mod.rs`
relative to the workspace root for those.
