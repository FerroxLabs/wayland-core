pub mod artifacts;
pub mod audit;
pub mod bundled;
pub mod conditional;
pub mod context_modifier;
pub mod curate;
pub mod discovery;
pub mod draft;
pub mod executor;
pub mod frontmatter;
pub mod hooks;
pub mod loader;
pub mod mcp;
pub mod paths;
pub mod permissions;
pub mod prioritizer;
pub mod prompt;
pub mod refs;
pub mod router;

pub use router::{SkillRouter, SkillRouterInput};
pub mod shell;
pub mod substitution;
pub mod telemetry;
pub mod types;
pub mod watcher;

#[cfg(test)]
mod permissions_supplemental_tests;

#[cfg(test)]
#[path = "integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod bundled_supplemental_tests;

#[cfg(test)]
mod watcher_tests;

#[cfg(test)]
mod w9_module_presence_smoke {
    /// Compile-time witness: these paths exist in the crate.
    #[test]
    fn draft_module_compiles() {
        let _ = crate::draft::MODULE_NAME;
    }

    #[test]
    fn curate_module_compiles() {
        let _ = crate::curate::MODULE_NAME;
    }
}
