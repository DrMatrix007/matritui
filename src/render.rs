// This project uses vt100 rather than termwiz for terminal emulation. vt100 is a
// small, purpose-built crate: feed it bytes via `Parser::process()` and read back a
// `Screen` snapshot, which is exactly what's needed to bridge a child process's raw
// terminal output into a ratatui `Buffer`. termwiz is wezterm's full terminal-emulator
// toolkit, with a much larger footprint and its own terminal/widget concepts that
// would be unused overhead here.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::OverviewEntry;
use crate::config::Colors;
use crate::state::{PaneState, PaneStatus, SubagentNode};

fn translate_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(n) => Some(Color::Indexed(n)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();
    if let Some(fg) = translate_color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = translate_color(cell.bgcolor()) {
        style = style.bg(bg);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

pub fn render_screen(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, screen: &vt100::Screen) {
    let (screen_rows, screen_cols) = screen.size();
    let rows = area.height.min(screen_rows.get());
    let cols = area.width.min(screen_cols.get());

    let buffer = frame.buffer_mut();
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let Some(buf_cell) = buffer.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            let contents = cell.contents();
            let symbol = if contents.is_empty() { " " } else { contents };
            buf_cell.set_symbol(symbol);
            buf_cell.set_style(cell_style(cell));
        }
    }

    if !screen.hide_cursor() {
        let (cursor_row, cursor_col) = screen.cursor_position();
        frame.set_cursor_position((area.x + cursor_col, area.y + cursor_row));
    }
}

pub fn render_pane(
    frame: &mut ratatui::Frame,
    rect: ratatui::layout::Rect,
    screen: &vt100::Screen,
    focused: bool,
    draw_border: bool,
    colors: &Colors,
) {
    if !draw_border {
        render_screen(frame, rect, screen);
        return;
    }

    let border_style = if focused {
        Style::default().fg(colors.focused_border)
    } else {
        Style::default().fg(colors.unfocused_border)
    };
    let block = Block::default().borders(Borders::ALL).border_style(border_style);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    render_screen(frame, inner, screen);
}

pub fn render_status_line(
    frame: &mut ratatui::Frame,
    rect: ratatui::layout::Rect,
    window_names: &[String],
    focused_idx: usize,
    kill_confirm_prompt: Option<&str>,
    colors: &Colors,
) {
    let mut style = Style::default();
    if let Some(fg) = colors.status_fg {
        style = style.fg(fg);
    }
    if let Some(bg) = colors.status_bg {
        style = style.bg(bg);
    }

    if let Some(text) = kill_confirm_prompt {
        frame.render_widget(Paragraph::new(text).style(style), rect);
        return;
    }

    let line = window_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            if idx == focused_idx {
                format!("{idx}:{name}*")
            } else {
                format!("{idx}:{name}")
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    frame.render_widget(Paragraph::new(line).style(style), rect);
}

fn status_description(status: &PaneStatus) -> String {
    match status {
        PaneStatus::Idle => "idle".to_string(),
        PaneStatus::RunningTool(name) => format!("running tool \"{name}\""),
        PaneStatus::RunningSubagent(label) => format!("running subagent \"{label}\""),
    }
}

fn subagent_node_label(node: &SubagentNode) -> String {
    let marker = if node.active { "▸" } else { "✓" };
    let mut label = format!("{marker} {}", node.agent_type);
    if let Some(description) = &node.description {
        label.push_str(&format!(": \"{description}\""));
    }
    if let Some(tool) = &node.current_tool {
        label.push_str(&format!(" [running: {tool}]"));
    }
    label
}

fn push_subagent_children_lines(lines: &mut Vec<String>, children: &[SubagentNode], prefix: &str) {
    let n = children.len();
    for (i, child) in children.iter().enumerate() {
        let is_last = i + 1 == n;
        let connector = if is_last { "└─ " } else { "├─ " };
        lines.push(format!("{prefix}{connector}{}", subagent_node_label(child)));
        let child_prefix = format!("{prefix}{}", if is_last { "   " } else { "│  " });
        push_subagent_children_lines(lines, &child.children, &child_prefix);
    }
}

fn push_subagent_tree_lines(lines: &mut Vec<String>, node: &SubagentNode) {
    lines.push(subagent_node_label(node));
    push_subagent_children_lines(lines, &node.children, "");
}

fn diff_lines(before: &str, after: &str) -> Vec<Line<'static>> {
    let diff = similar::TextDiff::from_lines(before, after);
    diff.iter_all_changes()
        .map(|change| {
            let (prefix, style) = match change.tag() {
                similar::ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
                similar::ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
                similar::ChangeTag::Equal => (" ", Style::default()),
            };
            let text = format!("{prefix}{}", change.as_str().unwrap_or_default().trim_end_matches('\n'));
            Line::from(Span::styled(text, style))
        })
        .collect()
}

// `expanded_rank` is newest-first: rank 0 is the most recently pushed entry (the
// last element of `edit_timeline`), rank 1 the one before it, and so on.
pub fn render_sidebar(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    pane_state: &PaneState,
    expanded_rank: Option<usize>,
) {
    let block = Block::default().borders(Borders::ALL).title("sidebar");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [tree_area, timeline_area] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);

    let status_line = format!("Status: {}", status_description(&pane_state.status()));

    let mut tree_lines = Vec::new();
    for node in &pane_state.root_subagents {
        push_subagent_tree_lines(&mut tree_lines, node);
    }
    let tree_text = if let Some(stats) = &pane_state.transcript_stats {
        format!(
            "{status_line}\ntokens: {}/{} in/out  cost: ${:.4}  messages: {}\n\n{}",
            stats.total_input_tokens,
            stats.total_output_tokens,
            stats.total_cost_usd,
            stats.message_count,
            tree_lines.join("\n")
        )
    } else {
        format!("{status_line}\n\n{}", tree_lines.join("\n"))
    };
    frame.render_widget(Paragraph::new(tree_text), tree_area);

    let mut timeline_lines: Vec<Line> = Vec::new();
    for (rank, entry) in pane_state.edit_timeline.iter().rev().enumerate() {
        if expanded_rank == Some(rank) {
            timeline_lines.push(Line::from(entry.file_path.clone()));
            timeline_lines.extend(diff_lines(entry.before.as_deref().unwrap_or(""), &entry.after));
        } else {
            timeline_lines.push(Line::from(format!(
                "{}  +{}/-{}",
                entry.file_path, entry.added, entry.removed
            )));
        }
    }
    frame.render_widget(Paragraph::new(timeline_lines), timeline_area);
}

pub fn render_overview(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    entries: &[OverviewEntry],
    selected: usize,
    title: &str,
) {
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let selected = selected.min(entries.len().saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    let mut last_cwd: Option<&std::path::PathBuf> = None;
    for (idx, entry) in entries.iter().enumerate() {
        if last_cwd != Some(&entry.cwd) {
            lines.push(Line::from(entry.cwd.display().to_string()));
            last_cwd = Some(&entry.cwd);
        }

        let text = format!(
            "  window {} pane {}  {}",
            entry.window_idx,
            entry.pane_id,
            status_description(&entry.status)
        );
        let line = Line::from(text);
        lines.push(if idx == selected {
            line.style(Style::default().add_modifier(Modifier::REVERSED))
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use ratatui::backend::{Backend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;

    use super::{render_overview, render_pane, render_screen, render_sidebar, render_status_line};
    use crate::app::OverviewEntry;
    use crate::config::Colors;
    use crate::ipc::HookEvent;
    use crate::state::{EditEntry, PaneState, PaneStatus, StateEngine, SubagentNode};

    #[test]
    fn renders_plain_text_with_default_style() {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(10).unwrap(),
            NonZeroU16::new(20).unwrap(),
            0,
        );
        parser.process(b"hello");

        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_screen(frame, frame.area(), parser.screen()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for (col, expected_char) in "hello".chars().enumerate() {
            let cell = buffer.cell((col as u16, 0)).unwrap();
            assert_eq!(cell.symbol(), expected_char.to_string());
            assert_eq!(cell.fg, Color::Reset);
            assert_eq!(cell.bg, Color::Reset);
            assert!(cell.modifier.is_empty());
        }
    }

    #[test]
    fn renders_combined_sgr_attributes() {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(10).unwrap(),
            NonZeroU16::new(20).unwrap(),
            0,
        );
        parser.process(b"\x1b[1;3;4;31;44mX");

        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_screen(frame, frame.area(), parser.screen()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let cell = buffer.cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "X");
        assert_eq!(cell.fg, Color::Indexed(1));
        assert_eq!(cell.bg, Color::Indexed(4));
        assert!(cell.modifier.contains(Modifier::BOLD));
        assert!(cell.modifier.contains(Modifier::ITALIC));
        assert!(cell.modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn positions_cursor_relative_to_area_offset() {
        let mut parser = vt100::Parser::new(
            NonZeroU16::new(10).unwrap(),
            NonZeroU16::new(20).unwrap(),
            0,
        );
        parser.process(b"\x1b[3;5H");

        let backend = TestBackend::new(25, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 2,
            y: 1,
            width: 20,
            height: 10,
        };
        terminal
            .draw(|frame| render_screen(frame, area, parser.screen()))
            .unwrap();

        let (cursor_row, cursor_col) = parser.screen().cursor_position();
        let cursor_position = terminal.backend_mut().get_cursor_position().unwrap();
        assert_eq!(cursor_position.x, area.x + cursor_col);
        assert_eq!(cursor_position.y, area.y + cursor_row);
    }

    #[test]
    fn blank_cells_are_never_rendered_with_empty_symbol() {
        let mut parser = vt100::Parser::new(NonZeroU16::new(5).unwrap(), NonZeroU16::new(10).unwrap(), 0);
        parser.process(b"hi");

        let backend = TestBackend::new(10, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_screen(frame, frame.area(), parser.screen()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for cell in buffer.content() {
            assert_ne!(cell.symbol(), "");
        }
    }

    #[test]
    fn render_pane_uses_configured_focused_border_color() {
        let parser = vt100::Parser::new(NonZeroU16::new(5).unwrap(), NonZeroU16::new(10).unwrap(), 0);
        let colors = Colors {
            focused_border: Color::Red,
            ..Colors::default()
        };

        let backend = TestBackend::new(10, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_pane(frame, frame.area(), parser.screen(), true, true, &colors))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let cell = buffer.cell((0, 0)).unwrap();
        assert_eq!(cell.fg, Color::Red);
    }

    #[test]
    fn render_status_line_applies_configured_colors() {
        let colors = Colors {
            status_fg: Some(Color::White),
            status_bg: Some(Color::Blue),
            ..Colors::default()
        };

        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_status_line(frame, frame.area(), &["bash".to_string()], 0, None, &colors)
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let cell = buffer.cell((0, 0)).unwrap();
        assert_eq!(cell.fg, Color::White);
        assert_eq!(cell.bg, Color::Blue);
    }

    #[test]
    fn render_sidebar_shows_compact_edit_stat_line() {
        let mut pane_state = PaneState::default();
        pane_state.edit_timeline.push(EditEntry {
            file_path: "foo.rs".to_string(),
            before: Some("a\n".to_string()),
            after: "b\n".to_string(),
            added: 1,
            removed: 1,
        });

        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &pane_state, None))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..20 {
            for x in 0..40 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }

        assert!(text.contains("foo.rs"));
        assert!(text.contains("+1/-1"));
    }

    #[test]
    fn render_sidebar_shows_running_tool_status() {
        let mut engine = StateEngine::default();
        engine.record(&HookEvent {
            pane_id: "pane1".to_string(),
            event: serde_json::json!({
                "hook_event_name": "PreToolUse",
                "tool_name": "Bash",
                "tool_use_id": "t1"
            }),
        });
        let pane_state = engine.state_for("pane1");

        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), pane_state, None))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..20 {
            for x in 0..40 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }

        assert!(text.contains("Bash"));
        assert!(text.contains("running"));
    }

    #[test]
    fn render_sidebar_expanding_entry_shows_diff_lines() {
        let mut pane_state = PaneState::default();
        pane_state.edit_timeline.push(EditEntry {
            file_path: "foo.rs".to_string(),
            before: Some("a\n".to_string()),
            after: "b\n".to_string(),
            added: 1,
            removed: 1,
        });

        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &pane_state, Some(0)))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut lines = Vec::new();
        for y in 0..20 {
            let mut line = String::new();
            for x in 0..40 {
                line.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            lines.push(line);
        }

        let inner_lines: Vec<&str> = lines
            .iter()
            .map(|l| l.trim_matches(|c: char| c == '│' || c == '┌' || c == '┐' || c == '└' || c == '┘' || c == '─'))
            .collect();

        assert!(inner_lines.iter().any(|l| l.trim_start().starts_with('+')));
        assert!(inner_lines.iter().any(|l| l.trim_start().starts_with('-')));
    }

    #[test]
    fn render_overview_groups_entries_by_cwd_and_shows_identifiers_and_status() {
        let cwd = std::path::PathBuf::from("/tmp/project");
        let entries = vec![
            OverviewEntry {
                workspace_idx: 0,
                window_idx: 0,
                pane_id: 0,
                cwd: cwd.clone(),
                status: PaneStatus::Idle,
            },
            OverviewEntry {
                workspace_idx: 0,
                window_idx: 1,
                pane_id: 0,
                cwd: cwd.clone(),
                status: PaneStatus::RunningTool("Bash".to_string()),
            },
        ];

        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_overview(frame, frame.area(), &entries, 0, "overview"))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..20 {
            for x in 0..60 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }

        assert_eq!(text.matches("/tmp/project").count(), 1);
        assert!(text.contains("window 0 pane 0"));
        assert!(text.contains("window 1 pane 0"));
        assert!(text.contains("idle"));
        assert!(text.contains("running tool \"Bash\""));
    }

    #[test]
    fn render_overview_shows_custom_title() {
        let entries = vec![OverviewEntry {
            workspace_idx: 0,
            window_idx: 0,
            pane_id: 0,
            cwd: std::path::PathBuf::from("/tmp/project"),
            status: PaneStatus::Idle,
        }];

        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_overview(frame, frame.area(), &entries, 0, "nav"))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..20 {
            for x in 0..60 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }

        assert!(text.contains("nav"));
    }

    #[test]
    fn render_overview_highlights_selected_row() {
        let cwd = std::path::PathBuf::from("/tmp/project");
        let entries = vec![
            OverviewEntry {
                workspace_idx: 0,
                window_idx: 0,
                pane_id: 0,
                cwd: cwd.clone(),
                status: PaneStatus::Idle,
            },
            OverviewEntry {
                workspace_idx: 0,
                window_idx: 1,
                pane_id: 0,
                cwd: cwd.clone(),
                status: PaneStatus::Idle,
            },
        ];

        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_overview(frame, frame.area(), &entries, 1, "overview"))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row_with_window_1 = (0..20)
            .find(|&y| {
                let mut line = String::new();
                for x in 0..60 {
                    line.push_str(buffer.cell((x, y)).unwrap().symbol());
                }
                line.contains("window 1 pane 0")
            })
            .unwrap();
        let row_with_window_0 = (0..20)
            .find(|&y| {
                let mut line = String::new();
                for x in 0..60 {
                    line.push_str(buffer.cell((x, y)).unwrap().symbol());
                }
                line.contains("window 0 pane 0")
            })
            .unwrap();

        let selected_cell = buffer.cell((2, row_with_window_1)).unwrap();
        let unselected_cell = buffer.cell((2, row_with_window_0)).unwrap();
        assert!(selected_cell.modifier.contains(Modifier::REVERSED));
        assert!(!unselected_cell.modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn render_sidebar_shows_tree_connectors_for_multi_level_tree() {
        let mut pane_state = PaneState::default();
        pane_state.root_subagents.push(SubagentNode {
            agent_id: "a1".to_string(),
            agent_type: "Explore".to_string(),
            description: None,
            active: true,
            current_tool: None,
            children: vec![
                SubagentNode {
                    agent_id: "b1".to_string(),
                    agent_type: "Task".to_string(),
                    description: None,
                    active: false,
                    current_tool: None,
                    children: vec![SubagentNode {
                        agent_id: "c1".to_string(),
                        agent_type: "Bash".to_string(),
                        description: None,
                        active: false,
                        current_tool: None,
                        children: vec![],
                    }],
                },
                SubagentNode {
                    agent_id: "b2".to_string(),
                    agent_type: "Task".to_string(),
                    description: None,
                    active: false,
                    current_tool: None,
                    children: vec![],
                },
            ],
        });

        let backend = TestBackend::new(40, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &pane_state, None))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..20 {
            for x in 0..40 {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }

        assert!(text.contains("├─"));
        assert!(text.contains("└─"));
        assert!(text.contains("│"));
        assert!(text.contains("▸ Explore"));
        assert!(text.contains("✓ Task"));
    }
}
