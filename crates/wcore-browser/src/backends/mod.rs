//! Backend implementations of `BrowserProvider`. The default build only
//! pulls in Camoufox (sidecar HTTP). The `chromium` and `browserbase`
//! features pull their respective implementations.

pub mod camoufox;

#[cfg(feature = "chromium")]
pub mod chromium;

#[cfg(feature = "browserbase")]
pub mod browserbase;

pub use camoufox::CamoufoxBackend;

#[cfg(feature = "chromium")]
pub use chromium::ChromiumBackend;

#[cfg(feature = "browserbase")]
pub use browserbase::BrowserbaseBackend;
