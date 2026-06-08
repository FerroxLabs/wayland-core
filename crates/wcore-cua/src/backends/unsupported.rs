//! Fallback backend for platforms the crate doesn't ship a native
//! implementation for. Returns `CuaError::UnsupportedPlatform` for every
//! op so the tool layer can surface a typed error without panicking.

use async_trait::async_trait;

use crate::backend::{ComputerUseBackend, CuaSession, Platform};
use crate::error::{CuaError, CuaResult};
use crate::op::{CuaOp, CuaOpResult};

pub struct UnsupportedBackend {
    platform: Platform,
}

impl UnsupportedBackend {
    pub fn new(platform: Platform) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl ComputerUseBackend for UnsupportedBackend {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn platform(&self) -> Platform {
        self.platform
    }

    async fn dispatch(&self, _session: &CuaSession, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Wait is a useful exception — sleeping is platform-neutral so
        // we honour it even on the fallback backend so the cancel-token
        // race wraps it normally.
        if let CuaOp::Wait { duration_ms } = op {
            tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
            return Ok(CuaOpResult::Ok);
        }
        Err(CuaError::UnsupportedPlatform(
            "no native cua backend for this platform",
        ))
    }

    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_is_honoured() {
        let b = UnsupportedBackend::new(Platform::Unsupported);
        let r = b
            .dispatch(&CuaSession::for_test("u"), CuaOp::Wait { duration_ms: 1 })
            .await
            .unwrap();
        assert!(matches!(r, CuaOpResult::Ok));
    }

    #[tokio::test]
    async fn click_returns_unsupported() {
        let b = UnsupportedBackend::new(Platform::Unsupported);
        let r = b
            .dispatch(
                &CuaSession::for_test("u"),
                CuaOp::LeftClick {
                    x: 1,
                    y: 1,
                    button: crate::backend::MouseButton::Left,
                    mods: crate::backend::KeyMods::default(),
                },
            )
            .await;
        assert!(matches!(r, Err(CuaError::UnsupportedPlatform(_))));
    }

    #[tokio::test]
    async fn frontmost_is_none() {
        let b = UnsupportedBackend::new(Platform::Unsupported);
        assert!(b.frontmost_app().await.unwrap().is_none());
    }
}
