# wcore-cli::deterministic_openai_loop::packaged_f04_run_is_repeatable_and_content_addressed (Windows only)

**Confidence (self-reported):** confident

## Root cause

The workspace-normalising hasher does not recognise the workspace path when it appears JSON-ESCAPED, and on Windows the OpenAI wire format guarantees it appears that way from the second request of a turn onward. `crates/wcore-providers/src/openai.rs:435` serialises an assistant tool call's input as `"arguments": serde_json::to_string(input)` — a whole JSON document stored as a JSON *string* — so once the fixture parses the request body, the `arguments` leaf is a `Value::String` whose bytes are `{"file_path":"C:\\Users\\...\\.tmpXXXX\\repository\\src\\settings.toml"}` with every separator DOUBLED. `workspace_evidence::hash_json` treats that leaf as an opaque string and calls `hash_text` on it (workspace_evidence.rs:113-116), and `workspace_forms` only ever offers the plain spelling, its canonicalised spelling, and (on Windows) the `/`-substituted spelling (workspace_evidence.rs:73-85). None of those match `C:\\Users\\...`, so the random per-run TempDir name is hashed verbatim into `semantic_body_sha256` and two runs can never agree. Request 1 has no tool call yet — the workspace only appears as plain text in the system prompt — which is exactly why element [0] is identical and [1..] are not. The per-leaf assertion stays silent because `insert_semantic_leaf_hash` (fixtures/openai.rs:640-650) hands the leaf's raw bytes back to `semantic_sha256`, which JSON-PARSES them, so the leaf path sees the UNescaped path and normalises it correctly — the leaf digests agree while the body digest does not. It is Windows-only because JSON escapes `\` and nothing else in a path: on Linux/macOS `/tmp/.tmpXXXX/...` survives JSON encoding byte-identical and matches the plain spelling.

## Evidence

- crates/wcore-providers/src/openai.rs:435 — `"arguments": serde_json::to_string(input).unwrap_or_default()` — the tool-call input is a JSON document stored as a JSON string, so every `\` in a Windows path is doubled inside it. By contrast the tool RESULT is a plain string (`openai.rs:485` `"content": content`), which is why only the assistant `tool_calls` leaf is affected.
- crates/wcore-eval-scenarios/src/workspace_evidence.rs:73-85 — `push_spellings` offers only the native spelling and, `#[cfg(windows)]`, `path.replace('\\', "/")`. There is no escaped spelling, so `next_workspace` cannot find `C:\\Users\\…` in the decoded `arguments` text.
- crates/wcore-eval-scenarios/src/workspace_evidence.rs:113-116 — `Value::String(value) => { hasher.update(b"S"); hash_text(hasher, value.as_bytes(), workspace_forms); }` — the body hash never re-parses an embedded JSON document; it hashes the escaped text as-is.
- crates/wcore-eval-scenarios/src/fixtures/openai.rs:640-650 — `insert_semantic_leaf_hash` does `encoded = text.as_bytes()` then `workspace_evidence::semantic_sha256(...)`, and `semantic_sha256` (workspace_evidence.rs:19-24) JSON-parses its input, so the LEAF path descends into the escaped document and sees the unescaped path. That is the asymmetry that lets `assert_request_leaves_equal` (deterministic_openai_loop.rs:685-700) pass while line 1054 fails.
- crates/wcore-cli/tests/deterministic_openai_loop.rs:882-901 — the F04 script's first three steps are `OpenAiStep::tool_call(... settings_path.to_string_lossy() ...)`, i.e. the workspace path is inside every tool call's arguments; the workspace differs per run (`assert_ne!(first.workspace, second.workspace)`, line 1051).
- crates/wcore-eval-scenarios/tests/openai_fixture_contract.rs:86-90 — the fixture-level contract that should have caught this uses `("C:/private/openai-run-a", …)` — FORWARD slashes on Windows. A `/`-spelled root has nothing for JSON to escape, so this contract could never reproduce the failure.
- Simulation (scratchpad/sim.py, a line-for-line Python port of workspace_evidence.rs + the leaf decomposition, run locally): `BEFORE fix: request1 body equal=True  request2 body equal=False  request2 leaves equal=True` / `AFTER fix: request1 body equal=True  request2 body equal=True  request2 leaves equal=True` / `known-negative (different file, same workspace) equal: False`. This reproduces the exact three-way signature in the panic — [0] identical, [1..] divergent, leaves silent — and shows the fix closes it without collapsing genuinely different runs.

## How to verify

Three levels, cheapest first.

1) Linux (hetzner-dsm, when a cargo slot is free) — the new unit gate runs everywhere:
   `cargo nextest run -p wcore-eval-scenarios workspace_evidence::tests::a_json_encoded_workspace_path_normalizes`
   Observable: PASS. Run it on the UNPATCHED tree first (apply only the test half of the diff) and it must FAIL with the two `semantic_sha256` values unequal — that is the pre/post discriminator. Note the unix leg deliberately names its workspace directories `run\a` / `run\b` so a backslash exists to escape; if that setup ever stops being possible the in-test positive control (`assert_ne!(native, native.replace('\\', "\\\\"))`) fails loudly rather than passing vacuously.

2) Windows (SeanDesktop / ferrox-win-msvc) — the fixture-layer gate:
   `cargo nextest run -p wcore-eval-scenarios --test openai_fixture_contract workspace_aware_identity_is_root_independent_and_collision_free`
   Observable: PASS after the patch, FAIL before it (its `assert_eq!(semantic_body_sha256, …)` breaks) now that the Windows roots use native `\` and the body carries a JSON-encoded `arguments`.

3) Windows — the real gate that reported the defect:
   `cargo nextest run -p wcore-cli --test deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed`
   Observable: PASS, i.e. line 1054 no longer fires. If it still fires, dump both bodies and diff them: the fixture already records `body_sha256` per request, so add a temporary `std::fs::write` of `body` next to the `semantic_body_sha256` computation in `crates/wcore-eval-scenarios/src/fixtures/openai.rs:461` keyed by `state.requests.len()`, run the test twice under `WCORE_F04_EVIDENCE_DIR`, and `diff` request 2 of run 1 against request 2 of run 2. Any surviving per-run token will be visible in that diff — that is the measurement that settles any residual cause I could not see from source.

I have NOT run any of these: no cargo on the Mac, no Windows host in this session. What I did observe is the Python port of the algorithm reproducing the exact failure signature and the fix closing it (`python3 scratchpad/sim.py`), plus `rustfmt --edition 2024 --check` clean and `git apply --check` clean on both files.

## Mutant

Delete the single line `push_form(forms, path.replace('\\', "\\\\").into_bytes());` from `push_spellings`. `workspace_evidence::tests::a_json_encoded_workspace_path_normalizes` must then FAIL on Linux/macOS with the two digests unequal, and `openai_fixture_contract::workspace_aware_identity_is_root_independent_and_collision_free` plus the F04 gate must fail on Windows. Second mutant, aimed at the opposite failure mode (a gate that can no longer fail): make the escaped form swallow the tail — e.g. push `path.replace('\\', "\\\\") + "\\\\src"` — and the known-negative `assert_ne!` at the end of the new test must fire, proving the test still discriminates genuinely different runs rather than merely normalising everything to a constant.

## Unknowns

- I could not run cargo on any platform, so nothing here is compile-verified. The new unit test uses `NAMES.map(...)` over a `const [&str; 2]` and relies on `&PathBuf -> &Path` deref coercion into the typed closure; I believe both are sound but a compile is the proof.
- I could not run on Windows, so I did not OBSERVE the failing test turn green. The mechanism is observed only in a faithful Python port of the same algorithm. If a SECOND Windows-only nondeterminism also exists (the leaf assertion cannot see it either, so it is not excluded by the evidence), the F04 gate would still fail — step 3 of how_to_verify names the dump-and-diff measurement that would expose it.
- Blast radius on other known-negative assertions: adding a spelling can only make more evidence compare EQUAL, never less. Every `assert_ne!` on a semantic digest I inspected (`receipt_contract.rs:395-431`, `:490-528`) uses `/private/run-a`-style roots with no backslash, so it is unaffected on every platform. I did not exhaustively audit `packaged_driver_gate.rs`, `journey_receipt_contract.rs`, `f28_receipt_contract.rs` for a Windows known-negative that relies on an escaped workspace path staying un-normalised.
- The fix recognises exactly ONE level of JSON escaping — deliberately, because that is exactly what the leaf decomposition already sees (`insert_semantic_leaf_hash` parses once). A path double-encoded twice (a JSON string inside a JSON string inside `arguments`) would still leak. Nothing in the current OpenAI shape produces that.
- Design residual, not fixed here: `assert_request_leaves_equal` is structurally incapable of catching any divergence the leaf path normalises more aggressively than the body path — the leaf path silently re-parses embedded JSON, the body path does not. It stayed silent through this entire failure. Worth a follow-up so the leaf assertion is a real second opinion rather than a strictly weaker one.

## Proposed patch (NOT APPLIED, NOT COMPILED)

```diff
--- a/crates/wcore-eval-scenarios/src/workspace_evidence.rs
+++ b/crates/wcore-eval-scenarios/src/workspace_evidence.rs
@@ -71,19 +71,38 @@
 /// Append every spelling of one pathname the platform can produce, skipping
 /// duplicates so `next_workspace` never scans the same bytes twice.
 fn push_spellings(forms: &mut Vec<Vec<u8>>, path: &str) {
-    let native = path.as_bytes().to_vec();
-    if !forms.contains(&native) {
-        forms.push(native);
-    }
+    push_form(forms, path.as_bytes().to_vec());
+    // The workspace also reaches evidence from INSIDE a JSON-encoded string,
+    // where every `\` is doubled. That is not hypothetical: an OpenAI request
+    // carries a tool call's input as `function.arguments` — a whole JSON
+    // document stored as a JSON *string*, built by
+    // `serde_json::to_string(input)` in `crates/wcore-providers/src/openai.rs`
+    // — so from the SECOND request of a turn onward the body embeds
+    // `C:\\Users\\…\\.tmpXXXX` while the harness holds `C:\Users\…\.tmpXXXX`.
+    //
+    // Left unmatched, the random per-run directory name survives into
+    // `semantic_body_sha256` and no two runs of the F04 repeatability gate can
+    // agree — while every per-LEAF digest still agrees, because
+    // `insert_semantic_leaf_hash` re-parses that leaf as JSON and therefore
+    // sees the UNescaped path. That asymmetry is exactly the shape the failure
+    // took on Windows: request 1 identical, requests 2..n divergent, and
+    // `assert_request_leaves_equal` silent throughout.
+    //
+    // `/`-separated platforms have nothing to escape, so this spelling
+    // deduplicates away there and their digests are byte-for-byte unchanged.
+    push_form(forms, path.replace('\\', "\\\\").into_bytes());
     #[cfg(windows)]
     {
-        let slash = path.replace('\\', "/").into_bytes();
-        if !forms.contains(&slash) {
-            forms.push(slash);
-        }
+        push_form(forms, path.replace('\\', "/").into_bytes());
     }
 }
 
+fn push_form(forms: &mut Vec<Vec<u8>>, form: Vec<u8>) {
+    if !forms.contains(&form) {
+        forms.push(form);
+    }
+}
+
 fn hash_json(hasher: &mut Sha256, value: &serde_json::Value, workspace_forms: &[Vec<u8>]) {
     match value {
         serde_json::Value::Null => hasher.update(b"N"),
@@ -179,6 +198,8 @@
 
 #[cfg(test)]
 mod tests {
+    use std::path::Path;
+
     use super::semantic_sha256;
 
     #[test]
@@ -246,7 +267,82 @@
         assert_ne!(
             semantic_sha256(b"test", evidence(0).as_bytes(), &links[0]).unwrap(),
             semantic_sha256(b"test", deeper.as_bytes(), &links[0]).unwrap()
+        );
+    }
+
+    /// The workspace reaches an OpenAI request body under a spelling the
+    /// harness never holds: JSON-ESCAPED, because `function.arguments` is a
+    /// whole JSON document stored as a JSON *string*. On Windows that doubles
+    /// every separator, the plain spelling stops matching, and the random
+    /// per-run directory name survives into `semantic_body_sha256` — the F04
+    /// repeatability gate then diverges from its SECOND request onward while
+    /// its per-leaf assertion stays silent (leaves are re-parsed as JSON, so
+    /// they see the unescaped path).
+    ///
+    /// JSON escapes only `\`, so on `/`-separated platforms the workspace
+    /// directory is given a name that CONTAINS one — legal on unix, and the
+    /// same code path Windows takes for every path it produces. Without that
+    /// this test would be green on Linux whether or not the bug is fixed.
+    #[test]
+    fn a_json_encoded_workspace_path_normalizes() {
+        #[cfg(unix)]
+        const NAMES: [&str; 2] = ["run\\a", "run\\b"];
+        #[cfg(not(unix))]
+        const NAMES: [&str; 2] = ["run-a", "run-b"];
+
+        let root = tempfile::tempdir().unwrap();
+        let workspaces = NAMES.map(|name| {
+            let directory = root.path().join(name);
+            std::fs::create_dir(&directory).unwrap();
+            directory
+        });
+
+        // Positive control on the setup: the escaped spelling must actually
+        // differ from the plain one, or nothing below exercises the escaped
+        // form and the assertions pass unfixed.
+        let native = workspaces[0].to_str().unwrap();
+        assert_ne!(
+            native,
+            native.replace('\\', "\\\\"),
+            "this platform's workspace path has no escapable separator; the \
+             assertions below would prove nothing"
         );
+
+        // Exactly the shape an OpenAI request carries from its second call
+        // onward: an assistant tool call whose `arguments` is a JSON document
+        // encoded into a string.
+        let body = |workspace: &Path, file: &str| {
+            let arguments = serde_json::to_string(&serde_json::json!({
+                "file_path": workspace.join("src").join(file),
+            }))
+            .unwrap();
+            serde_json::to_vec(&serde_json::json!({
+                "messages": [{
+                    "role": "assistant",
+                    "tool_calls": [{
+                        "id": "call-edit",
+                        "type": "function",
+                        "function": {"name": "Edit", "arguments": arguments},
+                    }],
+                }],
+            }))
+            .unwrap()
+        };
+
+        let first = body(&workspaces[0], "settings.toml");
+        let second = body(&workspaces[1], "settings.toml");
+        assert_ne!(first, second, "the two runs must differ on the wire");
+        assert_eq!(
+            semantic_sha256(b"test", &first, &workspaces[0]).unwrap(),
+            semantic_sha256(b"test", &second, &workspaces[1]).unwrap()
+        );
+
+        // Known-negative: recognising the escaped spelling must not swallow the
+        // rest of the encoded document, or the gate could no longer fail.
+        assert_ne!(
+            semantic_sha256(b"test", &first, &workspaces[0]).unwrap(),
+            semantic_sha256(b"test", &body(&workspaces[0], "other.toml"), &workspaces[0]).unwrap()
+        );
     }
 
     #[test]
--- a/crates/wcore-eval-scenarios/tests/openai_fixture_contract.rs
+++ b/crates/wcore-eval-scenarios/tests/openai_fixture_contract.rs
@@ -83,8 +83,14 @@
     // `workspace_forms`, whose first guard is `!workspace.is_absolute()`. These are
     // synthetic identity roots — never created, never opened — so a drive prefix on
     // Windows preserves the test exactly while making the literal absolute there.
+    //
+    // The Windows roots use NATIVE `\` separators. They were `C:/…` until
+    // 2026-08-04, and that is precisely why this contract stayed green on
+    // Windows while the F04 repeatability gate diverged: a `/`-spelled root has
+    // nothing for JSON to escape, so the escaped-spelling case below could
+    // never arise here.
     let (first_root, second_root) = if cfg!(windows) {
-        ("C:/private/openai-run-a", "C:/private/openai-run-b")
+        ("C:\\private\\openai-run-a", "C:\\private\\openai-run-b")
     } else {
         ("/private/openai-run-a", "/private/openai-run-b")
     };
@@ -113,6 +119,37 @@
     let mut second_request = request("fixture-chat-v1");
     second_request["messages"][0]["content"] =
         json!(format!("Edit {second_root}/src/settings.toml"));
+    // From the SECOND request of a real turn onward, OpenAI carries the tool
+    // call's input as `function.arguments` — a whole JSON document stored as a
+    // JSON *string*, so every separator inside it is escaped. Recording only
+    // the plain spelling is what let the F04 repeatability gate diverge on
+    // Windows while every per-leaf digest agreed. On `/`-separated platforms
+    // this adds no discrimination (nothing to escape); on Windows it fails
+    // unless `push_spellings` recognises the escaped form.
+    let assistant_tool_call = |root: &str| {
+        json!({
+            "role": "assistant",
+            "tool_calls": [{
+                "id": "call-edit",
+                "type": "function",
+                "function": {
+                    "name": "Edit",
+                    "arguments": serde_json::to_string(
+                        &json!({"file_path": format!("{root}/src/settings.toml")}),
+                    )
+                    .expect("encode tool-call arguments"),
+                },
+            }],
+        })
+    };
+    first_request["messages"]
+        .as_array_mut()
+        .expect("first messages array")
+        .push(assistant_tool_call(first_root));
+    second_request["messages"]
+        .as_array_mut()
+        .expect("second messages array")
+        .push(assistant_tool_call(second_root));
     assert!(
         post(first.base_url(), first_request)
             .await

```
