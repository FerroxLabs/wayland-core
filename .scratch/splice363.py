p = "crates/wcore-channel-discord/src/lib.rs"
s = open(p).read()
add = open(".scratch/c6discord.rs").read()
s = s.rstrip()
assert s.endswith("}")
s = s[: s.rfind("}")].rstrip("\n") + "\n" + add
open(p, "w").write(s)
print("spliced")
