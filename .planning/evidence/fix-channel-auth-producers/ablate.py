#!/usr/bin/env python3
"""Per-hunk ablation for lane/fix-channel-auth-producers.

Reverts ONE production hunk at a time, with the tests left untouched, and
re-runs the affected crate suites. Every replacement asserts EXACTLY ONE
occurrence and aborts otherwise, so a silently-missed revert cannot masquerade
as a green (LANE-BRIEF section 3.2).

A hunk that changes nothing when reverted is NOT load-bearing, and this script
is how that gets found rather than asserted.
"""
import subprocess, sys, shutil, re, json, os

ROOT = "/root/wayland-authprod"
GW = ROOT + "/crates/wcore-channel-discord/src/gateway.rs"
SL = ROOT + "/crates/wcore-channel-slack/src/lib.rs"

HUNKS = {
    "H1-discord-close-capture": (GW, """                        let code = close_code_of(frame.as_ref());
                        if let Some(reason) = auth_rejection_for_close_code(code) {
                            return Ok(SessionExit::AuthRejected { reason });
                        }
                        return Err(format!("close frame (code {code:?})"));""",
     """                        let _ = frame;
                        return Err("close frame".to_string());"""),

    "H2-discord-authexpired-publish": (GW, """                inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::AuthExpired { reason });
                break;""",
     """                let _ = reason;
                break;"""),

    "H3-slack-auth-test": (SL,
     "match api::auth_test(&self.http, &self.config.api_base_url, &bot_token).await {",
     "match Ok::<String, SlackError>(String::new()) {"),

    "H4-slack-send-authexpired": (SL, """                self.inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::AuthExpired {
                        reason: format!("slack refused the bot token on send: {code}"),
                    });
                return Err(ChannelError::Auth(code));""",
     """                return Err(ChannelError::Auth(code));"""),
}

CRATES = ["wcore-channel-discord", "wcore-channel-slack"]


def run_suites(tag):
    out = {}
    for c in CRATES:
        p = subprocess.run(["/root/.cargo/bin/cargo", "test", "-p", c],
                           cwd=ROOT, capture_output=True, text=True)
        open("/tmp/lane-authprod-ablate-%s-%s.log" % (tag, c), "w").write(p.stdout + p.stderr)
        passed = failed = 0
        for m in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed", p.stdout):
            passed += int(m.group(1)); failed += int(m.group(2))
        names = re.findall(r"^test (\S+) \.\.\. FAILED", p.stdout, re.M)
        build_ok = "error[" not in p.stderr and "could not compile" not in p.stderr
        out[c] = {"passed": passed, "failed": failed,
                  "failed_names": names, "build_ok": build_ok}
    return out


def bump(path):
    """Force the source NEWER than any artifact built from it.

    REPAIRED 2026-07-30, after this harness produced a false result. Backups
    were taken with shutil.copy, which does NOT preserve mtime, so restoring
    one set the source mtime BACKWARDS. Cargo fingerprints on mtime, decided
    the artifact was current, and skipped the rebuild -- so the next run
    executed the PREVIOUS ablation's binary while `git status` reported a clean
    tree. Measured: gateway.rs at 14:46:34 against a test binary at 14:46:35,
    a discord test failing under a SLACK-only ablation, and `touch` alone
    turning it green again.
    """
    os.utime(path, None)


def apply(path, old, new):
    src = open(path).read()
    n = src.count(old)
    if n != 1:
        sys.exit("ABORT: expected exactly 1 occurrence, found %d in %s" % (n, path))
    open(path, "w").write(src.replace(old, new))
    bump(path)


results = {}
print("=== BASELINE (all hunks present) ===", flush=True)
results["baseline"] = run_suites("baseline")
print(json.dumps(results["baseline"], indent=2), flush=True)

for name in HUNKS:
    path, old, new = HUNKS[name]
    backup = path + ".ablate.bak"
    shutil.copy(path, backup)
    try:
        apply(path, old, new)
        print("\n=== REVERTED %s ===" % name, flush=True)
        r = run_suites(name)
        results[name] = r
        print(json.dumps(r, indent=2), flush=True)
    finally:
        shutil.move(backup, path)
        bump(path)
        # Write results as we go: an abort must not discard everything
        # measured so far.
        open("/tmp/lane-authprod-ablation.json", "w").write(json.dumps(results, indent=2))

# Closing control. If the tree did not return to the baseline numbers, some
# restore did not take and every red above is suspect. This is the check that
# would have caught the stale-artifact contamination immediately.
print("\n=== RESTORE CONTROL (must match baseline exactly) ===", flush=True)
results["restored"] = run_suites("restored")
print(json.dumps(results["restored"], indent=2), flush=True)

open("/tmp/lane-authprod-ablation.json", "w").write(json.dumps(results, indent=2))
print("\n=== SUMMARY ===")
b = results["baseline"]
r = results["restored"]
ok = all(r[c]["passed"] == b[c]["passed"] and r[c]["failed"] == b[c]["failed"]
         for c in CRATES)
print("RESTORE CONTROL: %s" % ("PASS - tree returned to baseline" if ok
      else "*** FAIL - a restore did not take; every result below is suspect ***"))
for name in results:
    if name in ("baseline", "restored"):
        continue
    r = results[name]
    reddened = []
    for c in CRATES:
        if r[c]["failed"] > b[c]["failed"] or not r[c]["build_ok"]:
            reddened.append("%s: %d failed %s build_ok=%s"
                            % (c, r[c]["failed"], r[c]["failed_names"], r[c]["build_ok"]))
    verdict = "LOAD-BEARING" if reddened else "*** NOT LOAD-BEARING - nothing reddened ***"
    print("%s: %s" % (name, verdict))
    for x in reddened:
        print("    " + x)
