//! Path-map tree widget — renders a `TreeModel`.
//!
//! FROZEN Wave-0 signature; T0.4 fills the body.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{TreeModel, TreeNode};
use crate::tui::theme::Theme;

/// Render a path-map tree (the workspace right-rail file tree).
///
/// Directories render in the brand accent with a `/` suffix; files in the
/// dimmed text color. Depth is shown with a two-space indent per level,
/// matching the mockup's `.tree` rail. An empty model renders a single
/// muted placeholder line.
///
/// FROZEN Wave-0 contract.
pub fn path_tree(f: &mut Frame, area: Rect, tree: &TreeModel, t: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    if tree.roots.is_empty() {
        lines.push(Line::from(Span::styled(
            "no files touched yet",
            Style::default().bg(t.surface).fg(t.text_muted),
        )));
    } else {
        for root in &tree.roots {
            push_node(root, 0, t, &mut lines);
        }
    }

    let para = Paragraph::new(lines).style(Style::default().bg(t.surface));
    f.render_widget(para, area);
}

/// Append `node` (and its children, depth-first) as styled `Line`s.
fn push_node(node: &TreeNode, depth: usize, t: &Theme, out: &mut Vec<Line<'static>>) {
    let indent = "  ".repeat(depth);
    let (label, style) = if node.is_dir {
        (
            format!("{indent}{}/", node.name),
            Style::default()
                .bg(t.surface)
                .fg(t.orange)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            format!("{indent}{}", node.name),
            Style::default().bg(t.surface).fg(t.text_dim),
        )
    };
    out.push(Line::from(Span::styled(label, style)));

    for child in &node.children {
        push_node(child, depth + 1, t, out);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::{TreeModel, TreeNode};
    use crate::tui::theme::Theme;

    fn file(name: &str) -> TreeNode {
        TreeNode {
            name: name.into(),
            is_dir: false,
            children: vec![],
        }
    }

    fn dir(name: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            name: name.into(),
            is_dir: true,
            children,
        }
    }

    fn render(tree: &TreeModel, t: &Theme, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| path_tree(f, f.area(), tree, t))
            .expect("render tree");
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn empty_tree_renders_a_placeholder() {
        let rows = render(&TreeModel::default(), &Theme::hearth(), 30, 3);
        assert!(rows[0].contains("no files touched yet"), "{rows:?}");
    }

    #[test]
    fn tree_renders_nested_dirs_and_files() {
        let tree = TreeModel {
            roots: vec![dir("src", vec![dir("auth", vec![file("anthropic.rs")])])],
        };
        let rows = render(&tree, &Theme::hearth(), 30, 4);
        assert!(rows[0].starts_with("src/"), "root dir: {:?}", rows[0]);
        assert!(rows[1].contains("auth/"), "nested dir: {:?}", rows[1]);
        assert!(rows[1].starts_with("  "), "depth indent: {:?}", rows[1]);
        assert!(
            rows[2].contains("anthropic.rs") && rows[2].starts_with("    "),
            "deep file: {:?}",
            rows[2]
        );
    }

    #[test]
    fn directories_use_the_accent_color() {
        let tree = TreeModel {
            roots: vec![dir("src", vec![file("main.rs")])],
        };
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).expect("test terminal");
        terminal
            .draw(|f| path_tree(f, f.area(), &tree, &t))
            .expect("render");
        let buf = terminal.backend().buffer();
        // The directory line's first glyph is painted in the accent.
        assert_eq!(buf[(0, 0)].fg, t.orange);
        // The file line's first glyph is the dimmed text color.
        assert_eq!(buf[(2, 1)].fg, t.text_dim);
    }
}
