import sys
p = "crates/wcore-mcp/tests/mcp_malware_gate_boundaries.rs"
s = open(p).read()
old = '''async fn launch_program(program: &str, args: &[&str], marker: &Path) -> Launch {
    let result = StdioTransport::spawn(program, &argv(args), &HashMap::new()).await;
    // Long enough for the script's first line; the refusal arms assert the
    // marker is ABSENT, so this wait is what makes that assertion mean "never
    // ran" rather than "had not run yet".
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let executed = marker.exists();
    Launch { result, executed }
}'''
new = '''/// How long "it never ran" is allowed to take before we believe it.
///
/// A FIXED sleep here is a flake, not a shortcut: at 500 ms this helper was
/// observed reporting `executed = false` for a launch that DID run, on a build
/// host under load ~35. The permissive arms then fail on their own premise
/// ("the default posture is fail-OPEN") while the security assertion below
/// them never executes — a red that says nothing about the gate.
///
/// So: poll. The permissive arms return as soon as the child has written its
/// marker, and the refusal arms — the ones whose assertion is `!executed` —
/// wait the whole budget, which is what makes "absent" mean "never ran"
/// rather than "had not run yet".
const EXEC_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

async fn launch_program(program: &str, args: &[&str], marker: &Path) -> Launch {
    let result = StdioTransport::spawn(program, &argv(args), &HashMap::new()).await;
    let deadline = std::time::Instant::now() + EXEC_WAIT;
    let mut executed = marker.exists();
    while !executed && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        executed = marker.exists();
    }
    Launch { result, executed }
}'''
assert s.count(old) == 1, s.count(old)
open(p, "w").write(s.replace(old, new, 1))
print("patched launch_program")
