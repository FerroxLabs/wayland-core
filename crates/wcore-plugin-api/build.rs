//! Build-time lint that fails the build if `wcore-plugin-api` ever picks up
//! a dependency on a core wcore crate or on a dylib-loading crate. This is
//! the FORBIDDEN_CORE_IMPORTS check per design spec §5.17.

use std::fs;
use std::path::PathBuf;

const FORBIDDEN_CORE_IMPORTS: &[&str] = &[
    "wcore-agent",
    "wcore-tools",
    "wcore-mcp",
    "wcore-skills",
    "wcore-memory",
    "wcore-config",
    "wcore-providers",
    "wcore-compact",
    // Wave 3 mid-tier crates (Z0 audit STABILITY MAJOR #3):
    "wcore-browser",
    "wcore-cua",
    "wcore-eval",
    "wcore-evolve",
    "wcore-observability",
    "wcore-repomap",
    // F25-01 (2026-07-26). This crate carries real execution authority AND a
    // credentialed network transport (ssh, and a cloud control plane holding a
    // bearer token). It is precisely what audit finding F2 forbids a plugin
    // from reaching, so a plugin describes an execution backend through
    // `ExecutionBackendSpec` and the host adapter in wcore-agent translates it
    // after `initialize()` returns. Without this entry the lint would have
    // silently permitted the dependency the mirror exists to avoid.
    "wcore-exec-backend",
    //
    // DELIBERATELY ABSENT: `wcore-sandbox`.
    //
    // The F25-01 plan asserted that its omission was an unnoticed hole. It is
    // measured NOT to be. `wcore-plugin-api/Cargo.toml` carries
    // `wcore-sandbox.workspace = true` as an explicit M5.1 SECURITY-ALLOWLISTED
    // dependency, annotated as such in the manifest, because `PluginContext`
    // hands plugins a `SandboxRegistry` HANDLE (`context.rs`, field `sandbox`)
    // so a `requires_sandbox = true` tool can be contained at all. Adding
    // `wcore-sandbox` to this list fails the build of the existing tree
    // immediately — verified 2026-07-26 on hetzner-dsm, build.rs exit 101.
    //
    // The distinction that makes both decisions correct: `wcore-sandbox` is
    // handed over as a CONSTRAINING capability (it can only narrow what a
    // plugin may do), whereas `wcore-exec-backend` would be an ENABLING one
    // (off-box execution plus a credential). Reversing the M5.1 allowlist is a
    // real architectural question and it belongs to whoever owns that
    // decision, not to a phase that happened to walk past it. Recorded in the
    // 25-01 SUMMARY as a finding rather than silently forced through.
    "libloading",
    "wasmtime",
    "wasmer",
    "dlopen",
    "abi_stable",
];

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");

    // SAFETY: `CARGO_MANIFEST_DIR` is set by cargo for every build
    // script invocation. A missing value means the build script is
    // being invoked outside cargo, which is not supported. Build
    // scripts may panic to fail the build — that's the canonical
    // signalling mechanism.
    let manifest_path =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"))
            .join("Cargo.toml");

    let manifest =
        fs::read_to_string(&manifest_path).unwrap_or_else(|e| panic!("read Cargo.toml: {e}"));

    let parsed: toml::Value =
        toml::from_str(&manifest).unwrap_or_else(|e| panic!("parse Cargo.toml: {e}"));

    // Walk every [dependencies], [dev-dependencies], [build-dependencies] table.
    // Any key matching FORBIDDEN_CORE_IMPORTS fails the build with a diagnostic
    // that points at design §5.17.
    let mut violations: Vec<(String, String)> = Vec::new();
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = parsed.get(table_name).and_then(|v| v.as_table()) {
            for key in table.keys() {
                if FORBIDDEN_CORE_IMPORTS.contains(&key.as_str()) {
                    violations.push((table_name.to_string(), key.to_string()));
                }
            }
        }
    }

    if !violations.is_empty() {
        let lines: Vec<String> = violations
            .iter()
            .map(|(table, dep)| format!("  - [{table}] {dep}"))
            .collect();
        panic!(
            "\n\nFORBIDDEN_CORE_IMPORTS violation in wcore-plugin-api/Cargo.toml:\n{}\n\n\
             The wcore-plugin-api crate is the isolation boundary for the plugin\n\
             architecture (design spec §5.17). It must NOT depend on any wcore-* crate\n\
             beyond wcore-types and wcore-protocol, and it must NOT depend on any\n\
             dynamic-loading crate (libloading / wasmtime / etc.).\n\
             \n\
             If you genuinely need to expose something from a forbidden crate, mirror\n\
             its types here as api-crate-local definitions and let the host adapter\n\
             translate. See e.g. BundledSkillSpec vs wcore_skills::BundledSkillDefinition.\n",
            lines.join("\n")
        );
    }
}
