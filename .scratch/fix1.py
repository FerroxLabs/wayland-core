import io
p='crates/wcore-budget/src/spend.rs'
s=open(p).read()
old = """    /// Total order from most permissive to most restrictive. Everything
    /// [] admits, [] admits too, and everything
    ///  admits,  admits — so the modes really do form a
    /// ladder and "stricter" is well defined.
"""
new = """    /// Total order from most permissive to most restrictive. Everything
    /// [`Self::LocalOnly`] admits, [`Self::NoPaid`] admits too, and everything
    /// `NoPaid` admits, `Unrestricted` admits — so the modes really do form a
    /// ladder and "stricter" is well defined.
"""
assert old in s, "spend.rs doc not found"
s = s.replace(old, new, 1)
open(p,'w').write(s)

p2='crates/wcore-config/src/config.rs'
c=open(p2).read()
old2 = "        // STRICTEST wins, for the same reason as : the\n"
new2 = "        // STRICTEST wins, for the same reason as `max_daily_cost_usd`: the\n"
assert old2 in c, "config.rs comment not found"
c = c.replace(old2, new2, 1)
open(p2,'w').write(c)
print('fixed')
