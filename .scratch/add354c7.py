import io
p = ".planning/ledger/wayland-core-354.md"
s = io.open(p, encoding="utf-8").read()
anchor = """    note: "docs/mcp.md carries no malware_gate section in the graded tree, and /doctor reports no gate mode."
---"""
new = """    note: "docs/mcp.md carries no malware_gate section in the graded tree, and /doctor reports no gate mode."
  - id: c7
    text: "The already-shipping non-session MCP launch path reads the operator's chosen mode, not the uninstalled permissive default"
    state: not-met
    owner: core
    note: "Added 2026-08-29 by the adversarial verifier of lane/f13-mcp-gate-mode, which REFUTED this entry's own disclosure. c1-c6 hold on their literal text, but the note claiming the uninstalled default only affects a hypothetical caller that `should be revisited if a non-session MCP launch path ever appears` is false: such a path already ships. `wayland --doctor --probe-mcp` returns at main.rs:1819 into doctor::run BEFORE config/OAuth/engine bootstrap, so it reaches StdioTransport::spawn without an installed mode and silently takes permissive. Under strict, the one command an operator runs to ASK whether the gate is on is the command that does not honour it. A mode that the diagnostic path ignores is not an operator choice."
---"""
assert anchor in s, "anchor not found"
io.open(p, "w", encoding="utf-8").write(s.replace(anchor, new, 1))
print("added c7 to core#354")
