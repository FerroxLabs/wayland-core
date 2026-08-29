p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
lines = s.split('\n')
out = []
patched = 0
for i, ln in enumerate(lines):
    out.append(ln)
    if ln.strip().startswith('provider: Arc::new(') and i > 0 and 'super::AgentEngine {' in '\n'.join(lines[max(0,i-6):i]):
        indent = ln[:len(ln) - len(ln.lstrip())]
        out.append(f'{indent}spend_guard: test_spend_guard(),')
        patched += 1
print('patched', patched)
s = '\n'.join(out)

helper = '''
/// #174 — an unrestricted, in-memory spend guard for the hand-built
/// `AgentEngine` literals in this test module. The constructors install a real
/// one; these literals bypass the constructors, so they need their own.
#[cfg(test)]
fn test_spend_guard() -> Arc<crate::spend_guard::SpendGuard> {
    Arc::new(crate::spend_guard::SpendGuard::new(
        wcore_budget::SpendMode::Unrestricted,
        "test-session",
        wcore_budget::ModelSpendProfile::new(
            "test",
            "test-model",
            wcore_budget::ModelBilling::Free,
            0.0,
        ),
        Arc::new(wcore_budget::MemorySpendAuditSink::default()),
    ))
}

pub struct AgentEngine {'''
assert s.count('\npub struct AgentEngine {') == 1
s = s.replace('\npub struct AgentEngine {', helper, 1)
open(p,'w').write(s)
