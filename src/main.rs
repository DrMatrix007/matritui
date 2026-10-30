mod input;
mod pane;
mod render;

use pane::Pane;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let (cols, rows) = crossterm::terminal::size()?;
    let mut pane = Pane::spawn("claude", (rows, cols))?;

    loop {
        if pane.try_wait_exit()?.is_some() {
            break;
        }

        if crossterm::event::poll(std::time::Duration::from_millis(33))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
                        break;
                    }
                    if key.kind == KeyEventKind::Press {
                        let bytes = input::key_to_bytes(key);
                        pane.write_input(&bytes)?;
                    }
                }
                Event::Resize(cols, rows) => {
                    pane.resize(rows, cols)?;
                }
                _ => {}
            }
        }

        let guard = pane.parser().lock().unwrap();
        let screen = guard.screen();
        terminal.draw(|frame| {
            render::render_screen(frame, frame.area(), screen);
        })?;
        drop(guard);
    }

    drop(_guard);

    if pane.try_wait_exit()?.is_none() {
        let _ = pane.kill();
    }
    let _ = pane.wait();

    Ok(())
}
