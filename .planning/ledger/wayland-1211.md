---
issue: 1211
repo: FerroxLabs/wayland
kind: defect
title: "is_self_hosted_base_url reads the authority past a query string, so a public host spelled with ?x=@127.0.0.1 is treated as self-hosted"
status: closed
last_verified_commit: 65b95a87
criteria:
  - id: c1
    text: "https://api.openai.com?x=@127.0.0.1 and https://h?a=@10.0.0.1 are classified NOT self-hosted"
    state: met
    evidence: "test:crates/wcore-providers/src/openai.rs::public_base_urls_are_not_self_hosted"
    owner: core
    note: "Both named spellings are rows in this list. Graded HERE, at the provider-side consumer the ticket names (openai.rs:157 `select_key` sends SELF_HOSTED_PLACEHOLDER_KEY on a `true`), so c1 and c3 do not share one anchor: c3 grades the config-side list the criterion names by test name. RED ARM 2026-08-30 (mutation M1): today's hand-cut authority restored inside `is_self_hosted_base_url` -- diff-verified to land on executable code between `pub fn is_self_hosted_base_url` and the `match`, not on a doc comment, by printing the whole function body after the edit -- gives verbatim: `thread 'openai::tests::public_base_urls_are_not_self_hosted' panicked at crates/wcore-providers/src/openai.rs:6714:13: expected public: https://api.openai.com?x=@127.0.0.1`, `Summary 2 tests run: 1 passed, 1 failed`. The sibling `self_hosted_base_urls_are_detected` stayed GREEN under the same mutation, so this row is not riding on a mutation that breaks the predicate outright. Restored with `git checkout --`, touched, re-run green."
  - id: c2
    text: "The authority is parsed with a URL parser, or cut at the first of '/', '?' and '#'"
    state: met
    evidence: "absent:crates/wcore-config/src/self_hosted.rs::rsplit"
    owner: core
    note: "The FIRST branch of the criterion, not the second: the host now comes from `url::Url::parse` + `Url::host()` (`Host::Domain`/`Ipv4`/`Ipv6`), the same parser reqwest is built on, so the predicate cannot disagree with the address the request is dialed against. The hand-cut chain the ticket names is GONE from the file, which is what this token re-reads on every run; its known-positive control is that the path must exist, and in the same call `grep -c Url::parse` returns 1 while `grep rsplit` exits 1. Measured reason the second branch would NOT have been enough: cutting at the first of '/', '?', '#' still misreads `https://api.openai.com\\@127.0.0.1/v1`, because for a special scheme the WHATWG parser maps '\\' to a path separator -- probed before writing the fix (`PROBE \"https://api.openai.com\\\\@127.0.0.1/v1\" -> host=Some(Domain(\"api.openai.com\"))`) and independently confirmed by the audit fix one crate over, crates/wcore-mcp/src/transport/sse.rs::resolve_endpoint_backslash_at_smuggle_rejected. That backslash row is carried in the c3 test, so a future rewrite back to a hand cut reds a test as well as this token. FAIL-CLOSED CHANGE, recorded rather than buried: a base_url with no parsable host (scheme-less, e.g. `localhost:11434`) now classifies NOT self-hosted where the old string walk said self-hosted. The only thing a `true` unlocks is waiving a credential, so an address that cannot be resolved must fail closed; no test in the tree asserted the scheme-less spelling, and such a URL cannot reach the wire anyway (reqwest rejects it)."
  - id: c3
    text: "Both spellings are added to the_locality_predicate_rejects_public_hosts, which is shown RED against today's rsplit('@')"
    state: met
    evidence: "test:crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs::the_locality_predicate_rejects_public_hosts"
    owner: core
    note: "Both named spellings are rows, plus three more of the same class: the '#' fragment spelling, the backslash spelling that only a real URL parser gets right, and real userinfo pointed the other way -- a loopback literal used as the USER name in front of a public host, which is the spelling `rsplit` gets RIGHT and a naive left-hand read gets wrong. RED ARM 2026-08-30 under mutation M1 (the same restored hand-cut authority as c1), verbatim: `thread 'the_locality_predicate_rejects_public_hosts' panicked at crates/wcore-config/tests/keyless_self_hosted_endpoint_test.rs:299:9: expected public: https://api.openai.com?x=@127.0.0.1`. In the same run the file's four pre-existing negative controls and both positive tests stayed GREEN (`10 tests run: 7 passed, 3 failed`), so the mutation is discriminating rather than a blanket break. Restored, touched, re-run `10 tests run: 10 passed`."
  - id: c4
    text: "With every credential variable unset, --base-url 'https://api.openai.com?x=@127.0.0.1' refuses to start; the run is quoted"
    state: met
    evidence: "test:crates/wcore-cli/tests/keyless_local_endpoint_e2e.rs::a_public_host_spelled_as_loopback_in_the_query_is_still_refused"
    owner: core
    note: "THE RUN, on hetzner at 65b95a87, the shipped binary, every credential variable stripped with `env -u` (API_KEY OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY GOOGLE_API_KEY OPENROUTER_API_KEY DEEPSEEK_API_KEY GROQ_API_KEY AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_PROFILE WAYLAND_VAULT_PASSPHRASE WAYLAND_VAULT_PASSPHRASE_FD) and WAYLAND_HOME/HOME redirected to a throwaway dir: `./target/debug/wayland-core --no-tui --provider openai --model qwen3:8b --base-url \"https://api.openai.com?x=@127.0.0.1\" --project-dir /tmp/scg-live/proj say hello` -> `RC=1`, stdout empty, stderr ends `Error: No API key found. Add one with 'wayland-core auth add <provider> <key>' ...`. The cited test is that run automated through CARGO_BIN_EXE_wayland-core. RED ARM 2026-08-30 under mutation M1, and it reproduces the TICKET'S OWN observation rather than a test artefact -- verbatim: `thread 'a_public_host_spelled_as_loopback_in_the_query_is_still_refused' panicked at crates/wcore-cli/tests/keyless_local_endpoint_e2e.rs:284:5: the refusal must still name the missing credential -- reaching the network at all means the exemption fired on a public host` with `error: Provider rejected this request: API error 421`, which is the 421 the sweep recorded. The pre-existing control `a_remote_endpoint_without_a_key_is_still_refused` stayed GREEN in the same run (`2 tests run: 1 passed, 1 failed`), so this row is graded by the query spelling and not by the plain-remote case. Restored, touched, re-run green."
---

The predicate took the authority as everything before the first `/` and then
the last `@`-separated part, so every byte of a query string was authority.
`https://api.openai.com?x=@127.0.0.1` classified as loopback, the startup
credential gate waived the key, and the CLI dispatched the user's prompt to
api.openai.com.

The host now comes from `url::Url::parse` -- the same parser the HTTP client
is built on -- so the predicate cannot disagree with the address the request
goes to. Cutting at the first of `/`, `?`, `#` (the criterion's second branch)
would have closed the reported spelling and left the backslash spelling open;
both are rows in the polarity test now.

Fixed in 059291c0e and 65b95a875 on lane/f13-sec-credgate. Ordered BEFORE
wayland#1212 deliberately: #1212 c2 makes the two credential gates share this
predicate, so sharing it while it was broken would have spread the
misclassification to a second gate.
