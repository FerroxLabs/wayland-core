p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

s = s.replace(
    "&format!(\"{fallback_provider}/{fallback_model}\"),",
    "&format!(\"{current_attempt_provider}/{current_attempt_model}\"),", 1)
open(p,'w').write(s)
print('ok')
