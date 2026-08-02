"""TEMPORARY lane tool. DELETE AT MERGE together with win-stdout-capture.yml.

Toggles `collapse_cr_lines` between the fixed form and the shipped, defective
form so the Windows proof job can show the same gates red and green on one
host with nothing else changed.
"""

import sys

path = "crates/wcore-compact/src/sanitize.rs"
src = open(path, encoding="utf-8").read()

FIXED = (
    "    for (index, line) in text.split('\\n').enumerate() {\n"
    "        // Keyed on the split index, not on `result.is_empty()`: an empty first\n"
    "        // line left `result` empty, so the separator for the SECOND line was\n"
    "        // skipped too and a leading blank line vanished from the output.\n"
    "        if index > 0 {\n"
    "            result.push('\\n');\n"
    "        }\n"
    "        let line = line.strip_suffix('\\r').unwrap_or(line);\n"
)

MUTANT = (
    "    for line in text.split('\\n') {\n"
    "        if !result.is_empty() {\n"
    "            result.push('\\n');\n"
    "        }\n"
)

mode = sys.argv[1]
if mode == "mutate":
    if FIXED not in src:
        sys.exit("anchor (fixed form) not found")
    open(path, "w", encoding="utf-8").write(src.replace(FIXED, MUTANT))
    print("MUTANT APPLIED (collapse_cr_lines restored to the shipped, defective form)")
elif mode == "restore":
    if MUTANT not in src:
        sys.exit("anchor (mutant form) not found")
    open(path, "w", encoding="utf-8").write(src.replace(MUTANT, FIXED))
    print("FIX RESTORED")
else:
    sys.exit("usage: mutate.py mutate|restore")
