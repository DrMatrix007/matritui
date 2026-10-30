mod app;
mod config;
mod hook_forward;
mod hooks;
mod input;
mod ipc;
mod layout;
mod pane;
mod render;
mod state;
mod window;

use crossterm::event::{Event, KeyEventKind};

const SIDEBAR_WIDTH: u16 = 40;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("hook-forward") {
        hook_forward::run();
        return Ok(());
    }

    let config = config::Config::load();

    // Bind once, process-wide, before any panes are spawned, so pane processes' MMUX_IPC_SOCKET (set by hooks::pane_launch_extras from this same ipc::socket_name()) matches a socket we're actually listening on.
    let ipc_server = ipc::Server::bind(&ipc::socket_name())?;

    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let (cols, rows) = crossterm::terminal::size()?;
    let full_area = ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows };
    let content_area = ratatui::layout::Layout::vertical([
        ratatui::layout::Constraint::Min(0),
        ratatui::layout::Constraint::Length(1),
    ])
    .split(full_area)[0];
    let mut app = app::App::new("claude", content_area)?;

    let mut input_handler = input::InputHandler::with_config(config.keybindings);
    let mut awaiting_kill_confirm = false;
    let mut show_sidebar = false;
    let mut expanded_edit_rank: Option<usize> = None;

    loop {
        if app.reap_exited_panes()? {
            break;
        }

        while let Some(event) = ipc_server.try_recv() {
            app.record_hook_event(event);
        }

        if crossterm::event::poll(std::time::Duration::from_millis(33))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match input_handler.handle_key(key) {
                            input::Action::Forward(bytes) => {
                                app.focused_window_mut().write_to_focused(&bytes)?;
                            }
                            input::Action::NewWindow => {
                                app.new_window()?;
                            }
                            input::Action::NextWindow => {
                                app.next_window();
                            }
                            input::Action::PrevWindow => {
                                app.prev_window();
                            }
                            input::Action::SelectWindow(n) => {
                                app.select_window(n);
                            }
                            input::Action::SplitHorizontal => {
                                app.focused_window_mut().split(layout::SplitDirection::Horizontal, "claude")?;
                            }
                            input::Action::SplitVertical => {
                                app.focused_window_mut().split(layout::SplitDirection::Vertical, "claude")?;
                            }
                            input::Action::FocusPane(dir) => {
                                app.focused_window_mut().focus_direction(dir);
                            }
                            input::Action::KillPane => {
                                awaiting_kill_confirm = true;
                            }
                            input::Action::ConfirmKillPane => {
                                awaiting_kill_confirm = false;
                                if app.close_focused_pane()? {
                                    break;
                                }
                            }
                            input::Action::CancelKillPane => {
                                awaiting_kill_confirm = false;
                            }
                            input::Action::ToggleSidebar => {
                                show_sidebar = !show_sidebar;
                                expanded_edit_rank = None;
                                let (cols, rows) = crossterm::terminal::size()?;
                                let full_area = ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows };
                                app.resize(full_area, if show_sidebar { SIDEBAR_WIDTH } else { 0 })?;
                            }
                            input::Action::ExpandNextEdit => {
                                if show_sidebar
                                    && let Some(pane) = app.focused_window().pane(app.focused_window().focused_id())
                                {
                                    let len = app.pane_state(pane.hook_id()).edit_timeline.len();
                                    if len > 0 {
                                        expanded_edit_rank = match expanded_edit_rank {
                                            None => Some(0),
                                            Some(r) if r + 1 >= len => None,
                                            Some(r) => Some(r + 1),
                                        };
                                    }
                                }
                            }
                            // tmux's real Ctrl-b d is detach-client. Detach/reattach isn't implemented yet
                            // (single-process, deferred to a later milestone), so Ctrl-b d is repurposed here
                            // as "quit mmux entirely, killing all panes" -- the closest available match to
                            // tmux's semantics given that detach doesn't exist yet.
                            input::Action::Quit => {
                                break;
                            }
                            input::Action::None => {}
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    app.resize(
                        ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows },
                        if show_sidebar { SIDEBAR_WIDTH } else { 0 },
                    )?;
                }
                _ => {}
            }
        }

        terminal.draw(|frame| {
            let (left_area, sidebar_rect) =
                app::split_for_sidebar(frame.area(), if show_sidebar { SIDEBAR_WIDTH } else { 0 });
            let status_area = ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(left_area)[1];

            let window = app.focused_window();
            let rects = window.pane_rects();
            let draw_border = rects.len() >= 2;
            let focused_id = window.focused_id();
            for (id, rect) in &rects {
                if let Some(pane) = window.pane(*id) {
                    let guard = pane.parser().lock().unwrap();
                    render::render_pane(frame, *rect, guard.screen(), *id == focused_id, draw_border, &config.colors);
                }
            }

            let (names, focused_idx) = app.window_names();
            let prompt = if awaiting_kill_confirm {
                Some(format!("kill-pane {}? (y/n)", focused_id))
            } else {
                None
            };
            render::render_status_line(frame, status_area, &names, focused_idx, prompt.as_deref(), &config.colors);

            if show_sidebar
                && let Some(pane) = window.pane(focused_id)
            {
                render::render_sidebar(frame, sidebar_rect, app.pane_state(pane.hook_id()), expanded_edit_rank);
            }
        })?;
    }

    drop(_guard);
    app.kill_all()?;

    Ok(())
}
