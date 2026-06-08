//! E.1 integration test — `BrowserProvider` trait surface must compile and
//! be dyn-dispatchable. Plus minimum end-to-end smoke (open → dispatch
//! GetState → close) against an in-test stub.

use async_trait::async_trait;
use wcore_browser::{BrowserOp, BrowserOpError, BrowserProvider, BrowserSession, SessionCtx};

struct Smoke;

#[async_trait]
impl BrowserProvider for Smoke {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        Ok(BrowserSession {
            ctx: SessionCtx::for_test("smoke"),
            persistent_profile,
        })
    }
    async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        Ok(())
    }
    async fn dispatch(
        &self,
        _ctx: &SessionCtx,
        _op: BrowserOp,
    ) -> Result<wcore_browser::provider::OpResult, BrowserOpError> {
        Ok(wcore_browser::provider::OpResult::Ok)
    }
    fn backend_name(&self) -> &'static str {
        "smoke"
    }
}

#[tokio::test]
async fn provider_trait_is_dyn_dispatchable() {
    let provider: Box<dyn BrowserProvider> = Box::new(Smoke);
    let s = provider.open_session(false).await.unwrap();
    assert_eq!(s.ctx.session_id, "smoke");
    let _ = provider
        .dispatch(&s.ctx, BrowserOp::GetState {})
        .await
        .unwrap();
    provider.close_session(&s.ctx).await.unwrap();
    assert_eq!(provider.backend_name(), "smoke");
}
