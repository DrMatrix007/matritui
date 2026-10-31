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
mod transcript;
mod window;

use std::path::PathBuf;

use crossterm::event::{Event, KeyEventKind};

const SIDEBAR_WIDTH: u16 = 40;
const AGENT_CMD: &str = "claude";

enum PendingConfirmUi {
    None,
    KillPane,
    Quit,
}

#[derive(PartialEq, Eq)]
enum SidebarMode {
    Hidden,
    Edits,
    Nav,
}

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
    let mut app = app::App::new(&config.default_shell, false, content_area)?;

    let mut input_handler = input::InputHandler::with_config(config.keybindings);
    let mut pending_confirm = PendingConfirmUi::None;
    let mut sidebar_mode = SidebarMode::Hidden;
    let mut expanded_edit_rank: Option<usize> = None;
    let mut show_overview = false;
    let mut overview_selected: usize = 0;
    let mut nav_selected: usize = 0;

    loop {
        if app.reap_exited_panes()? {
            break;
        }

        while let Some(event) = ipc_server.try_recv() {
            app.record_hook_event(event);
        }

        app.refresh_transcript_stats();

        app.focused_window_mut().tick_animation();

        if crossterm::event::poll(std::time::Duration::from_millis(33))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match input_handler.handle_key(key) {
                            input::Action::Forward(bytes) => {
                                app.focused_window_mut().write_to_focused(&bytes)?;
                            }
                            input::Action::NewWindow => {
                                app.new_window(&config.default_shell, false)?;
                            }
                            input::Action::NewWindowAgent => {
                                app.new_window(AGENT_CMD, true)?;
                            }
                            input::Action::StartNewWorkspace => {}
                            input::Action::CreateWorkspace(path) => {
                                app.new_workspace(PathBuf::from(path), &config.default_shell, false)?;
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
                                app.split_focused(layout::SplitDirection::Horizontal, &config.default_shell, false)?;
                            }
                            input::Action::SplitHorizontalAgent => {
                                app.split_focused(layout::SplitDirection::Horizontal, AGENT_CMD, true)?;
                            }
                            input::Action::SplitVertical => {
                                app.split_focused(layout::SplitDirection::Vertical, &config.default_shell, false)?;
                            }
                            input::Action::SplitVerticalAgent => {
                                app.split_focused(layout::SplitDirection::Vertical, AGENT_CMD, true)?;
                            }
                            input::Action::FocusPane(dir) => {
                                app.focused_window_mut().focus_direction(dir);
                            }
                            input::Action::KillPane => {
                                pending_confirm = PendingConfirmUi::KillPane;
                            }
                            input::Action::ConfirmKillPane => {
                                pending_confirm = PendingConfirmUi::None;
                                if app.close_focused_pane()? {
                                    break;
                                }
                            }
                            input::Action::CancelConfirm => {
                                pending_confirm = PendingConfirmUi::None;
                            }
                            input::Action::ToggleSidebar => {
                                sidebar_mode = match sidebar_mode {
                                    SidebarMode::Edits => SidebarMode::Hidden,
                                    SidebarMode::Hidden | SidebarMode::Nav => SidebarMode::Edits,
                                };
                                expanded_edit_rank = None;
                                let (cols, rows) = crossterm::terminal::size()?;
                                let full_area = ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows };
                                app.resize(full_area, if sidebar_mode == SidebarMode::Hidden { 0 } else { SIDEBAR_WIDTH })?;
                            }
                            input::Action::ToggleNavSidebar => {
                                sidebar_mode = match sidebar_mode {
                                    SidebarMode::Nav => SidebarMode::Hidden,
                                    SidebarMode::Hidden | SidebarMode::Edits => SidebarMode::Nav,
                                };
                                nav_selected = 0;
                                let (cols, rows) = crossterm::terminal::size()?;
                                let full_area = ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows };
                                app.resize(full_area, if sidebar_mode == SidebarMode::Hidden { 0 } else { SIDEBAR_WIDTH })?;
                            }
                            input::Action::NavUp => {
                                if sidebar_mode == SidebarMode::Nav {
                                    nav_selected = nav_selected.saturating_sub(1);
                                }
                            }
                            input::Action::NavDown => {
                                if sidebar_mode == SidebarMode::Nav {
                                    let len = app.overview_entries().len();
                                    if nav_selected + 1 < len {
                                        nav_selected += 1;
                                    }
                                }
                            }
                            input::Action::NavSelect => {
                                if sidebar_mode == SidebarMode::Nav
                                    && let Some(entry) = app.overview_entries().get(nav_selected)
                                {
                                    app.focus_pane(entry.workspace_idx, entry.window_idx, entry.pane_id);
                                }
                            }
                            input::Action::ExpandNextEdit => {
                                if sidebar_mode == SidebarMode::Edits
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
                            input::Action::ToggleOverview => {
                                show_overview = !show_overview;
                                overview_selected = 0;
                            }
                            input::Action::OverviewUp => {
                                overview_selected = overview_selected.saturating_sub(1);
                            }
                            input::Action::OverviewDown => {
                                let len = app.overview_entries().len();
                                if overview_selected + 1 < len {
                                    overview_selected += 1;
                                }
                            }
                            input::Action::OverviewSelect => {
                                if let Some(entry) = app.overview_entries().get(overview_selected) {
                                    app.focus_pane(entry.workspace_idx, entry.window_idx, entry.pane_id);
                                }
                                show_overview = false;
                            }
                            input::Action::Quit => {
                                pending_confirm = PendingConfirmUi::Quit;
                            }
                            input::Action::ConfirmQuit => {
                                break;
                            }
                            input::Action::None => {}
                        }
                    }
                }
                Event::Resize(cols, rows) => {
                    app.resize(
                        ratatui::layout::Rect { x: 0, y: 0, width: cols, height: rows },
                        if sidebar_mode == SidebarMode::Hidden { 0 } else { SIDEBAR_WIDTH },
                    )?;
                }
                _ => {}
            }
        }

        let prompt = match pending_confirm {
            PendingConfirmUi::KillPane => {
                Some(format!("kill-pane {}? (y/n)", app.focused_window().focused_id()))
            }
            PendingConfirmUi::Quit => Some("quit mmux? (y/n)".to_string()),
            PendingConfirmUi::None => input_handler
                .pending_workspace_path()
                .map(|buffer| format!("new workspace path: {buffer}")),
        };

        terminal.draw(|frame| {
            if show_overview {
                let entries = app.overview_entries();
                render::render_overview(frame, frame.area(), &entries, overview_selected, "overview");
                return;
            }

            let (left_area, sidebar_rect) = app::split_for_sidebar(
                frame.area(),
                if sidebar_mode == SidebarMode::Hidden { 0 } else { SIDEBAR_WIDTH },
            );
            let status_area = ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(1),
            ])
            .split(left_area)[1];

            let window = app.focused_window();
            let rects = window.pane_rects_animated();
            let draw_border = rects.len() >= 2;
            let focused_id = window.focused_id();
            for (id, rect) in &rects {
                if let Some(pane) = window.pane(*id) {
                    let guard = pane.parser().lock().unwrap();
                    render::render_pane(frame, *rect, guard.screen(), *id == focused_id, draw_border, &config.colors);
                }
            }

            if let Some(ghost_rect) = window.closing_ghost_rect() {
                frame.render_widget(
                    ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL),
                    ghost_rect,
                );
            }

            let (names, focused_idx) = app.window_names();
            render::render_status_line(frame, status_area, &names, focused_idx, prompt.as_deref(), &config.colors);

            match sidebar_mode {
                SidebarMode::Edits => {
                    if let Some(pane) = window.pane(focused_id) {
                        render::render_sidebar(frame, sidebar_rect, app.pane_state(pane.hook_id()), expanded_edit_rank);
                    }
                }
                SidebarMode::Nav => {
                    let entries = app.overview_entries();
                    render::render_overview(frame, sidebar_rect, &entries, nav_selected, "nav");
                }
                SidebarMode::Hidden => {}
            }
        })?;
    }

    drop(_guard);
    app.kill_all()?;

    Ok(())
}
