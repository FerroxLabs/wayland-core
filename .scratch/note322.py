import io, re
p = ".planning/ledger/wayland-core-322.md"
s = io.open(p, encoding="utf-8").read()
# append to c4's note, or add one
m = re.search(r"(  - id: c4\n(?:.*\n)*?    owner: core\n)", s)
assert m, "c4 block not found"
add = ("    note: \"VERIFIER 2026-08-29 (lane/f13-atref-residuals, verdict PARTIAL): the named case IS "
       "closed and at_ref_resolve.rs:1070 passes, but 'the same treatment' does not hold and this "
       "entry's parity rationale is refutable from the tree. is_vcs_store_or_control_dir tests the "
       "path ITSELF; the deny walk it claims one-list-one-owner parity with uses inside_vcs_store "
       "(workspace_policy.rs:2835, path.ancestors().any(is_vcs_store_dir)) at :93. Different "
       "predicates, so a store reached under another name is not given equal treatment. Graded as a "
       "narrowing sold as a closure - c4 stays not-met.\"\n")
if "VERIFIER 2026-08-29" not in s:
    s = s[:m.end(1)] + add + s[m.end(1):]
    io.open(p, "w", encoding="utf-8").write(s)
    print("noted core#322 c4")
else:
    print("already noted")
