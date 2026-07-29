#!/usr/bin/env python3
"""Self-test for the repaired brace scanner (LANE-BRIEF §6b-ii, three assertions).

Assertion 3 is the one that matters: it proves the OLD naive matcher was wrong
on the real source line that produced the false positive. Without it, the
self-test would pass on the broken instrument too.
"""
import sys

from rsutil import fn_body_range, strip_literals

SRC = '''\
    #[test]
    fn google_meet_token_status_absent_when_unparsable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("google_meet.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert_eq!(google_meet_token_status(&path), GoogleMeetTokenStatus::Absent);
    }

    #[test]
    #[serial]
    fn esc_saves_an_unsaved_toggle() {
        let prev = std::env::var_os("WAYLAND_HOME");
    }
'''.splitlines()

FN_START = 1          # `fn google_meet_...` line index
TRUE_END = 6          # its closing brace


def naive_end(lines, start):
    """The ORIGINAL broken matcher: counts braces including those in strings."""
    depth, started, k = 0, False, start
    while k < len(lines):
        depth += lines[k].count("{") - lines[k].count("}")
        if "{" in lines[k]:
            started = True
        if started and depth <= 0:
            return k
        k += 1
    return len(lines) - 1


def v2_end(lines, start):
    """The PREVIOUS repair: strips comments/raw-strings and single-line strings,
    but has no state for an ordinary string that spans lines. Reproduced by
    clearing the multi-line-string flag before each line."""
    state = {}
    depth, started, k = 0, False, start
    while k < len(lines):
        state["str"] = False
        code = strip_literals(lines[k], state)
        depth += code.count("{") - code.count("}")
        if "{" in code:
            started = True
        if started and depth <= 0:
            return k
        k += 1
    return len(lines) - 1


def main():
    ok = True

    # A1 known-positive: repaired scanner bounds the body correctly.
    got = fn_body_range(SRC, FN_START)
    a1 = got == TRUE_END
    print(f"A1 known-positive  : repaired end={got} expected={TRUE_END} -> "
          f"{'PASS' if a1 else 'FAIL'}")
    ok &= a1

    # A2 known-negative: the body must NOT swallow the following test, so the
    # WAYLAND_HOME mutation at line 11 must fall OUTSIDE the range.
    body = "\n".join(SRC[FN_START:got + 1])
    a2 = "WAYLAND_HOME" not in body
    print(f"A2 known-negative  : WAYLAND_HOME absent from body -> "
          f"{'PASS' if a2 else 'FAIL'}")
    ok &= a2

    # A3 the OLD matcher would have missed it -- proves the repair does work.
    old = naive_end(SRC, FN_START)
    old_body = "\n".join(SRC[FN_START:old + 1])
    a3 = old != TRUE_END and "WAYLAND_HOME" in old_body
    print(f"A3 old-matcher-wrong: naive end={old} (swallowed next test, "
          f"WAYLAND_HOME leaked in) -> {'PASS' if a3 else 'FAIL'}")
    ok &= a3

    # Bonus: literal stripping leaves no stray brace on the offending line.
    st = {}
    stripped = [strip_literals(l, st) for l in SRC]
    print(f"   line 4 stripped  : {stripped[4]!r}")

    # ---- Multi-line string literal (second repair) ----
    # A Rust string continued across lines whose contents contain shell-escaped
    # braces. Previously the scanner restarted mid-string and counted `{{`.
    SRC2 = '''\
    #[tokio::test]
    async fn rank24_close_reaps_grandchildren() {
        let script = format!(
            "sleep 300 & echo $! > {pid}; read line; \\
             printf '{{\\"jsonrpc\\":\\"2.0\\",\\"result\\":{{}}}}\\\\n'",
            pid = pid_path.display()
        );
    }

    #[tokio::test]
    async fn b1_profile_home_reaches_spawned_child() {
        unsafe { std::env::set_var("WAYLAND_HOME", &wh) };
    }
'''.splitlines()
    S2_START, S2_TRUE_END = 1, 7

    got2 = fn_body_range(SRC2, S2_START)
    b1 = got2 == S2_TRUE_END
    print(f"\nB1 multiline-string: repaired end={got2} expected={S2_TRUE_END} -> "
          f"{'PASS' if b1 else 'FAIL'}")
    ok &= b1

    body2 = "\n".join(SRC2[S2_START:got2 + 1])
    b2 = "WAYLAND_HOME" not in body2
    print(f"B2 known-negative  : WAYLAND_HOME absent from body -> "
          f"{'PASS' if b2 else 'FAIL'}")
    ok &= b2

    # B3 runs against the REAL source file, not the synthetic fixture: the
    # synthetic one does not reproduce the imbalance (its braces happen to
    # balance), and asserting on a fixture that does not exhibit the bug would
    # be a self-passing test. `rank24_close_reaps_grandchildren` at
    # stdio.rs:1047 truly ends at 1119; the naive matcher ran to 1502 and
    # swallowed 8 following tests, including two env mutators.
    import os
    real = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "..", "..", "..", "crates", "wcore-mcp", "src",
                        "transport", "stdio.rs")
    if os.path.exists(real):
        rl = open(real, encoding="utf-8", errors="replace").read().splitlines()
        start = next(i for i, l in enumerate(rl)
                     if "fn rank24_close_reaps_grandchildren" in l)
        rep = fn_body_range(rl, start)
        v2 = v2_end(rl, start)
        rep_body = "\n".join(rl[start:rep + 1])
        v2_body = "\n".join(rl[start:v2 + 1])
        b3 = (v2 > rep
              and "WAYLAND_HOME" in v2_body
              and "WAYLAND_HOME" not in rep_body)
        print(f"B3 prior-version-wrong (REAL FILE): repaired end={rep+1}, "
              f"v2 (single-line stripping only) end={v2+1}; v2 leaked "
              f"WAYLAND_HOME in, repaired did not -> {'PASS' if b3 else 'FAIL'}")
        print(f"   (fully-naive end={naive_end(rl, start)+1} -- coincidentally "
              f"correct here, because the raw line's shell-escaped braces "
              f"BALANCE; it was v2's partial stripping that unbalanced them, "
              f"so v2 was worse than v1 on this case.)")
    else:
        b3 = False
        print("B3 old-matcher-wrong (REAL FILE): source not found -> FAIL")
    ok &= b3

    print("\nSELF-TEST:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
