//! Re-export of `wcore_types::model_aliases` for integration-test convenience.
//!
//! This file used to host the constants directly (introduced in Task H);
//! Task H.1 relocated the source of truth to `wcore-types` so inline
//! `#[cfg(test)]` blocks in other crates' `src/` files can share the same
//! aliases without crate-boundary path tricks. Add new accessors there.

#![allow(unused_imports)]

pub use wcore_types::model_aliases::*;
