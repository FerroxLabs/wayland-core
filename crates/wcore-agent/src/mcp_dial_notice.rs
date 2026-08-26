//! The user-facing half of the MCP dial: it is bounded, so say so.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::output::OutputSink;

/// How long a dial runs before it says that it is running.
///
/// Five seconds, the same budget every other step of the pre-provider path
/// uses — `wcore_providers::http_client::STREAM_SILENCE_NOTICE_AFTER` for the
/// provider stream and `KEY_STORE_ACQUIRE_BUDGET` in
/// [`crate::recovery_confidential`] for the seal. From the user's side these
/// are not three waits, they are one wait, and three patience budgets on it
/// would be three answers to one question.
pub const MCP_DIAL_NOTICE_AFTER: Duration = Duration::from_secs(5);

/// Await an MCP dial, and tell the user if it takes a while.
///
/// # Why a bounded wait still needs a notice
///
/// The dial IS bounded — `wcore_mcp::manager::CONNECT_TIMEOUT` per server,
/// every server dialed concurrently — so it always ends. But a bound nobody
/// is told about is indistinguishable at the host from a wedge. Measured on
/// 0.13.8 with one stdio server that never speaks (`command = "sleep"`): the
/// host got NOTHING for 30.3 s, then `mcp_failed`, then `stream_start`.
/// Thirty seconds of a blank turn reads as a dead app whatever caused it, and
/// the cause was never on the wire.
///
/// # The channel is the whole point
///
/// `tracing::warn!` cannot fix "the user isn't told". With `RUST_LOG` unset
/// only `ERROR` reaches stderr and everything else goes to a log file nobody
/// has open during a turn — a trap that has silently defeated three features
/// in this repo already. This goes through [`OutputSink::emit_info`], which is
/// a `ProtocolEvent::Info` for a json-stream host and a terminal line for a
/// CLI run, the same channel and the same shape as the engine's existing
/// "Still waiting on the provider" notice.
///
/// # One line, not a ticker
///
/// Latched, for the same reason the provider notice is latched: the job is to
/// break the silence and name the cause, and a line every five seconds during
/// a bounded wait is noise a reader learns to scroll past. It names the
/// deadline it is counting towards — read from `wcore_mcp`, never copied —
/// so the reader knows what they are waiting for and that it will end.
///
/// Nothing is cancelled, retried or failed: the returned value is exactly
/// what `dial` produced, whenever it produced it.
pub async fn announce_slow_mcp_dial<F>(dial: F, output: &Arc<dyn OutputSink>) -> F::Output
where
    F: Future,
{
    let mut dial = std::pin::pin!(dial);
    tokio::select! {
        settled = &mut dial => settled,
        _ = tokio::time::sleep(MCP_DIAL_NOTICE_AFTER) => {
            output.emit_info(&format!(
                "Still waiting on MCP servers to connect - no output for {}s. Each server is \
                 bounded at {}s, then this continues without the ones that did not answer.",
                MCP_DIAL_NOTICE_AFTER.as_secs(),
                wcore_mcp::manager::CONNECT_TIMEOUT.as_secs(),
            ));
            dial.await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestSink, TestSinkHandle};

    fn sink() -> (Arc<dyn OutputSink>, TestSinkHandle) {
        let sink = TestSink::new();
        let handle = sink.handle();
        (Arc::new(sink), handle)
    }

    fn notices(handle: &TestSinkHandle) -> Vec<String> {
        handle
            .snapshot()
            .iter()
            .filter(|event| event["type"].as_str() == Some("info"))
            .filter_map(|event| event["message"].as_str().map(str::to_string))
            .filter(|message| message.contains("Still waiting on MCP servers"))
            .collect()
    }

    /// A dial that outlives the budget is announced exactly once, and the
    /// notice names the real deadline rather than a copy of it.
    ///
    /// `start_paused` so the clock is the runtime's: the fixture settles one
    /// second short of the live per-server deadline, which is precisely the
    /// window this notice exists to fill, and the test still runs in
    /// milliseconds.
    #[tokio::test(start_paused = true)]
    async fn a_slow_dial_is_announced_once_with_the_real_deadline() {
        let (output, events) = sink();
        let settled = announce_slow_mcp_dial(
            async {
                tokio::time::sleep(wcore_mcp::manager::CONNECT_TIMEOUT - Duration::from_secs(1))
                    .await;
                "dialled"
            },
            &output,
        )
        .await;

        assert_eq!(settled, "dialled", "the notice must not change the outcome");
        let notices = notices(&events);
        assert_eq!(notices.len(), 1, "exactly one line, got {notices:?}");
        assert!(
            notices[0].contains(&format!(
                "{}s",
                wcore_mcp::manager::CONNECT_TIMEOUT.as_secs()
            )),
            "the notice must name the deadline it counts towards, got {:?}",
            notices[0]
        );
    }

    /// The boot dial has exactly one await, and it goes through the notice.
    ///
    /// A helper nothing calls is not a fix, and the failure this guards is
    /// specifically a SECOND call site left bare. The count is the assertion;
    /// finding the string at all is the known-positive control for it.
    #[test]
    fn the_boot_dial_is_never_awaited_without_the_notice() {
        let bootstrap = include_str!("bootstrap.rs");
        assert_eq!(
            bootstrap.matches("connect_all_with_policy").count(),
            1,
            "bootstrap gained or lost an MCP dial; every one of them must be announced"
        );
        assert!(
            bootstrap.contains("announce_slow_mcp_dial(dial, &self.output)"),
            "the boot dial no longer goes through the notice"
        );
        assert!(
            !bootstrap.contains("connect_all_with_policy(&resolved_servers, egress_policy).await"),
            "the boot dial is bare-awaited again: it will take up to the full per-server \
             deadline with nothing said to the user"
        );
    }

    /// A healthy dial says nothing. A notice printed on every launch is a
    /// notice nobody reads on the launch that matters.
    #[tokio::test(start_paused = true)]
    async fn a_dial_inside_the_budget_says_nothing() {
        let (output, events) = sink();
        let settled = announce_slow_mcp_dial(
            async {
                tokio::time::sleep(MCP_DIAL_NOTICE_AFTER - Duration::from_secs(1)).await;
                7u8
            },
            &output,
        )
        .await;

        assert_eq!(settled, 7);
        assert!(
            notices(&events).is_empty(),
            "a dial that finished inside its budget must be silent"
        );
    }
}
