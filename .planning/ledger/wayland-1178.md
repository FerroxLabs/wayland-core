---
issue: 1178
repo: FerroxLabs/wayland
kind: defect
title: "A base_url with the conventional /v1 suffix builds /v1/v1/chat/completions and 404s"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A base URL carrying the conventional /v1 suffix either reaches the endpoint or fails with an error naming the doubled path"
    state: met
    evidence: "symbol:crates/wcore-config/src/compat.rs::join_endpoint"
    owner: core
    note: "Counts the longest run of WHOLE path segments that is both a suffix of the base path and a prefix of api_path, exactly once, with the authority held aside via split_authority. Called from openai.rs:660 (chat), openai.rs:798 (models/responses) and ProviderCompat::endpoint_url, so all three surfaces are joined the same way."
  - id: c2
    text: "A test covers the bare-root spelling, http://host:port"
    state: met
    evidence: "test:crates/wcore-providers/src/openai.rs::chat_url_is_identical_for_all_three_base_spellings"
    owner: core
    note: "Iterates the bare host:port, trailing-slash, /v1 and /v1/ spellings through the real OpenAIProvider and asserts one identical URL."
  - id: c3
    text: "A test covers the /v1-suffixed spelling"
    state: met
    evidence: "test:crates/wcore-config/src/compat.rs::join_endpoint_collapses_a_duplicated_v1_segment"
    owner: core
    note: "The /v1-suffixed spelling at the unit, with negative controls join_endpoint_does_not_collapse_a_substring_match (/apiv1, /v10) and join_endpoint_never_reads_the_authority_as_a_path_segment (a host literally named v1)."
  - id: c4
    text: "A test covers the trailing-slash spelling"
    state: met
    evidence: "file:crates/wcore-config/src/compat.rs:1772"
    owner: core
    note: "The trailing-slash spelling, inside the c3 test's spelling loop; the /v1/ form is the next entry at :1774. Both resolve to the single-/v1 endpoint."
  - id: c5
    text: "Configs that already specify the bare root keep working unchanged"
    state: met
    evidence: "test:crates/wcore-config/src/compat.rs::join_endpoint_with_no_overlap_is_plain_concatenation"
    owner: core
    note: "Asserts join_endpoint(base, path) equals plain concatenation byte for byte for api.openai.com, api.together.xyz/v1 plus /chat/completions, and an Azure-style deployment path. Mirrored at the provider by catalog_style_v1_base_with_overridden_api_path_is_unchanged."
---

`openai_defaults()` sets `api_path` to `/v1/chat/completions` and appends it to
the configured base URL. The spelling every OpenAI-compatible server prints in
its own docs — `http://127.0.0.1:11434/v1` — therefore builds
`http://127.0.0.1:11434/v1/v1/chat/completions` and 404s. Only the bare root
works, and nothing in the failure points at the doubled path, so the natural
read is that the server is incompatible rather than that four characters are
surplus.

This affects every OpenAI-compat endpoint, not only local ones. It was found
while fixing #1173 and deliberately left out of that lane, whose scope was the
credential gate.

The three spellings are three criteria because the schema takes one piece of
evidence per criterion, and because the entire defect is that two of the three
look identical to a user and only one works. c5 records the constraint that
makes this more than a one-line change.
