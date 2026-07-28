#!/usr/bin/env python3
"""portability-migrate-corpus.py — synthesise a peer home whose migration does
enough real work for a mid-apply interruption to be landable.

WHY A BIG CORPUS RATHER THAN A PACING KNOB

`backup restore` carries a `--pace-ms` test affordance, so 26-03's interruption
proof could size its own window. `migrate` carries no such knob, and adding one
to the product purely so this proof can land its kill would be instrumenting the
thing under test. So the window is made real instead: `apply_plan` admits every
executable item ONE AT A TIME, and each admit re-serialises and rewrites the
WHOLE quarantine index, so a corpus of N executable items costs O(N^2) index
bytes. That is the product's actual behaviour at a size a real Hermes home
reaches -- 26-02 measured 540 skill directories in the real install -- not a
contrivance.

The count ceiling is `MAX_QUARANTINE_FILES = 512`; the default here stays under
it so every item is genuinely ADMITTED rather than refused, because a refusal
path writes nothing and would give the kill nothing to land in.

Shapes:
  hermes    profiles/<n>/config.yaml + skills/<n>/SKILL.md carrying a shell
            directive (the predicate `contains_shell_commands` actually keys on)
  openclaw  openclaw.json whose mcp.servers each carry a launch `command`, which
            is the reciprocal path's executable class

Usage: portability-migrate-corpus.py --kind hermes|openclaw --out DIR [--items N]
"""

import argparse
import json
import os
import sys

# The directive the skills executor's own predicate recognises. Anything else
# would exercise a classifier the product does not run.
SHELL_DIRECTIVE = "!`echo migrate-interrupt-corpus-marker`"

PROFILE_CONFIG = "model:\n  default: claude-opus-4\n  provider: anthropic\n"


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def skill_body(name):
    return (
        "---\n"
        f"name: {name}\n"
        "description: migrate interruption corpus fixture\n"
        "---\n\n"
        f"Run this: {SHELL_DIRECTIVE}\n"
        # Padding so each payload is a non-trivial write rather than a few
        # bytes the kernel absorbs in one go.
        + ("filler line for a realistic payload size\n" * 24)
    )


def gen_hermes(out, items):
    write(os.path.join(out, "config.yaml"), PROFILE_CONFIG)
    # A handful of profiles so `config.toml` actually gains content: without a
    # profile the apply's final `patch_global_config` would be a no-op and the
    # "config not yet written" half of the mid-flight check would be vacuous.
    for i in range(8):
        write(
            os.path.join(out, "profiles", f"prof{i:02d}", "config.yaml"),
            PROFILE_CONFIG,
        )
    for i in range(items):
        name = f"skill{i:04d}"
        write(os.path.join(out, "skills", name, "SKILL.md"), skill_body(name))
    return {"profiles": 8, "skills": items}


def gen_openclaw(out, items):
    servers = {}
    for i in range(items):
        servers[f"srv{i:04d}"] = {
            "command": "/usr/bin/env",
            "args": ["node", f"server-{i:04d}.js", "--flag", "x" * 64],
            "env": {"CORPUS_PAD": "y" * 64},
        }
    doc = {
        "agents": {"defaults": {"model": {"primary": "anthropic/claude-opus-4"}}},
        "models": {
            "providers": {
                "anthropic": {
                    "baseUrl": "https://api.anthropic.com",
                    "models": [{"id": "claude-opus-4"}],
                },
                "openai": {
                    "baseUrl": "https://api.openai.com/v1",
                    "models": [{"id": "gpt-5"}],
                },
            }
        },
        "mcp": {"servers": servers},
    }
    write(os.path.join(out, "openclaw.json"), json.dumps(doc, indent=2))
    return {"providers": 2, "mcp_servers": items}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--kind", required=True, choices=["hermes", "openclaw"])
    ap.add_argument("--out", required=True)
    ap.add_argument("--items", type=int, default=440)
    args = ap.parse_args()

    if args.items >= 512:
        print(
            "refusing: --items must stay under the 512 quarantine count ceiling, "
            "or the items are refused rather than admitted and the corpus proves "
            "nothing about the admit loop",
            file=sys.stderr,
        )
        return 2

    os.makedirs(args.out, exist_ok=True)
    counts = (gen_hermes if args.kind == "hermes" else gen_openclaw)(
        args.out, args.items
    )
    for key in sorted(counts):
        print(f"CORPUS-{key.upper()}: {counts[key]}")
    print("CORPUS-KIND: " + args.kind)
    print("CORPUS-ROOT: " + os.path.abspath(args.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
