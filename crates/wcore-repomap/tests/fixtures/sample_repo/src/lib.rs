//! Sample crate for wcore-repomap fixture tests.

use std::path::PathBuf;

pub fn hello() -> &'static str {
    "hi"
}

pub struct Greeter {
    pub name: String,
}

pub enum Mood {
    Happy,
    Sad,
}

impl Greeter {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

pub mod inner;

pub use crate::inner::Helper;
