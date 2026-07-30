#!/usr/bin/env python3
"""Prototype: enumerate ProtocolEvent wire types by parsing events.rs, then diff
against the Desktop contract inventory. Ported to Rust as the C2a gate."""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
SRC = (ROOT / "crates/wcore-protocol/src/events.rs").read_text()
C = ROOT / "crates/wcore-protocol/contracts/desktop/v1"

out = []


def p(s=""):
    out.append(str(s))


def snake(name):
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", name)
    return s.lower()


anchor = SRC.index("pub enum ProtocolEvent {")
i = SRC.index("{", anchor) + 1
depth = 0
variants = []
pending_rename = None
line_start = i
buf = []
n = len(SRC)
while i < n:
    ch = SRC[i]
    if depth == 0 and ch == "}":
        break
    if ch in "{([":
        depth += 1
        i += 1
        continue
    if ch in "})]":
        depth -= 1
        i += 1
        continue
    if depth == 0 and ch == "\n":
        line = "".join(buf).strip()
        buf = []
        m = re.match(r'^#\[serde\(rename\s*=\s*"([^"]+)"', line)
        if m:
            pending_rename = m.group(1)
        else:
            m2 = re.match(r"^([A-Z][A-Za-z0-9]*)\s*$", line)
            if m2:
                variants.append((m2.group(1), pending_rename or snake(m2.group(1))))
                pending_rename = None
            elif line and not line.startswith(("//", "#[", "///")):
                pass
        i += 1
        continue
    if depth == 0:
        buf.append(ch)
    i += 1

p(f"parsed ProtocolEvent variants (depth-0 bare idents): {len(variants)}")
p("NOTE: struct-form variants end with '{' which increments depth, so the bare-ident")
p("      match above is insufficient. See second pass below.")

# Second pass: token-level. A variant name is an identifier at depth 0 that is
# immediately followed (after whitespace/comments) by '{', '(' or ','.
i = SRC.index("{", anchor) + 1
depth = 0
variants = []
pending_rename = None
while i < n:
    ch = SRC[i]
    if depth == 0 and ch == "}":
        break
    if ch == "#" and depth == 0:
        # attribute: consume balanced [...]
        j = SRC.index("[", i)
        d = 0
        while j < n:
            if SRC[j] == "[":
                d += 1
            elif SRC[j] == "]":
                d -= 1
                if d == 0:
                    break
            j += 1
        attr = SRC[i : j + 1]
        m = re.search(r'rename\s*=\s*"([^"]+)"', attr)
        if m and "rename_all" not in attr:
            pending_rename = m.group(1)
        i = j + 1
        continue
    if depth == 0 and SRC.startswith("///", i) or (depth == 0 and SRC.startswith("//", i)):
        i = SRC.index("\n", i) + 1
        continue
    if depth == 0 and (ch.isalpha() or ch == "_"):
        j = i
        while j < n and (SRC[j].isalnum() or SRC[j] == "_"):
            j += 1
        ident = SRC[i:j]
        k = j
        while k < n and SRC[k] in " \t\r\n":
            k += 1
        if k < n and SRC[k] in "{(,":
            variants.append((ident, pending_rename or snake(ident)))
            pending_rename = None
        i = j
        continue
    if ch in "{([":
        depth += 1
    elif ch in "})]":
        depth -= 1
    i += 1

p()
p(f"ProtocolEvent variants (token pass): {len(variants)}")
wire = [w for _, w in variants]
dupes = sorted({w for w in wire if wire.count(w) > 1})
p(f"  duplicate wire types among variants: {dupes}")

man = json.loads((C / "manifest.json").read_text())
desktop = {e["type"] for e in man["events"]}
deferred = (C / "DEFERRED.md").read_text()

core = json.loads((C / "schema/core-event.schema.json").read_text())
core_names = set()
for b in core["oneOf"]:
    t = b.get("properties", {}).get("type", {})
    if "const" in t:
        core_names.add(t["const"])
    core_names.update(t.get("enum") or [])

prod = json.loads((C / "schema/producer-complete.schema.json").read_text())
prod_names = set()
for b in prod["anyOf"]:
    t = b.get("properties", {}).get("type", {})
    if "const" in t:
        prod_names.add(t["const"])
    prod_names.update(t.get("enum") or [])

p(f"  manifest desktop events: {len(desktop)}")
p(f"  core-event.schema discriminators: {len(core_names)}")
p(f"  producer-complete discriminators: {len(prod_names)}")

unaccounted = []
for name, w in variants:
    in_desktop = w in desktop
    in_prod = w in prod_names
    in_deferred = w in deferred
    if not (in_desktop or in_prod or in_deferred):
        unaccounted.append((name, w))
p()
p(f"  variants in NEITHER manifest NOR producer-complete NOR DEFERRED.md: {len(unaccounted)}")
for name, w in unaccounted:
    p(f"    {name} -> {w}")

p()
p("  variants in producer-complete but NOT in manifest and NOT in DEFERRED.md")
p("  (the C4 class -- declared as inventory, justified nowhere):")
c4 = [(nm, w) for nm, w in variants if w in prod_names and w not in desktop and w not in deferred]
p(f"    count={len(c4)}")
for name, w in c4:
    p(f"    {name} -> {w}")

p()
p("  manifest events with NO matching ProtocolEvent variant (reverse direction):")
orphan = sorted(desktop - set(wire))
p(f"    count={len(orphan)}: {orphan}")

p()
p("  CONTROL known-positives (instrument liveness):")
p(f"    'ready' in variants: {'ready' in wire}")
p(f"    'execution_policy' in variants: {'execution_policy' in wire}")
p(f"    'workspace_policy' in variants: {'workspace_policy' in wire}")
p(f"    'this_variant_does_not_exist' in variants: {'this_variant_does_not_exist' in wire}")

p()
p("  DEFERRED.md verbatim:")
for line in deferred.splitlines():
    p(f"    | {line}")

sys.stdout.write("\n".join(out) + "\n")
