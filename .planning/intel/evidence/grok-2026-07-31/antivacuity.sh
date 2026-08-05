#!/usr/bin/env bash
# Anti-vacuity matrix for crates/wcore-cli/tests/migrate_grok.rs.
#
# For each test: break the thing it guards, prove the test goes RED, revert,
# prove it goes GREEN again. A test that cannot fail has proven nothing.
set -u
cd /root/lanes/grok || exit 1
export PATH=/root/.cargo/bin:$PATH
export CARGO_TARGET_DIR=/root/lanes-target/grok

GROK=crates/wcore-cli/src/migrate/grok.rs
MOD=crates/wcore-cli/src/migrate/mod.rs
OUT=/tmp/antivacuity.log
: > "$OUT"

run_one() {  # run_one <testname> -> echoes GREEN / RED / BUILDFAIL / WRONG-COUNT
  local t="$1" log rc summary
  log=$(cargo nextest run -p wcore-cli --profile ci --retries 0 \
        -E "binary(migrate_grok) and test(=${t})" --no-fail-fast 2>&1); rc=$?
  # Only a COMPILER error is a build failure. `error: test run failed` is
  # nextest reporting a red test, which is the outcome this harness wants.
  if echo "$log" | grep -qE 'error\[E[0-9]+\]|error: could not compile'; then
    echo "BUILDFAIL"; echo "--- $t BUILDFAIL ---" >> "$OUT"
    echo "$log" | grep -E 'error\[E[0-9]+\]|error: could not compile' | head -5 >> "$OUT"
    return
  fi
  # Assert the executed count explicitly. nextest prints "1 test run" (SINGULAR)
  # for a single test, so a matcher requiring "tests run" would see nothing —
  # and "0 tests run" reads as success if you do not look at the count.
  summary=$(echo "$log" | grep -oE '[0-9]+ tests? run: [0-9]+ passed[^)]*' | tail -1)
  if [ -z "$summary" ]; then echo "NO-SUMMARY"; echo "$log" | tail -15 >> "$OUT"; return; fi
  if ! echo "$summary" | grep -qE '^1 tests? run'; then echo "WRONG-COUNT[$summary]"; return; fi
  echo "$t rc=$rc :: $summary" >> "$OUT"
  if [ "$rc" -eq 0 ]; then echo "GREEN"; else echo "RED"; fi
}

mutate() { python3 - "$@" <<'PY'
import sys, pathlib
path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
p = pathlib.Path(path); t = p.read_text()
n = t.count(old)
if n != 1:
    print(f"MUTATION-ANCHOR-COUNT={n} for {path}", file=sys.stderr); sys.exit(3)
p.write_text(t.replace(old, new))
PY
}

case_run() {  # case_run <label> <test> <file> <old> <new>
  local label="$1" test="$2" file="$3" old="$4" new="$5"
  if ! mutate "$file" "$old" "$new" 2>>"$OUT"; then
    echo "M${label} | ${test} | ANCHOR-MISS" | tee -a "$OUT"; return
  fi
  local red; red=$(run_one "$test")
  /usr/bin/git checkout -- "$file"
  local green; green=$(run_one "$test")
  echo "M${label} | ${test} | mutated=${red} reverted=${green}" | tee -a "$OUT"
}

echo "=== baseline: whole binary ===" | tee -a "$OUT"
cargo nextest run -p wcore-cli --profile ci -E "binary(migrate_grok)" --no-fail-fast 2>&1 \
  | grep -E 'tests? run|Summary' | tee -a "$OUT"

echo "=== mutations ===" | tee -a "$OUT"

# M1 — the provider binding the root setup publishes.
case_run 1 apply_writes_the_grok_root_profile "$GROK" \
  'const PROVIDER: &str = "xai";' 'const PROVIDER: &str = "openai";'

# M2 — promote the OIDC session into a profile api_key.
case_run 2 the_oidc_session_is_never_promoted_even_with_include_credentials "$GROK" \
  '    let auth_path = home.join(AUTH_FILENAME);
    let has_credential = auth_path.is_file();' \
  '    let auth_path = home.join(AUTH_FILENAME);
    let has_credential = auth_path.is_file();
    if has_credential {
        config.api_key = std::fs::read_to_string(&auth_path).ok();
    }'

# M3 — same mutation, corpus leg: a canary must escape into the Wayland home.
case_run 3 the_real_install_corpus_imports_and_leaks_no_canary "$GROK" \
  '    let auth_path = home.join(AUTH_FILENAME);
    let has_credential = auth_path.is_file();' \
  '    let auth_path = home.join(AUTH_FILENAME);
    let has_credential = auth_path.is_file();
    if has_credential {
        config.api_key = std::fs::read_to_string(&auth_path).ok();
    }'

# M4 — stop classifying a peer MCP entry as launchable, so it is written live.
case_run 4 an_executable_grok_mcp_definition_is_contained_not_written_live "$GROK" \
  '        command: s.command.clone(),' '        command: None,'

# M5 — honour `enabled = false` no longer.
case_run 5 a_disabled_grok_mcp_server_is_absent_from_every_destination "$GROK" \
  '        if srv.enabled == Some(false) {' '        if false {'

# M6 — make the root profile name vary per run.
case_run 6 the_grok_import_is_idempotent_without_overwrite "$GROK" \
  '        name: GROK_ROOT_PROFILE_ID.to_string(),' \
  '        name: format!(
            "{GROK_ROOT_PROFILE_ID}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ),'

# M7 — remove grok's version.json branch from peer_version.
case_run 7 version_json_reaches_the_provenance_record "$MOD" \
    '    if matches!(source, PeerSource::Grok)
        && let Ok(s) = std::fs::read_to_string(home.join("version.json"))' \
    '    if false
        && let Ok(s) = std::fs::read_to_string(home.join("version.json"))'

# M8 — accept any directory as a grok home.
case_run 8 missing_grok_home_errors "$GROK" \
  '    home.join(CONFIG_FILENAME).is_file() || home.join(AUTH_FILENAME).is_file()' \
  '    let _ = home;
    true'

# M9 — read the session store's CONTENTS into the plan.
case_run 9 the_importer_never_opens_the_session_store "$GROK" \
  '    mcp_conflicts.sort();' \
  '    if let Ok(s) = std::fs::read_to_string(home.join(AUTH_FILENAME)) {
        warnings.push(format!("session store: {}", s.len()));
    }
    mcp_conflicts.sort();'

# M10 — drop `sessions` from the deferred inventory.
case_run 10 the_real_install_deferred_inventory_matches_the_manifest "$GROK" \
  '    "sessions",
    "vendor",' '    "vendor",'

echo "=== done ===" | tee -a "$OUT"
/usr/bin/git status --porcelain | tee -a "$OUT"
