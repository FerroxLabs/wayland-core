//! Backend implementations of `BrowserProvider`. The default build only
//! pulls in Camoufox (sidecar HTTP). The `browserbase` feature pulls in the
//! cloud implementation.

pub mod camoufox;

#[cfg(feature = "browserbase")]
pub mod browserbase;

pub use camoufox::CamoufoxBackend;

#[cfg(feature = "browserbase")]
pub use browserbase::BrowserbaseBackend;
