//! v0.9.4 W4 — Fleet=100 AgentNav render performance bench (criterion).
//!
//! Parameterized over fleet sizes [50, 100, 200]. Each iteration:
//!   1. Builds N `SubAgentView` fixtures (static, built once per group).
//!   2. Mounts an `AgentNavSurface` via `on_enter`.
//!   3. Renders one frame to a ratatui `TestBackend` (120×24).
//!
//! Target: Fleet=100 median < 5ms. If exceeded, the finding is recorded in
//! the bench output and a code comment below marks it as a performance gap.
//!
//! Fixture constructor mirrors the `sub()` helper in agent_nav.rs tests:
//!   SubAgentView { id, name, status: SubAgentStatus::Running, turns: 0, tokens: 0, feed: vec![] }

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use wcore_cli::tui::app::{App, SubAgentStatus, SubAgentView};
use wcore_cli::tui::surfaces::Surface;
use wcore_cli::tui::surfaces::agent_nav::AgentNavSurface;
use wcore_cli::tui::theme::Theme;

/// Build N `SubAgentView` fixtures with `status: Running`.
/// Mirrors the `sub()` helper in agent_nav tests exactly.
fn make_fleet(n: usize) -> Vec<SubAgentView> {
    (0..n)
        .map(|i| SubAgentView {
            id: format!("agent-{i}"),
            name: format!("sub-agent-{i}"),
            status: SubAgentStatus::Running,
            turns: i % 10,
            tokens: (i * 100) as u64,
            feed: Vec::new(),
        })
        .collect()
}

fn bench_agent_nav_render(c: &mut Criterion) {
    let theme = Theme::hearth();
    let mut group = c.benchmark_group("agent_nav_render");

    for &fleet_size in &[50usize, 100, 200] {
        let agents = make_fleet(fleet_size);

        group.bench_with_input(
            BenchmarkId::new("Fleet", fleet_size),
            &agents,
            |b, agents| {
                b.iter(|| {
                    let mut app = App::new();
                    app.session.sub_agents = agents.clone();

                    let mut surface = AgentNavSurface::default();
                    surface.on_enter(&mut app);

                    let mut terminal =
                        Terminal::new(TestBackend::new(120, 24)).expect("test terminal");
                    terminal
                        .draw(|f| surface.render(f, f.area(), &app, &theme))
                        .expect("render");
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_agent_nav_render);
criterion_main!(benches);
