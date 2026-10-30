// This project uses vt100 rather than termwiz for terminal emulation. vt100 is a
// small, purpose-built crate: feed it bytes via `Parser::process()` and read back a
// `Screen` snapshot, which is exactly what's needed to bridge a child process's raw
// terminal output into a ratatui `Buffer`. termwiz is wezterm's full terminal-emulator
// toolkit, with a much larger footprint and its own terminal/widget concepts that
// would be unused overhead here.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

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
) {
    if !draw_border {
        render_screen(frame, rect, screen);
        return;
    }

    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
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
) {
    if let Some(text) = kill_confirm_prompt {
        frame.render_widget(Paragraph::new(text), rect);
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
    frame.render_widget(Paragraph::new(line), rect);
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use ratatui::backend::{Backend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use ratatui::Terminal;

    use super::render_screen;

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
}
