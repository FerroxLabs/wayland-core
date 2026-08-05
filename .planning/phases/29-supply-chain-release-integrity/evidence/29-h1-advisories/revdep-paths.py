#!/usr/bin/env python3
"""Enumerate ALL paths from a target crate up to workspace roots, straight out of a
Cargo.lock. Independent of `cargo tree` (which the prior lane used) so this is a
genuinely separate instrument, not a re-run of theirs."""
import sys, re, collections

lock_path, target = sys.argv[1], sys.argv[2]
text = open(lock_path, encoding="utf-8").read()

# Parse [[package]] stanzas without a toml lib (stdlib tomllib exists in 3.11+, but
# parse manually so this runs anywhere and so the parse is auditable).
pkgs = []
for stanza in text.split("[[package]]")[1:]:
    name = re.search(r'^name = "([^"]+)"', stanza, re.M)
    ver = re.search(r'^version = "([^"]+)"', stanza, re.M)
    if not name or not ver:
        continue
    deps = []
    m = re.search(r"^dependencies = \[(.*?)^\]", stanza, re.M | re.S)
    if m:
        deps = re.findall(r'"([^"]+)"', m.group(1))
    pkgs.append({"name": name.group(1), "version": ver.group(1), "deps": deps})

by_name = collections.defaultdict(list)
for p in pkgs:
    by_name[p["name"]].append(p)

print(f"lockfile: {lock_path}")
print(f"total [[package]] stanzas parsed: {len(pkgs)}")

# SELF-TEST of the parser (a parser that returns nothing makes every absence claim true).
assert len(pkgs) > 500, f"parser produced only {len(pkgs)} packages — instrument dead"
assert by_name.get("serde"), "parser found no serde — instrument dead"
print(f"parser self-test: serde found ({len(by_name['serde'])} version(s)) -> instrument ALIVE")

tv = by_name.get(target, [])
print(f"\n=== resolved versions of {target}: {len(tv)} ===")
for p in tv:
    print(f"  {target} {p['version']}")

# A dep entry is "name" or "name version" or "name version (source)".
def dep_matches(dep_str, name, version):
    parts = dep_str.split()
    if parts[0] != name:
        return False
    if len(parts) == 1:
        # unversioned entry -> matches only when exactly one version is resolved
        return len(by_name[name]) == 1
    return parts[1] == version

# Reverse edges
def parents(name, version):
    out = []
    for p in pkgs:
        for d in p["deps"]:
            if dep_matches(d, name, version):
                out.append(p)
                break
    return out

WORKSPACE = {p["name"] for p in pkgs if p["name"].startswith(("wcore-", "wayland"))}

all_paths = []
def walk(node, trail, seen):
    key = (node["name"], node["version"])
    ps = parents(node["name"], node["version"])
    ps = [p for p in ps if (p["name"], p["version"]) not in seen]
    if not ps:
        all_paths.append(trail)          # reached a root
        return
    for p in ps:
        walk(p, trail + [f"{p['name']} {p['version']}"], seen | {key})

for p in tv:
    walk(p, [f"{p['name']} {p['version']}"], set())

print(f"\n=== distinct root-ward paths to {target}: {len(all_paths)} ===")
for path in sorted(all_paths, key=lambda x: (len(x), x)):
    print("  " + " <- ".join(path))

print(f"\n=== DIRECT parent edges (the count audit.toml called 'sole') ===")
edges = set()
for p in tv:
    for par in parents(p["name"], p["version"]):
        edges.add(f"{target} {p['version']} <- {par['name']} {par['version']}")
for e in sorted(edges):
    print("  " + e)
print(f"direct parent edge count: {len(edges)}")

wt = sorted({p[-1] for p in all_paths})
print(f"\n=== workspace crates the paths terminate at ===")
for w in wt:
    print("  " + w)
