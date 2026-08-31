#!/usr/bin/env bash
# ADMISSION: unconditional -- a supply-chain drill that only runs when the
# rest of the job is healthy proves nothing about the runs that matter.
#
# End-to-end drill for the signed release manifest (SR-29-9 / SR-29-11).
#
# Mints a corpus with the REAL `wayland-release` binary — using the exact
# `manifest-build` / `manifest-sign` command pair `.github/workflows/release.yml`
# runs, seed on STDIN — then drives it through the REAL shipped `ReleaseVerifier`
# and `decide_update` in `crates/wcore-cli/tests/release_manifest_pipeline.rs`.
#
# WHY A DRIVER RATHER THAN A PLAIN `cargo test`. The minting side lives in
# `wcore-eval-scenarios` and the verifying side in `wcore-cli`, which sits ABOVE
# it in the dependency graph. A single cargo test cannot own both without an
# upward edge, so the corpus is produced out of process and handed over on disk.
#
# NO CREDENTIAL. Every key here is generated at run time by
# `wayland-release trust-root-init` into a temporary directory this script
# deletes on exit. The production seed is never read, and there is no code path
# in this file that could accept one.
#
# ANTI-VACUITY. `cargo test` exits 0 when a filter matches nothing and when
# every test is `#[ignore]`d, so exit status is NOT trusted: the executed pass
# count is parsed out of the summary line and compared against EXPECTED_TESTS.
# Cargo is invoked by absolute path where one is resolvable, because the `rtk`
# proxy re-renders cargo output and strips the `ignored` / `filtered out` fields
# this check depends on.

set -euo pipefail

EXPECTED_TESTS=10

RUNNING_VERSION="0.12.25"
NEWER_VERSION="0.12.26"
OLDER_VERSION="0.12.24"
CONTROL_SEQUENCE=41
NOW="$(date -u +%s)"
# Comfortably past DEFAULT_MAX_MANIFEST_AGE_SECS (90 days).
OVERAGE_ISSUED_AT="$((NOW - 120 * 24 * 60 * 60))"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"

CARGO="$(command -v cargo || true)"
if [ -x "${HOME}/.cargo/bin/cargo" ]; then CARGO="${HOME}/.cargo/bin/cargo"; fi
if [ -z "${CARGO}" ]; then echo "cargo not found on PATH" >&2; exit 1; fi

workdir="$(mktemp -d)"
cleanup() { rm -rf "${workdir}"; }
trap cleanup EXIT

keys="${workdir}/keys"
corpus="${workdir}/corpus"
artifacts="${corpus}/artifacts"
mkdir -p "${corpus}" "${artifacts}"

say() { printf '\n=== %s ===\n' "$1"; }

# ---------------------------------------------------------------------------
say "build the real wayland-release binary"
# ---------------------------------------------------------------------------
"${CARGO}" build --release -p wcore-eval-scenarios --bin wayland-release
tool="${repo_root}/target/release/wayland-release"
test -x "${tool}"

# ---------------------------------------------------------------------------
say "mint throwaway keys (trust-root-init writes seeds 0600, prints public only)"
# ---------------------------------------------------------------------------
"${tool}" trust-root-init --directory "${keys}" --valid-from 0
cp "${keys}/trust-root.json" "${corpus}/trust-root.json"

acceptance_seed="${keys}/release-acceptance-key.seed"
packaging_seed="${keys}/packaging-key.seed"
test -s "${acceptance_seed}"
test -s "${packaging_seed}"

# A second root in which the acceptance key has been RETIRED, through the real
# rotation command. Retirement is a recorded fact, not a deletion: the
# signatures stay cryptographically valid and the root stops honouring them.
cp "${keys}/trust-root.json" "${corpus}/trust-root-retired.json"
"${tool}" trust-root-retire-key \
  --trust-root "${corpus}/trust-root-retired.json" \
  --key-id release-acceptance-key \
  --retired-at "$((NOW - 3600))"

# ---------------------------------------------------------------------------
say "package fake artifacts (content is irrelevant; the DIGESTS are the point)"
# ---------------------------------------------------------------------------
archive="wayland-core-v${NEWER_VERSION}-x86_64-unknown-linux-gnu.tar.gz"
head -c 4096 /dev/urandom > "${artifacts}/${archive}"
( cd "${artifacts}" && sha256sum wayland-core-* > wayland-core-checksums.txt )

# ---------------------------------------------------------------------------
# One case per refusal the updater must make, plus the pristine control.
# `mint <case> <seed> <sequence> <issued-at> <release-id> [extra manifest-build args...]`
# ---------------------------------------------------------------------------
mint() {
  local case_name="$1" seed_file="$2" sequence="$3" issued_at="$4" release_id="$5"
  shift 5
  local unsigned="${workdir}/${case_name}-unsigned.json"
  local key_id
  case "${seed_file}" in
    *release-acceptance-key.seed) key_id="release-acceptance-key" ;;
    *packaging-key.seed) key_id="packaging-key" ;;
    *) echo "unknown seed file ${seed_file}" >&2; return 1 ;;
  esac

  "${tool}" manifest-build \
    --artifacts "${artifacts}" \
    --output "${unsigned}" \
    --release-id "${release_id}" \
    --source-commit "$(printf 'a%.0s' {1..40})" \
    --sequence "${sequence}" \
    --issued-at "${issued_at}" \
    "$@"

  # STDIN ONLY — never argv. `set -x` is deliberately not enabled in this
  # script; a traced pipeline would still not expose the seed, but the habit is
  # the point.
  "${tool}" manifest-sign \
    --manifest "${unsigned}" \
    --output "${corpus}/${case_name}-release-manifest.json" \
    --key-id "${key_id}" < "${seed_file}"
}

say "mint the corpus"
# The PRISTINE CONTROL. Every refusal test re-proves this one first.
mint accepted "${acceptance_seed}" "${CONTROL_SEQUENCE}" "$((NOW - 60))" "v${NEWER_VERSION}-wayland-base"
# A genuine OLDER release: correctly signed, correctly attested, still refused.
mint rollback "${acceptance_seed}" "$((CONTROL_SEQUENCE - 1))" "$((NOW - 60))" "v${OLDER_VERSION}-wayland-base"
# A correctly signed but FROZEN view.
mint overage "${acceptance_seed}" "${CONTROL_SEQUENCE}" "${OVERAGE_ISSUED_AT}" "v${NEWER_VERSION}-wayland-base"
# A version withdrawn after signing.
mint revoked "${acceptance_seed}" "${CONTROL_SEQUENCE}" "$((NOW - 60))" "v${NEWER_VERSION}-wayland-base" \
  --revoke-version "${NEWER_VERSION}=withdrawn by the drill"
# Reaching packaging is not reaching acceptance.
mint packaging-signed "${packaging_seed}" "${CONTROL_SEQUENCE}" "$((NOW - 60))" "v${NEWER_VERSION}-wayland-base"

# A body edited AFTER signing. Everything else — key, role, sequence, signature
# bytes — stays exactly as the real signer produced it.
python3 - "${corpus}/accepted-release-manifest.json" "${corpus}/tampered-release-manifest.json" <<'PY'
import json, sys
source, target = sys.argv[1], sys.argv[2]
document = json.load(open(source))
document["body"]["release_id"] = document["body"]["release_id"].replace("0.12.26", "9.9.9")
json.dump(document, open(target, "w"), indent=2)
print("tampered: release_id edited after signing, signature untouched")
PY

cat > "${corpus}/drill-parameters.json" <<JSON
{
  "now_unix": ${NOW},
  "running_version": "${RUNNING_VERSION}",
  "newer_version": "${NEWER_VERSION}",
  "older_version": "${OLDER_VERSION}",
  "control_sequence": ${CONTROL_SEQUENCE}
}
JSON

say "corpus"
ls -1 "${corpus}"

# ---------------------------------------------------------------------------
say "SELF-CHECK: the signed control verifies through the real wayland-release too"
# ---------------------------------------------------------------------------
# A known-positive for the whole minting path before the Rust side runs. If this
# fails, the corpus is bad and every refusal below would pass for the wrong
# reason.
"${tool}" manifest-verify \
  --manifest "${corpus}/accepted-release-manifest.json" \
  --trust-root "${corpus}/trust-root.json" \
  --role release_acceptance \
  --now "${NOW}"

# And the matching known-negative: the retired root must REFUSE the same bytes.
if "${tool}" manifest-verify \
    --manifest "${corpus}/accepted-release-manifest.json" \
    --trust-root "${corpus}/trust-root-retired.json" \
    --role release_acceptance \
    --now "${NOW}" > "${workdir}/retired.out" 2>&1; then
  echo "INSTRUMENT DEAD: the retired trust root accepted a manifest it must refuse" >&2
  exit 1
fi
echo "retired root refused as required: $(tail -1 "${workdir}/retired.out")"

# ---------------------------------------------------------------------------
say "drive the corpus through the SHIPPED verifier"
# ---------------------------------------------------------------------------
summary="${workdir}/cargo-test.out"
set +e
WAYLAND_RELEASE_PIPELINE_CORPUS="${corpus}" \
  "${CARGO}" test -p wcore-cli --test release_manifest_pipeline -- --ignored \
  2>&1 | tee "${summary}"
cargo_status="${PIPESTATUS[0]}"
set -e

# ---------------------------------------------------------------------------
say "anti-vacuity"
# ---------------------------------------------------------------------------
# `cargo test` exits 0 for a filter that matched nothing and for a suite whose
# tests are all ignored. So the COUNT is the gate, not the status.
# `/usr/bin/grep` throughout: the `rtk` proxy re-renders grep and cargo output,
# and cargo's rewriting strips the very `ignored` / `filtered out` fields this
# block reads back.
result_line="$(/usr/bin/grep -E '^test result:' "${summary}" | tail -1 || true)"
if [ -z "${result_line}" ]; then
  echo "FAIL: no 'test result:' line — the suite did not run at all" >&2
  exit 1
fi
echo "${result_line}"

# Field-anchored extraction, not a greedy sed: `.*([0-9]+) failed` captures the
# LAST digit of a two-digit count, so a run with 10 failures would read as 0.
field() { printf '%s' "${result_line}" | /usr/bin/grep -oE "[0-9]+ $1" | /usr/bin/grep -oE '[0-9]+' | head -1; }
passed="$(field passed)"; passed="${passed:-0}"
failed="$(field failed)"; failed="${failed:-1}"
ignored="$(field ignored)"; ignored="${ignored:-0}"
filtered="$(printf '%s' "${result_line}" | /usr/bin/grep -oE '[0-9]+ filtered out' | /usr/bin/grep -oE '[0-9]+' | head -1)"
filtered="${filtered:-0}"

if [ "${cargo_status}" -ne 0 ] || [ "${failed}" != "0" ]; then
  echo "FAIL: the drill is RED (status=${cargo_status}, failed=${failed})" >&2
  exit 1
fi
# Three flavours of a suite that exits 0 having run nothing, all measured on
# this repository: every test `#[ignore]`d, an env-gated early return, and a
# filter that matches no test name. The first and third are visible here; the
# second is closed in the test file, which PANICS on a missing corpus.
if [ "${ignored}" != "0" ] || [ "${filtered}" != "0" ]; then
  echo "FAIL: ${ignored} ignored and ${filtered} filtered out — the drill did not run whole." >&2
  exit 1
fi
if [ "${passed}" -ne "${EXPECTED_TESTS}" ]; then
  echo "FAIL: expected ${EXPECTED_TESTS} executed tests, the suite reported ${passed}." >&2
  echo "A count below the expectation means tests were filtered out, ignored or removed —" >&2
  echo "which is exactly how a green run covers nothing. Update EXPECTED_TESTS only when" >&2
  echo "the drill genuinely gains or loses a case." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
say "NEGATIVE CONTROL: the drill must be able to go RED"
# ---------------------------------------------------------------------------
# Ten green tests prove nothing until the harness is shown to discriminate. A
# corpus a verifier accepts unconditionally, a `decide_update` that proceeds on
# everything, or a corpus loader that silently returns the same file for every
# name would ALL produce the run above. So: swap the pristine control for the
# packaging-signed manifest — a real, valid signature under the wrong role — and
# require the suite to fail. The control is what every test re-proves first, so
# breaking it must break the whole drill.
cp "${corpus}/accepted-release-manifest.json" "${workdir}/accepted.keep"
cp "${corpus}/packaging-signed-release-manifest.json" "${corpus}/accepted-release-manifest.json"

set +e
WAYLAND_RELEASE_PIPELINE_CORPUS="${corpus}" \
  "${CARGO}" test -p wcore-cli --test release_manifest_pipeline -- --ignored \
  > "${workdir}/negative.out" 2>&1
negative_status="$?"
set -e
cp "${workdir}/accepted.keep" "${corpus}/accepted-release-manifest.json"

negative_line="$(/usr/bin/grep -E '^test result:' "${workdir}/negative.out" | tail -1 || true)"
negative_failed="$(printf '%s' "${negative_line}" | /usr/bin/grep -oE '[0-9]+ failed' | /usr/bin/grep -oE '[0-9]+' | head -1)"
negative_failed="${negative_failed:-0}"
echo "${negative_line:-<no result line>}"

if [ "${negative_status}" -eq 0 ] || [ "${negative_failed}" -eq 0 ]; then
  echo "INSTRUMENT DEAD: the drill stayed green with a wrong-role manifest as its control." >&2
  echo "Every PASS above is therefore worthless. Fix the harness before trusting any of it." >&2
  exit 1
fi
echo "negative control refused as required: ${negative_failed} test(s) failed on a broken corpus"

printf '\nDRILL PASSED: %s tests executed against a CLI-minted corpus,\n' "${passed}"
printf 'and %s failed when the corpus was deliberately broken.\n' "${negative_failed}"
