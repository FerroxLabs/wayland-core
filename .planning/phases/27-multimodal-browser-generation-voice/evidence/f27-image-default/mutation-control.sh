#!/usr/bin/env bash
# F-27C3-04 — mutation control. LANE-BRIEF §3.2: "gates must be able to fail".
#
# Every assertion below is a claim about a DEFAULT. A test that reads a default
# is the easiest kind to write self-passing: it can pass on the value it was
# written against even if nothing reads that value at runtime. So each gate is
# broken at its own source of truth and must go RED.
#
# Rules applied:
#  - the baseline is asserted GREEN first (a gate already red proves nothing);
#  - the EXECUTED COUNT is read back, not the exit status (`cargo test <filter>`
#    exits 0 having run zero tests when the filter matches no name — measured);
#  - the source file is restored from a byte copy, never via git (the object
#    store is shared with other lanes);
#  - runs in /root/wayland-f27imgdef, a path unique to this lane (/tmp and the
#    repo are shared).

set -u
ROOT=/root/wayland-f27imgdef
CARGO=/root/.cargo/bin/cargo
COMPAT=$ROOT/crates/wcore-config/src/compat.rs
IMGGEN=$ROOT/crates/wcore-agent/src/tool_backends/image_gen.rs
BAK=/root/wayland-f27imgdef-mutbak
mkdir -p "$BAK"
cp "$COMPAT" "$BAK/compat.rs.orig"
cp "$IMGGEN" "$BAK/image_gen.rs.orig"
cd "$ROOT" || exit 2

PASSES=0; FAILS=0

# run <pkg> <filter> <expect-count> -> prints "PASSED=n FAILED=n" and sets $VERDICT
run() {
  local pkg="$1" filter="$2" expect="$3"
  local out; out="$($CARGO test -p "$pkg" --lib "$filter" -- --test-threads=1 2>&1)"
  local line; line="$(printf '%s' "$out" | grep -E '^test result:' | tail -1)"
  local ran;  ran="$(printf '%s' "$out"  | grep -cE '^test .* \.\.\. (ok|FAILED)')"
  local p;    p="$(printf '%s' "$line" | sed -n 's/.*ok\. \([0-9]*\) passed.*/\1/p')"
  [ -z "$p" ] && p="$(printf '%s' "$line" | sed -n 's/.*FAILED\. \([0-9]*\) passed.*/\1/p')"
  local f;    f="$(printf '%s' "$line" | sed -n 's/.*; \([0-9]*\) failed.*/\1/p')"
  if [ "$ran" -ne "$expect" ]; then
    VERDICT="VOID_RAN_${ran}_EXPECTED_${expect}"
  elif [ "${f:-x}" = "0" ]; then
    VERDICT="GREEN"
  else
    VERDICT="RED"
  fi
  echo "    executed=$ran passed=${p:-?} failed=${f:-?}  -> $VERDICT"
}

check() { # label expected-verdict
  if [ "$VERDICT" = "$2" ]; then echo "    [OK]   $1 = $2"; PASSES=$((PASSES+1));
  else echo "    [BAD]  $1 expected $2 got $VERDICT"; FAILS=$((FAILS+1)); fi
}

restore() { cp "$BAK/compat.rs.orig" "$COMPAT"; cp "$BAK/image_gen.rs.orig" "$IMGGEN"; }

echo "=== BASELINES (a mutation against an already-red gate proves nothing)"
echo "  B1 flux default"
run wcore-agent flux_session_defaults_to_flux_image_with_no_env_var 1; check B1 GREEN
echo "  B2 openai non-regression"
run wcore-agent openai_session_still_defaults_to_gpt_image_1 1;       check B2 GREEN
echo "  B3 models differ (compat layer)"
run wcore-config image_model_differs_between_openai_and_flux_router 1; check B3 GREEN
echo "  B4 merge ripple"
run wcore-config merge_user_image_model_overrides_preset 1;            check B4 GREEN
echo "  B5 env outranks compat"
run wcore-agent openai_image_model_env_var_still_outranks_the_compat_default 1; check B5 GREEN

echo
echo "=== M1 — revert the fix: flux_router_defaults declares no image model."
echo "        This is EXACTLY the pre-fix state: the resolver falls back to the"
echo "        global gpt-image-1, which is what failed live on a Flux key."
sed -i 's|^            image_model: Some("flux-image".into()),|            image_model: None,|' "$COMPAT"
grep -q 'image_model: Some("flux-image"' "$COMPAT" && { echo "    MUTATION DID NOT APPLY"; FAILS=$((FAILS+1)); }
run wcore-agent flux_session_defaults_to_flux_image_with_no_env_var 1;  check M1-flux RED
run wcore-config image_model_differs_between_openai_and_flux_router 1;  check M1-differ RED
run wcore-agent openai_session_still_defaults_to_gpt_image_1 1;         check M1-openai-unaffected GREEN
restore

echo
echo "=== M2 — give OpenAI the Flux model. The two-provider 'differ' assertion"
echo "        must catch it; asserting each value alone would not."
sed -i 's|^            image_model: Some("gpt-image-1".into()),|            image_model: Some("flux-image".into()),|' "$COMPAT"
run wcore-config image_model_differs_between_openai_and_flux_router 1;  check M2-differ RED
run wcore-agent openai_session_still_defaults_to_gpt_image_1 1;         check M2-openai RED
run wcore-agent openai_and_flux_resolve_different_models_from_the_same_code_path 1; check M2-resolver-differ RED
restore

echo
echo "=== M3 — drop the merge() arm (the documented ProviderCompat gotcha: a"
echo "        field not threaded through merge compiles and silently discards"
echo "        every user override)."
sed -i 's|^            image_model: user.image_model.or(defaults.image_model),|            image_model: defaults.image_model,|' "$COMPAT"
run wcore-config merge_user_image_model_overrides_preset 1;             check M3-merge RED
restore

echo
echo "=== M4 — resolver ignores the compat argument (the #310 shape: endpoint"
echo "        and key resolved from config, model left global)."
sed -i 's|^        config.compat.image_model.as_deref(),|        None,|' "$IMGGEN"
grep -q 'config.compat.image_model.as_deref()' "$IMGGEN" && { echo "    MUTATION DID NOT APPLY"; FAILS=$((FAILS+1)); }
run wcore-agent flux_session_defaults_to_flux_image_with_no_env_var 1;  check M4-flux RED
run wcore-agent openai_and_flux_resolve_different_models_from_the_same_code_path 1; check M4-differ RED
restore

echo
echo "=== RESTORED baseline re-check (a mutation left in place would poison"
echo "    every later measurement in this lane)"
run wcore-agent flux_session_defaults_to_flux_image_with_no_env_var 1;  check R1 GREEN
run wcore-config image_model_differs_between_openai_and_flux_router 1;  check R2 GREEN
run wcore-config merge_user_image_model_overrides_preset 1;             check R3 GREEN
if /usr/bin/diff -q "$BAK/compat.rs.orig" "$COMPAT" >/dev/null && \
   /usr/bin/diff -q "$BAK/image_gen.rs.orig" "$IMGGEN" >/dev/null; then
  echo "    [OK]   sources byte-identical to pre-mutation"; PASSES=$((PASSES+1))
else
  echo "    [BAD]  SOURCES NOT RESTORED"; FAILS=$((FAILS+1))
fi

echo
echo "MUTATION_CONTROL_PASSES=$PASSES MUTATION_CONTROL_FAILS=$FAILS"
if [ "$FAILS" -eq 0 ] && [ "$PASSES" -ge 16 ]; then
  echo "MUTATION_CONTROL=PASS"
else
  echo "MUTATION_CONTROL=FAIL"
fi
