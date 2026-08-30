#!/usr/bin/env bash
# ADMISSION: caller-decides -- release-rehearsal.yml is manually dispatched
# and is not a required status context; its steps are a sequence, and a
# failure part-way through should stop the rehearsal rather than continue it.
#
# Rollback-rehearsal and state-separation drill (F29-04-03, SUPPLY-29-34).
#
# WHAT THIS CLOSES. Phase 29 built a four-state release ledger — packaging,
# deployment_preparation, rollback_rehearsal, release_acceptance — with per-state
# signature domains, role-bound keys, previous-record digest chaining and
# disjoint evidence sets. It proved the mechanism once, by hand, on hetzner.
# Nothing then executed it: measured at b2ddf113, `state-append` and
# `state-verify` appear ZERO times in any file under .github/workflows/, and
# `rollback` appears exactly once in .github/ — inside the *manifest* drill,
# where it names rollback PROTECTION (refusing a downgrade), which is a
# different property from rollback REHEARSAL. So the ledger was a mechanism
# nothing ran, and "rollback rehearsal" happened nowhere in CI.
#
# A rollback path nobody has ever executed is not a rollback path. This drill
# executes one on every run.
#
# WHAT IT DELIBERATELY DOES NOT DO. It cannot reach `release_acceptance`, and it
# asserts that it cannot. The shipped `manifest build` hardcodes
# `certification: Evidence::Unavailable` (open MEDIUM F29-04-01), and release
# acceptance gates on an OBSERVED certification binding. So the honest ceiling
# for a shipped-tool-only run is rollback_rehearsal, and this drill pins that
# ceiling as an assertion rather than working around it. If someone ever makes
# acceptance reachable without a certification binding, THIS DRILL GOES RED.
# That is the point: the ceiling is load-bearing, not a limitation being papered
# over.
#
# NO CREDENTIAL. Every key is minted at run time by `trust-root-init` into a
# mktemp directory trapped away on exit. No production seed is read and there is
# no code path here that could accept one. Seeds go in on STDIN only, never argv.
#
# ANTI-VACUITY. Exit status is not trusted anywhere. Every assertion reads a
# FIELD back out of the tool's own stdout — `highest_state`, `records`,
# `accepted` — and compares it to an expected value. A drill that merely checks
# rc=0 would pass against a tool that printed nothing, and a "the chain was
# refused" check would pass for free against a tool that refuses everything, so
# each refusal below is paired with the positive control it must not break.
# `/usr/bin/grep` throughout: the `rtk` proxy re-renders grep and cargo output.

set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

CARGO="$(command -v cargo || true)"
if [ -x "${HOME}/.cargo/bin/cargo" ]; then CARGO="${HOME}/.cargo/bin/cargo"; fi
if [ -z "${CARGO}" ]; then echo "cargo not found on PATH" >&2; exit 1; fi

workdir="$(mktemp -d)"
cleanup() { rm -rf "${workdir}"; }
trap cleanup EXIT

keys="${workdir}/keys"
artifacts="${workdir}/artifacts"
evidence="${workdir}/evidence"
mkdir -p "${artifacts}" "${evidence}"

NOW="$(date -u +%s)"
failures=0

say()  { printf '\n=== %s ===\n' "$1"; }
fail() { echo "FAIL: $1" >&2; failures=$((failures + 1)); }

# ---------------------------------------------------------------------------
say "build the real wayland-release binary"
# ---------------------------------------------------------------------------
"${CARGO}" build --release --locked -p wcore-eval-scenarios --bin wayland-release || {
  echo "could not build wayland-release" >&2; exit 1; }
tool="${repo_root}/target/release/wayland-release"
test -x "${tool}" || { echo "${tool} is not executable" >&2; exit 1; }

# ---------------------------------------------------------------------------
say "mint throwaway role keys (one per state, written 0600, public halves printed)"
# ---------------------------------------------------------------------------
"${tool}" trust-root-init --directory "${keys}" --valid-from 0 >/dev/null || {
  echo "trust-root-init failed" >&2; exit 1; }
trust_root="${keys}/trust-root.json"
test -s "${trust_root}" || { echo "no trust root produced" >&2; exit 1; }

seed_for() {  # seed_for <state>
  echo "${keys}/$(printf '%s' "$1" | tr '_' '-')-key.seed"
}
key_id_for() { printf '%s-key' "$(printf '%s' "$1" | tr '_' '-')"; }

# ---------------------------------------------------------------------------
say "package artifacts and build+sign the manifest the chain binds to"
# ---------------------------------------------------------------------------
head -c 4096 /dev/urandom > "${artifacts}/wayland-core-v0.0.0-drill-x86_64-unknown-linux-gnu.tar.gz"
( cd "${artifacts}" && sha256sum wayland-core-* > wayland-core-checksums.txt )

"${tool}" manifest-build \
  --artifacts "${artifacts}" \
  --output "${workdir}/manifest-unsigned.json" \
  --release-id "v0.0.0-drill" \
  --source-commit "$(printf 'a%.0s' {1..40})" \
  --sequence 1 \
  --issued-at "${NOW}" >/dev/null || { echo "manifest-build failed" >&2; exit 1; }

manifest="${workdir}/manifest.json"
"${tool}" manifest-sign \
  --manifest "${workdir}/manifest-unsigned.json" \
  --output "${manifest}" \
  --key-id "$(key_id_for release_acceptance)" \
  < "$(seed_for release_acceptance)" >/dev/null || { echo "manifest-sign failed" >&2; exit 1; }

# Disjoint evidence: each state must cite its own file. Reusing one across two
# states is one of the collapse attempts 29-04 refused, so the drill must not
# accidentally commit it.
for name in packaging deployment rehearsal acceptance; do
  printf 'evidence for %s, drill run at %s\n' "${name}" "${NOW}" > "${evidence}/${name}.txt"
done

# ---------------------------------------------------------------------------
# append <chain> <state> <evidence-file> [key-state-override]
# The override exists only so a collapse attempt can sign a state with the
# WRONG role's key while changing nothing else.
# ---------------------------------------------------------------------------
append() {
  local chain="$1" state="$2" ev="$3" key_state="${4:-$2}"
  "${tool}" state-append \
    --manifest "${manifest}" \
    --chain "${chain}" \
    --state "${state}" \
    --key-id "$(key_id_for "${key_state}")" \
    --evidence "${state}=${ev}" \
    < "$(seed_for "${key_state}")"
}

verify() {  # verify <chain>  -> prints the tool's line, returns its rc
  "${tool}" state-verify \
    --manifest "${manifest}" \
    --chain "$1" \
    --trust-root "${trust_root}" \
    --now "${NOW}"
}

field() {  # field <line> <name>
  printf '%s' "$1" | /usr/bin/grep -oE "$2=[A-Za-z0-9_]+" | /usr/bin/grep -oE '[^=]+$' | head -1
}

# ---------------------------------------------------------------------------
say "POSITIVE CONTROL: rehearse packaging -> deployment_preparation -> rollback_rehearsal"
# ---------------------------------------------------------------------------
chain="${workdir}/chain.json"
append "${chain}" packaging              "${evidence}/packaging.txt"  || fail "packaging append"
append "${chain}" deployment_preparation "${evidence}/deployment.txt" || fail "deployment append"
append "${chain}" rollback_rehearsal     "${evidence}/rehearsal.txt"  || fail "rollback_rehearsal append"

good_line="$(verify "${chain}")"; good_rc=$?
echo "${good_line}"
if [ "${good_rc}" -ne 0 ]; then
  fail "the rehearsed chain did not verify (rc=${good_rc}); every refusal below would then pass for the wrong reason"
fi
[ "$(field "${good_line}" highest_state)" = "rollback_rehearsal" ] \
  || fail "highest_state is '$(field "${good_line}" highest_state)', expected rollback_rehearsal — the rehearsal did not happen"
[ "$(field "${good_line}" records)" = "3" ] \
  || fail "records=$(field "${good_line}" records), expected 3"
[ "$(field "${good_line}" accepted)" = "false" ] \
  || fail "accepted=$(field "${good_line}" accepted), expected false — a shipped-tool-only run must NOT reach acceptance"
cp "${chain}" "${workdir}/chain.good"

# ---------------------------------------------------------------------------
say "CEILING: appending release_acceptance must NOT produce an accepted chain"
# ---------------------------------------------------------------------------
# F29-04-01: `manifest build` emits certification=Unavailable, and acceptance
# requires an OBSERVED certification binding. Holding every key is not
# sufficient — possession of a signature is not authority. If this ever starts
# reporting accepted=true, the certification gate has been lost.
cp "${workdir}/chain.good" "${workdir}/chain.accept"
append "${workdir}/chain.accept" release_acceptance "${evidence}/acceptance.txt" >/dev/null 2>&1
accept_line="$(verify "${workdir}/chain.accept" 2>&1)"; accept_rc=$?
echo "${accept_line}"
if [ "${accept_rc}" -eq 0 ] && [ "$(field "${accept_line}" accepted)" = "true" ]; then
  fail "the chain reported accepted=true using only the shipped tooling. Release acceptance is supposed to require an observed certification binding; that gate is GONE."
else
  echo "ceiling holds: acceptance refused without a certification binding"
fi

# ---------------------------------------------------------------------------
say "NEGATIVE CONTROL 1: rollback_rehearsal signed by the RELEASE-ACCEPTANCE key"
# ---------------------------------------------------------------------------
# A real, cryptographically valid signature under the wrong role. If the chain
# still verifies, role binding is decorative.
#
# WHY THE ACCEPTANCE KEY AND NOT THE PACKAGING KEY. The first version of this
# control used the packaging key, and it was refused — but for the WRONG REASON:
# `key id packaging-key signs more than one state in the same chain` fired
# first, because packaging had already signed record 0. That is a real rule and
# a real refusal, but it is not role binding, so the control was reporting a
# pass for a property it never reached. The release-acceptance key has signed
# nothing in this chain, so it isolates the role check with no other rule in
# front of it. Verified: the refusal message changed from the key-reuse rule to
# a role mismatch when the key changed.
chain_role="${workdir}/chain-wrong-role.json"
append "${chain_role}" packaging              "${evidence}/packaging.txt"  >/dev/null || fail "setup: packaging"
append "${chain_role}" deployment_preparation "${evidence}/deployment.txt" >/dev/null || fail "setup: deployment"
append "${chain_role}" rollback_rehearsal     "${evidence}/rehearsal.txt" release_acceptance >/dev/null 2>&1
role_line="$(verify "${chain_role}" 2>&1)"; role_rc=$?
if [ "${role_rc}" -eq 0 ] && [ "$(field "${role_line}" highest_state)" = "rollback_rehearsal" ]; then
  fail "a rollback_rehearsal record signed by the packaging key was ACCEPTED — role binding is not enforced"
else
  echo "refused as required: $(printf '%s' "${role_line}" | tail -1)"
fi

# ---------------------------------------------------------------------------
say "NEGATIVE CONTROL 2: skip deployment_preparation"
# ---------------------------------------------------------------------------
# Canonical order is a prefix match. Jumping straight from packaging to the
# rehearsal must not silently succeed, or the states are not separate.
chain_skip="${workdir}/chain-skip.json"
append "${chain_skip}" packaging          "${evidence}/packaging.txt" >/dev/null || fail "setup: packaging"
append "${chain_skip}" rollback_rehearsal "${evidence}/rehearsal.txt" >/dev/null 2>&1
skip_line="$(verify "${chain_skip}" 2>&1)"; skip_rc=$?
if [ "${skip_rc}" -eq 0 ] && [ "$(field "${skip_line}" highest_state)" = "rollback_rehearsal" ]; then
  fail "a chain that SKIPPED deployment_preparation reached rollback_rehearsal — the states are not ordered"
else
  echo "refused as required: $(printf '%s' "${skip_line}" | tail -1)"
fi

# ---------------------------------------------------------------------------
say "NEGATIVE CONTROL 3: the rehearsal record's evidence digest is altered"
# ---------------------------------------------------------------------------
chain_ev="${workdir}/chain-evidence.json"
cp "${workdir}/chain.good" "${chain_ev}"
python3 - "${chain_ev}" <<'PY'
import json, sys
path = sys.argv[1]
chain = json.load(open(path))
# Flip one hex nibble of the rehearsal record's evidence digest. Nothing else
# changes: same key, same role, same order, same signature bytes.
digest = chain[2]["body"]["evidence"][0]["sha256"]
chain[2]["body"]["evidence"][0]["sha256"] = ("1" if digest[0] != "1" else "2") + digest[1:]
json.dump(chain, open(path, "w"), indent=2)
PY
ev_line="$(verify "${chain_ev}" 2>&1)"; ev_rc=$?
if [ "${ev_rc}" -eq 0 ] && [ "$(field "${ev_line}" highest_state)" = "rollback_rehearsal" ]; then
  fail "the rehearsal record verified after its evidence digest was altered — the evidence is not bound"
else
  echo "refused as required: $(printf '%s' "${ev_line}" | tail -1)"
fi

# ---------------------------------------------------------------------------
say "RESTORE: the untouched chain must still verify"
# ---------------------------------------------------------------------------
# Without this, three refusals above are equally consistent with a verifier that
# had simply stopped working. Re-proving the positive control AFTER the negatives
# is what distinguishes "the mutations were caught" from "everything is refused".
restore_line="$(verify "${workdir}/chain.good")"; restore_rc=$?
echo "${restore_line}"
if [ "${restore_rc}" -ne 0 ] || [ "$(field "${restore_line}" highest_state)" != "rollback_rehearsal" ]; then
  fail "the untouched chain stopped verifying — the refusals above prove nothing"
fi

# ---------------------------------------------------------------------------
say "verdict"
# ---------------------------------------------------------------------------
if [ "${failures}" -ne 0 ]; then
  echo "STATE DRILL FAILED: ${failures} assertion(s)." >&2
  exit 1
fi
cat <<'SUMMARY'
STATE DRILL PASSED.
  rollback_rehearsal REACHED and verified (3 records, accepted=false)
  acceptance ceiling held without a certification binding
  3 collapse attempts refused: wrong-role key, skipped state, altered evidence
  untouched chain re-verified afterwards
SUMMARY
