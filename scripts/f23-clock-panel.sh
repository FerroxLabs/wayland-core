#!/usr/bin/env bash
# F23-05 Task 1 — the four-way cross-audit driver for the clock policy.
#
# Builds ONE evidence bundle from the probe's determination plus this plan's own
# option set, puts that SAME bundle to every panel member, captures each reply
# VERBATIM to its own file, and writes the run-time sha256 manifest that binds
# the decision record to those captures.
#
#   --probe-log <path>  the probe's captured output; its determination lines go
#                       into the bundle verbatim
#   --nonce     <hex>   the run nonce every member must echo
#   --out-dir   <dir>   where the bundle, the four captures and the manifest go
#   --stage     run     write the bundle and run the three external members
#               seal    validate all four captures and write the manifest
#
# MEASURED PANEL MECHANICS — each of these silently drops a vote if invoked
# wrong, which is the same defect class as a self-passing gate:
#   * gemini returns NOTHING without --skip-trust ("not running in a trusted
#     directory"). The "Both GOOGLE_API_KEY and GEMINI_API_KEY are set" line is
#     a notice on stderr, not an error.
#   * kimi must be invoked by ABSOLUTE path (a Bash-tool shell's PATH predates
#     the shell profile) and it BULLET-PREFIXES and indents its answer lines, so
#     every PANEL_* extraction must be UNANCHORED.
#   * codex prints hook lines and REPEATS its final block, so every extraction
#     must take the LAST match.
#
# The fourth member is the executor's own adversarial pass. It is NOT produced
# here: it must construct the strongest case AGAINST whatever the three external
# members most favour, which requires having read them. `--stage seal` refuses
# to write a manifest until that capture exists.
#
# Gate discipline: no check here is a pipeline into a filter.

set -uo pipefail

PROBE_LOG=""
NONCE=""
OUT_DIR=""
STAGE="run"

while [ $# -gt 0 ]; do
  case "$1" in
    --probe-log) PROBE_LOG="${2:-}"; shift 2 ;;
    --nonce)     NONCE="${2:-}";     shift 2 ;;
    --out-dir)   OUT_DIR="${2:-}";   shift 2 ;;
    --stage)     STAGE="${2:-}";     shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

[ -n "$NONCE" ]   || { echo "FATAL: --nonce is required" >&2; exit 64; }
[ -n "$OUT_DIR" ] || { echo "FATAL: --out-dir is required" >&2; exit 64; }
mkdir -p "$OUT_DIR"

BUNDLE="$OUT_DIR/23B-04-panel-bundle.md"
MANIFEST="$OUT_DIR/23B-04-panel-manifest.txt"
MEMBERS="codex gemini kimi internal"
OPTIONS="real-time-full real-time-linux-accelerated-elsewhere accelerated-except-absolute-deadline escalate"

KIMI="$HOME/.kimi-code/bin/kimi"

# ── Stage: run ───────────────────────────────────────────────────────────────
if [ "$STAGE" = "run" ]; then
  [ -n "$PROBE_LOG" ] || { echo "FATAL: --probe-log is required for --stage run" >&2; exit 64; }
  [ -f "$PROBE_LOG" ] || { echo "FATAL: $PROBE_LOG does not exist" >&2; exit 65; }

  {
    echo "panel_run_nonce=$NONCE"
    echo
    echo "# F23-05 clock-policy cross-audit bundle"
    echo
    echo "You are one of four independent members of a cross-audit panel. Answer the"
    echo "DECISION below on the EVIDENCE below. Do not ask questions; commit to one option."
    echo
    echo "## The decision"
    echo
    echo "Which legs of a multi-day wait/resume/complete journey for Wayland Core must run"
    echo "against REAL elapsed wall time, and which may legitimately use an accelerated"
    echo "clock?"
    echo
    echo "## Success Criterion 5, verbatim"
    echo
    echo "> A multi-day wait, resume and complete journey preserves cumulative authority,"
    echo "> resource, evidence, memory and delivery state, with exactly one loop owner at"
    echo "> every resume point."
    echo
    echo "## The LIVE determination, measured before this panel was convened"
    echo
    echo "These lines are the verbatim output of scripts/f23-clock-probe.sh, run on the"
    echo "authoritative Linux host against the binary built at the commit under test. The"
    echo "probe armed durable budget authority in one real process, let that process EXIT,"
    echo "elapsed a REAL gap during which no process existed, and bound the authority again"
    echo "in a SECOND real process."
    echo
    echo '```'
    grep -E '^F23_04_' "$PROBE_LOG"
    echo '```'
    echo
    echo "## The four options, verbatim from the plan"
    echo
    echo "### real-time-full"
    echo "Full real elapsed time — the journey spans at least three real calendar days on"
    echo "all three platforms with real process restarts and no clock manipulation anywhere."
    echo "PROS: the strongest possible evidence and the only one that proves a persisted"
    echo "deadline, a session store, a journal and an operating system all behaved across"
    echo "days of the process not existing; it also incidentally exercises the Windows box's"
    echo "real reboots, which is the environment a user actually has; no argument about what"
    echo "was simulated."
    echo "CONS: costs at least three calendar days of elapsed wall time before the phase can"
    echo "close, and occupies the Linux host, the Windows box and a Mac for that span; a"
    echo "defect found on day three costs another full cycle to re-prove."
    echo
    echo "### real-time-linux-accelerated-elsewhere"
    echo "Real elapsed time on the long-lived Linux host, accelerated clock on the macOS and"
    echo "Windows legs."
    echo "PROS: keeps the strongest evidence on the one host that genuinely stays up"
    echo "unattended for days, while the two attended machines finish in an afternoon; the"
    echo "accelerated legs still perform real process restarts and real persistence, so only"
    echo "the time span is simulated; total calendar cost is the same three days but only one"
    echo "host is occupied."
    echo "CONS: the macOS and Windows platform claims are weaker than the Linux one and must"
    echo "be labelled that way in the evidence rather than presented as equivalent; a"
    echo "platform-specific time or persistence defect could hide on exactly the two"
    echo "platforms that were accelerated."
    echo
    echo "### accelerated-except-absolute-deadline"
    echo "Accelerated clock everywhere except the absolute-deadline leg, which always runs"
    echo "against real elapsed time on every platform."
    echo "PROS: targets the cost precisely at the one authority whose meaning depends on real"
    echo "time passing while the process is dead, and leaves everything with no real-time"
    echo "dependency free to run fast; finishes in roughly one day of calendar."
    echo "CONS: depends entirely on the determination being correct about which behaviors"
    echo "have a genuine real-time dependency; if that determination is wrong, an accelerated"
    echo "leg silently proves nothing and the error is invisible in the evidence."
    echo
    echo "### escalate"
    echo "Escalate — none of the above buys evidence worth its cost right now, so record the"
    echo "decision as open and do not run the journey."
    echo "PROS: spends no calendar time on a proof whose shape is not yet agreed, and leaves"
    echo "the criterion visibly open rather than closed on evidence the owner does not accept."
    echo "CONS: Success Criterion 5 stays open, Phase 23B cannot close, and the criterion most"
    echo "likely to reveal a cross-restart defect goes unexercised."
    echo
    echo "## The three host facts"
    echo
    echo "* Linux (authoritative, and the only host that stays up unattended for days):"
    echo "  hetzner-dsm, /root/wayland. Full workspace aggregate is 11,519 tests in roughly"
    echo "  194 seconds."
    echo "* Windows (native leg): SeanDesktop, C:\\ferrox-win. The box reboots and is shared;"
    echo "  the journey surviving that is a feature of the test rather than a problem with it."
    echo "* macOS (native leg): a developer Mac. The phase's controlling execution instruction"
    echo "  forbids running Cargo on it, and the binary resolver the plan expected"
    echo "  (scripts/f23-macos-binary.sh) was never landed by its owning plan."
    echo
    echo "## What to weigh explicitly"
    echo
    echo "An accelerated leg makes a platform's span assertion TRIVIALLY satisfiable, which is"
    echo "exactly why an accelerated leg is a WEAKER claim and must be labelled as one rather"
    echo "than presented as equivalent evidence. Weigh that against three calendar days."
    echo
    echo "Consider carefully whether the measured determination REMOVES any option outright."
    echo "An option that rests on a mechanism the product does not expose is not a cheaper"
    echo "option; it is an argument about a thing that does not exist."
    echo
    echo "## Reply contract — the last three lines of your reply MUST be exactly:"
    echo
    echo "PANEL_NONCE=$NONCE"
    echo "PANEL_POSITION=<one of: $OPTIONS>"
    echo "PANEL_RATIONALE=<one single line, at least forty characters, no newlines>"
  } > "$BUNDLE"

  echo "PANEL_BUNDLE_WRITTEN=$BUNDLE bytes=$(wc -c < "$BUNDLE" | tr -d ' ')"

  PROMPT=$(cat "$BUNDLE")

  echo "PANEL_MEMBER_START=codex"
  codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "$PROMPT" \
    > "$OUT_DIR/23B-04-panel-codex.txt" 2>&1
  echo "PANEL_MEMBER_DONE=codex rc=$?"

  echo "PANEL_MEMBER_START=gemini"
  gemini -p "$PROMPT" -m gemini-3.1-pro-preview -o text --skip-trust \
    > "$OUT_DIR/23B-04-panel-gemini.txt" 2>&1
  echo "PANEL_MEMBER_DONE=gemini rc=$?"

  echo "PANEL_MEMBER_START=kimi"
  "$KIMI" -p "$PROMPT" --output-format text \
    > "$OUT_DIR/23B-04-panel-kimi.txt" 2>&1
  echo "PANEL_MEMBER_DONE=kimi rc=$?"

  echo "PANEL_STAGE=run-complete; write $OUT_DIR/23B-04-panel-internal.txt then --stage seal"
  exit 0
fi

# ── Stage: seal ──────────────────────────────────────────────────────────────
if [ "$STAGE" != "seal" ]; then
  echo "FATAL: --stage must be run or seal" >&2
  exit 64
fi

[ -f "$BUNDLE" ] || { echo "FATAL: $BUNDLE does not exist; run --stage run first" >&2; exit 65; }

BUNDLE_NONCE=$(grep -oE 'panel_run_nonce=[0-9a-f]{16}' "$BUNDLE" | tail -1 | cut -d= -f2)
if [ "$BUNDLE_NONCE" != "$NONCE" ]; then
  echo "FATAL: bundle nonce '$BUNDLE_NONCE' != --nonce '$NONCE'" >&2
  exit 66
fi

FAILURES=0
for M in $MEMBERS; do
  F="$OUT_DIR/23B-04-panel-$M.txt"
  if [ ! -s "$F" ]; then
    echo "PANEL-FAIL: $M produced no output ($F)" >&2
    FAILURES=$((FAILURES + 1))
    continue
  fi
  # UNANCHORED, because kimi bullet-prefixes and indents its answer lines.
  if ! grep -qF "PANEL_NONCE=$NONCE" "$F"; then
    echo "PANEL-FAIL: $M omitted the run nonce" >&2
    FAILURES=$((FAILURES + 1))
  fi
  # LAST match, because codex repeats its final block.
  POS=$(grep -oE 'PANEL_POSITION=[a-z-]+' "$F" | tail -1 | cut -d= -f2)
  OK=0
  for O in $OPTIONS; do [ "$POS" = "$O" ] && OK=1; done
  if [ "$OK" -ne 1 ]; then
    echo "PANEL-FAIL: $M named a position outside the option set: '${POS:-<none>}'" >&2
    FAILURES=$((FAILURES + 1))
  fi
  if ! grep -qE 'PANEL_RATIONALE=.{40}' "$F"; then
    echo "PANEL-FAIL: $M gave no rationale of at least forty characters" >&2
    FAILURES=$((FAILURES + 1))
  fi
  echo "PANEL_POSITION_RECORDED=$M option=${POS:-none}"
done

if [ "$FAILURES" -ne 0 ]; then
  echo "PANEL=INCOMPLETE failures=$FAILURES" >&2
  exit 1
fi

# Exactly five lines, named explicitly rather than globbed, so the manifest can
# never list itself. Written at RUN TIME, so a capture edited afterwards fails
# `shasum -c`.
( cd "$OUT_DIR" && shasum -a 256 \
    23B-04-panel-bundle.md \
    23B-04-panel-codex.txt \
    23B-04-panel-gemini.txt \
    23B-04-panel-kimi.txt \
    23B-04-panel-internal.txt ) > "$MANIFEST"
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: could not write the manifest (shasum exited $rc)" >&2
  exit 67
fi

( cd "$OUT_DIR" && shasum -a 256 -c 23B-04-panel-manifest.txt )
rc=$?
if [ "$rc" -ne 0 ]; then
  echo "FATAL: the manifest does not verify against its own captures" >&2
  exit 68
fi

echo "PANEL=SEALED nonce=$NONCE manifest=$MANIFEST"
exit 0
