#!/usr/bin/env bash
# LANE-BRIEF 3.2 + 3b-iii: prove each new gate can FAIL, not merely that it passes.
# Each mutation is applied, the gate is run, then the tree is restored with
# `git checkout -- <path>` (a single named path in our own worktree; permitted).
set -u
export PATH=/root/.cargo/bin:$PATH
export CARGO_BUILD_JOBS=10
cd /root/wayland-cont-skills-cache || exit 1

RR=crates/wcore-agent/src/resilient_reporter.rs
BS=crates/wcore-agent/src/bootstrap.rs

run() { cargo test -p wcore-agent --lib "$1" 2>&1 | /usr/bin/grep -E '^test result|^test .*(ok|FAILED)'; }

echo "############ BASELINE (all three must pass) ############"
run cooldown_occurrence_fires_on_open
run the_open_occurrence_names_only
run pricing_refresher_construction_is_recorded

echo
echo "############ MUTATION 1: drop the Open guard (emit on every transition) ############"
/usr/bin/sed -i 's/if matches!(state, CircuitState::Open) {/if true {/' "$RR"
/usr/bin/grep -c 'if true {' "$RR"
run cooldown_occurrence_fires_on_open
/usr/bin/git checkout -- "$RR"
echo "restored: $(/usr/bin/grep -c 'matches!(state, CircuitState::Open)' "$RR") guard(s) back"

echo
echo "############ MUTATION 2: name the wrong capability ############"
/usr/bin/sed -i 's/wcore_protocol::events::CapabilityId::CooldownTracker,/wcore_protocol::events::CapabilityId::SmartHandoff,/' "$RR"
/usr/bin/grep -c 'CapabilityId::SmartHandoff' "$RR"
run the_open_occurrence_names_only
/usr/bin/git checkout -- "$RR"
echo "restored: $(/usr/bin/grep -c 'CapabilityId::CooldownTracker' "$RR") cooldown ref(s) back"

echo
echo "############ MUTATION 3: claim construction on the early-return path ############"
/usr/bin/sed -i 's/    if !config.provider_chain.enabled {\n        return Ok(Vec::new());/XXX/' "$BS"
# sed cannot span lines; use perl-free two-step: set the flag true at fn entry.
/usr/bin/sed -i 's|^    pricing_refresher_constructed: \&mut bool,$|    pricing_refresher_constructed: \&mut bool,\n    // MUTATION|' "$BS"
/usr/bin/sed -i 's|^) -> anyhow::Result<Vec<(FailoverCandidateMetadata, Arc<dyn LlmProvider>)>> {$|) -> anyhow::Result<Vec<(FailoverCandidateMetadata, Arc<dyn LlmProvider>)>> {\n    *pricing_refresher_constructed = true;|' "$BS"
/usr/bin/grep -c '^    \*pricing_refresher_constructed = true;$' "$BS"
run pricing_refresher_construction_is_recorded
/usr/bin/git checkout -- "$BS"
echo "restored"

echo
echo "############ POST-RESTORE (all three must pass again) ############"
run cooldown_occurrence_fires_on_open
run the_open_occurrence_names_only
run pricing_refresher_construction_is_recorded
echo "=== END ==="
