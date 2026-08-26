import io
import re
import glob as globmod

BAD = re.compile(r"->\s*McpToolDef\s*\{\s*$")
files = []
for pattern in ["crates/*/src/**/*.rs", "crates/*/tests/**/*.rs"]:
    files.extend(globmod.glob(pattern, recursive=True))
fixed = []
for f in sorted(set(files)):
    lines = io.open(f, encoding="utf-8").read().split("\n")
    out = []
    removed = 0
    for i, line in enumerate(lines):
        if (
            line.strip() == "annotations: None,"
            and i > 0
            and BAD.search(lines[i - 1])
        ):
            removed += 1
            continue
        out.append(line)
    if removed:
        io.open(f, "w", encoding="utf-8").write("\n".join(out))
        fixed.append((f, removed))
for f, n in fixed:
    print("unfixed-return-type", f, n)
print("p5b ok")
