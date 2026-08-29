p = "crates/wcore-cli/src/doctor/mod.rs"
s = open(p).read()

old = """            // wayland-core#354 — the launch gate's posture, printed whether or
            // not any server is declared: a fresh config with no servers yet
            // is exactly when an operator wants to see which mode they are on.
            println!("{}", malware_gate_line(cfg.mcp.malware_gate));"""
new = """            // wayland-core#354 c7 — install the operator's chosen mode into
            // this process BEFORE anything below can launch a server.
            //
            // `--doctor` returns at `main.rs` ahead of config/OAuth/engine
            // bootstrap, and `AgentBootstrap::build` is the only OTHER caller
            // of `install_mode`. Without this line the probe further down
            // reaches `StdioTransport::spawn` with the mode uninstalled and
            // silently takes the permissive default — so under strict, the one
            // command an operator runs to ASK whether the gate is on would be
            // the command that does not honour it. A mode the diagnostic path
            // ignores is not an operator choice.
            //
            // Installed at the point the mode is READ for display, so the
            // posture printed and the posture enforced cannot drift.
            // `install_mode` is one-shot and idempotent, so this cannot fight
            // a later boot; nothing in this process boots after `--doctor`.
            wcore_mcp::malware_gate::install_mode(cfg.mcp.malware_gate);
            // wayland-core#354 — the launch gate's posture, printed whether or
            // not any server is declared: a fresh config with no servers yet
            // is exactly when an operator wants to see which mode they are on.
            println!("{}", malware_gate_line(cfg.mcp.malware_gate));"""
assert old in s, "354 anchor miss"
s = s.replace(old, new, 1)
open(p, "w").write(s)
print("354 patched")
