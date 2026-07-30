#!/usr/bin/env python3
"""Both-directions prover: apply ONE mutation to config.rs, print a marker.

Usage: mutate.py <M1|M2|M3>
Exits non-zero if the target text is not found, so a mutation that silently
did nothing can never be reported as "the gate held".
"""
import sys

PATH = "/root/wayland-durable-posture/crates/wcore-config/src/config.rs"

M1_FROM = """    if require_durability {
        HostDurabilityDisposition::Refuse
    } else {
        HostDurabilityDisposition::Degrade
    }
}"""
M1_TO = """    let _ = require_durability;
    HostDurabilityDisposition::Degrade
}"""

M2_FROM = """    let session = if project.session.directory != default_session_dir() {
        SessionConfig {
            require_durability,
            ..project.session
        }
    } else {"""
M2_TO = """    let session = if project.session.directory != default_session_dir() {
        let _ = require_durability;
        project.session
    } else {"""

M3_FROM = """     has one. To accept running without durable sessions on this host, set \\
     [session] require_durability = false.\";"""
M3_TO = """     has one.\";"""

MUTATIONS = {"M1": (M1_FROM, M1_TO), "M2": (M2_FROM, M2_TO), "M3": (M3_FROM, M3_TO)}

name = sys.argv[1]
src, dst = MUTATIONS[name]
text = open(PATH, encoding="utf-8").read()
count = text.count(src)
if count != 1:
    print(f"MUTATION_{name}_TARGET_COUNT={count} (expected 1) -- ABORT")
    sys.exit(2)
open(PATH, "w", encoding="utf-8").write(text.replace(src, dst, 1))
print(f"MUTATION_{name}_APPLIED")
