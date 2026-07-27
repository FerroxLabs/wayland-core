//! `wayland-channel-sink` — the independent delivery destination, as its own
//! process.
//!
//! Phase 24. The library fixture in `fixtures::channel` can run in-process for
//! unit tests, but Success Criterion 1 is about a SERVICE that is killed and
//! restarted by the platform. An in-process sink dies with the test harness and
//! could never observe a delivery made by a systemd-supervised gateway.
//!
//! This binary is started first, outlives the gateway across its `kill -9` and
//! its platform-driven restart, and owns the arrivals journal for the whole
//! window. That is what makes the count independent: the gateway's only way to
//! add a line to that file is to complete a real TCP round trip to a process it
//! does not control and cannot restart.
//!
//! ```text
//! wayland-channel-sink --port 8471 --journal /tmp/f24c/arrivals.jsonl
//! wayland-channel-sink --port 8471 --journal ... --stall-after 8
//! ```
//!
//! `--stall-after N` answers the first N deliveries and then accepts, journals
//! and never answers the next one — the only way to place a delivery in the
//! gateway ledger's outcome-unknown class from outside the gateway.

use std::path::PathBuf;

use clap::Parser;
use wcore_eval_scenarios::fixtures::channel::{ChannelSink, SinkMode};

#[derive(Parser, Debug)]
#[command(
    name = "wayland-channel-sink",
    about = "Independent hermetic delivery sink for Phase 24 arrival counting"
)]
struct Args {
    /// Loopback port to bind. 0 picks an ephemeral port and prints it.
    #[arg(long, default_value_t = 0)]
    port: u16,

    /// Arrivals journal this sink owns. One JSON record per arrival.
    #[arg(long)]
    journal: PathBuf,

    /// Answer this many deliveries, then accept-journal-and-never-answer the
    /// next one. Omit to answer everything.
    #[arg(long)]
    stall_after: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mode = match args.stall_after {
        Some(n) => SinkMode::StallAfter(n),
        None => SinkMode::Answer,
    };

    let sink = ChannelSink::start(&args.journal, mode, args.port).await?;

    // Printed on stdout and flushed so a shell harness can read the bound URL
    // before it starts the gateway. Ordering matters: a gateway started against
    // an unbound port would fail its sends for a reason that looks like a
    // product defect.
    println!(
        "SINK_READY url={} journal={}",
        sink.base_url(),
        args.journal.display()
    );
    use std::io::Write as _;
    std::io::stdout().flush()?;

    tokio::signal::ctrl_c().await?;
    sink.shutdown().await;
    Ok(())
}
