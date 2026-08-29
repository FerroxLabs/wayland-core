import io
p = ".planning/ledger/wayland-core-322.md"
s = io.open(p, encoding="utf-8").read()
mine = '    note: "VERIFIER 2026-08-29'
i = s.find(mine)
assert i != -1, "my note not found"
j = s.find('\n    note: "NARROWED BY #339', i)
assert j != -1, "existing note not found"
my_text = s[i:j].split('note: "',1)[1].rstrip().rstrip('"')
# drop my separate line, then fold its text into the existing note
s = s[:i] + s[j+1:]
old_start = s.find('    note: "NARROWED BY #339')
end = s.find('"\n', old_start + 12)
existing = s[old_start:end]
merged = existing + " || " + my_text
s = s[:old_start] + merged + s[end:]
io.open(p, "w", encoding="utf-8").write(s)
print("merged into one note; note lines on c4 now:", s.count('note: "NARROWED BY #339'))
