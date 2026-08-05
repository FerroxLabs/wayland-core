#!/usr/bin/env python3
"""Both-directions prover for the f14 degraded-posture pair + crash matrix.

Each mutation removes ONE production behaviour the tests exist to protect.
Aborts if the target text is not present exactly once, so a mutation that
silently did nothing can never be reported as "the gate held".
"""
import sys

ROOT = "/root/wayland-durable-posture/"
ENGINE = ROOT + "crates/wcore-agent/src/engine.rs"
CONFIG = ROOT + "crates/wcore-config/src/config.rs"

# F1 -- stop announcing the degrade per turn. Only the per-turn assertion in the
# degrade-ALLOWED test should notice.
F1 = (
    ENGINE,
    "            self.announce_host_forced_degrade_for_this_turn();\n",
    "",
)

# F2 -- stop degrading at all, so a keyring-less host keeps session.enabled and
# journals. The residue assertions in BOTH the ALLOWED test and the crash matrix
# should notice.
F2 = (
    CONFIG,
    """            HostDurabilityDisposition::Degrade => {
                resolved.session.enabled = false;
                record_durable_sessions_disabled_by_host();
            }""",
    """            HostDurabilityDisposition::Degrade => {
                record_durable_sessions_disabled_by_host();
            }""",
)

# F3 -- ignore the operator's require_durability, so the refusal becomes a
# degrade. Only the degrade-FORBIDDEN test should notice.
F3 = (
    CONFIG,
    """            HostDurabilityDisposition::Refuse => anyhow::bail!("{}", DURABILITY_REQUIRED_REFUSAL),""",
    """            HostDurabilityDisposition::Refuse => {
                resolved.session.enabled = false;
                record_durable_sessions_disabled_by_host();
            }""",
)

MUTATIONS = {"F1": F1, "F2": F2, "F3": F3}

name = sys.argv[1]
path, src, dst = MUTATIONS[name]
text = open(path, encoding="utf-8").read()
count = text.count(src)
if count != 1:
    print(f"MUTATION_{name}_TARGET_COUNT={count} (expected 1) -- ABORT")
    sys.exit(2)
open(path, "w", encoding="utf-8").write(text.replace(src, dst, 1))
print(f"MUTATION_{name}_APPLIED to {path}")
