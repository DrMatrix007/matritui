use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::{Constraint, Layout, Rect};

use crate::ipc;
use crate::layout;
use crate::state;
use crate::transcript;
use crate::window::Window;

const TRANSCRIPT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

fn clamp_min_size(mut r: Rect) -> Rect {
    r.width = r.width.max(1);
    r.height = r.height.max(1);
    r
}

struct Workspace {
    path: PathBuf,
    windows: Vec<Window>,
    focused_window: usize,
}

impl Workspace {
    fn new(path: PathBuf, cmd: &str, with_hooks: bool, area: Rect) -> io::Result<Workspace> {
        let window = Window::new(cmd, with_hooks, area, &path)?;
        Ok(Workspace {
            path,
            windows: vec![window],
            focused_window: 0,
        })
    }

    fn new_window(&mut self, cmd: &str, with_hooks: bool, area: Rect) -> io::Result<()> {
        let window = Window::new(cmd, with_hooks, area, &self.path)?;
        self.windows.push(window);
        self.focused_window = self.windows.len() - 1;
        Ok(())
    }

    fn next_window(&mut self) {
        self.focused_window = (self.focused_window + 1) % self.windows.len();
    }

    fn prev_window(&mut self) {
        self.focused_window = (self.focused_window + self.windows.len() - 1) % self.windows.len();
    }

    fn select_window(&mut self, n: usize) {
        if n < self.windows.len() {
            self.focused_window = n;
        }
    }

    fn resize(&mut self, area: Rect) -> io::Result<()> {
        for window in &mut self.windows {
            window.resize(area)?;
        }
        Ok(())
    }

    fn reap_exited(&mut self) -> io::Result<bool> {
        for window in &mut self.windows {
            window.reap_exited()?;
        }

        let empty: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, window)| window.is_empty())
            .map(|(i, _)| i)
            .collect();
        for i in empty.into_iter().rev() {
            self.windows.remove(i);
        }

        self.focused_window = self.focused_window.min(self.windows.len().saturating_sub(1));
        Ok(self.windows.is_empty())
    }

    fn close_focused_pane(&mut self) -> io::Result<bool> {
        let now_empty = self.windows[self.focused_window].close_focused()?;
        if now_empty {
            self.windows.remove(self.focused_window);
            self.focused_window = self.focused_window.min(self.windows.len().saturating_sub(1));
        }
        Ok(self.windows.is_empty())
    }

    fn kill_all(&mut self) -> io::Result<()> {
        for window in &mut self.windows {
            window.kill_all()?;
        }
        Ok(())
    }

    fn window_names(&self) -> (Vec<String>, usize) {
        (
            (0..self.windows.len()).map(|i| i.to_string()).collect(),
            self.focused_window,
        )
    }

    fn focused_window(&self) -> &Window {
        &self.windows[self.focused_window]
    }

    fn focused_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.focused_window]
    }

    fn split_focused(&mut self, dir: layout::SplitDirection, cmd: &str, with_hooks: bool) -> io::Result<()> {
        let path = self.path.clone();
        self.focused_window_mut().split(dir, cmd, with_hooks, &path)
    }

    fn focus_pane(&mut self, window_idx: usize, pane_id: layout::PaneId) {
        if window_idx < self.windows.len() {
            self.focused_window = window_idx;
            self.windows[window_idx].focus_pane(pane_id);
        }
    }
}

pub struct App {
    workspaces: Vec<Workspace>,
    focused_workspace: usize,
    content_rect: Rect,
    pane_states: state::StateEngine,
    last_transcript_refresh: Option<Instant>,
}

impl App {
    pub fn new(cmd: &str, with_hooks: bool, area: Rect) -> io::Result<App> {
        let area = clamp_min_size(area);
        let path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workspace = Workspace::new(path, cmd, with_hooks, area)?;
        Ok(App {
            workspaces: vec![workspace],
            focused_workspace: 0,
            content_rect: area,
            pane_states: state::StateEngine::default(),
            last_transcript_refresh: None,
        })
    }

    pub fn new_workspace(&mut self, path: PathBuf, cmd: &str, with_hooks: bool) -> io::Result<()> {
        let workspace = Workspace::new(path, cmd, with_hooks, self.content_rect)?;
        self.workspaces.push(workspace);
        self.focused_workspace = self.workspaces.len() - 1;
        Ok(())
    }

    pub fn new_window(&mut self, cmd: &str, with_hooks: bool) -> io::Result<()> {
        self.workspaces[self.focused_workspace].new_window(cmd, with_hooks, self.content_rect)
    }

    pub fn next_window(&mut self) {
        self.workspaces[self.focused_workspace].next_window();
    }

    pub fn prev_window(&mut self) {
        self.workspaces[self.focused_workspace].prev_window();
    }

    pub fn select_window(&mut self, n: usize) {
        self.workspaces[self.focused_workspace].select_window(n);
    }

    pub fn resize(&mut self, full_area: Rect, sidebar_width: u16) -> io::Result<()> {
        let (left_area, _sidebar_area) = split_for_sidebar(full_area, sidebar_width);
        let areas = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(left_area);
        self.content_rect = clamp_min_size(areas[0]);
        for workspace in &mut self.workspaces {
            workspace.resize(self.content_rect)?;
        }
        Ok(())
    }

    pub fn reap_exited_panes(&mut self) -> io::Result<bool> {
        let mut empty = Vec::new();
        for (i, workspace) in self.workspaces.iter_mut().enumerate() {
            if workspace.reap_exited()? {
                empty.push(i);
            }
        }

        for i in empty.into_iter().rev() {
            self.workspaces.remove(i);
        }

        self.focused_workspace = self.focused_workspace.min(self.workspaces.len().saturating_sub(1));
        Ok(self.workspaces.is_empty())
    }

    pub fn close_focused_pane(&mut self) -> io::Result<bool> {
        let now_empty = self.workspaces[self.focused_workspace].close_focused_pane()?;
        if now_empty {
            self.workspaces.remove(self.focused_workspace);
            self.focused_workspace = self.focused_workspace.min(self.workspaces.len().saturating_sub(1));
        }
        Ok(self.workspaces.is_empty())
    }

    pub fn kill_all(&mut self) -> io::Result<()> {
        for workspace in &mut self.workspaces {
            workspace.kill_all()?;
        }
        Ok(())
    }

    pub fn window_names(&self) -> (Vec<String>, usize) {
        self.workspaces[self.focused_workspace].window_names()
    }

    pub fn focused_window(&self) -> &Window {
        self.workspaces[self.focused_workspace].focused_window()
    }

    pub fn focused_window_mut(&mut self) -> &mut Window {
        self.workspaces[self.focused_workspace].focused_window_mut()
    }

    pub fn split_focused(&mut self, dir: layout::SplitDirection, cmd: &str, with_hooks: bool) -> io::Result<()> {
        self.workspaces[self.focused_workspace].split_focused(dir, cmd, with_hooks)
    }

    pub fn record_hook_event(&mut self, event: ipc::HookEvent) {
        self.pane_states.record(&event);
    }

    pub fn pane_state(&self, pane_id: &str) -> &state::PaneState {
        self.pane_states.state_for(pane_id)
    }

    pub fn refresh_transcript_stats(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_transcript_refresh
            && now.duration_since(last) < TRANSCRIPT_REFRESH_INTERVAL
        {
            return;
        }
        self.last_transcript_refresh = Some(now);
        let root = transcript::transcripts_root();
        self.refresh_transcript_stats_with_root(&root);
    }

    fn refresh_transcript_stats_with_root(&mut self, root: &std::path::Path) {
        for workspace in &self.workspaces {
            for window in &workspace.windows {
                for (pane_id, _rect) in window.pane_rects() {
                    let Some(pane) = window.pane(pane_id) else { continue };
                    if !pane.is_agent() {
                        continue;
                    }
                    let Some(session_id) = self.pane_states.state_for(pane.hook_id()).session_id.clone() else {
                        continue;
                    };
                    let path = transcript::transcript_path(root, pane.cwd(), &session_id);
                    if let Ok(stats) = transcript::parse_transcript(&path) {
                        self.pane_states.set_transcript_stats(pane.hook_id(), stats);
                    }
                }
            }
        }
    }

    pub fn overview_entries(&self) -> Vec<OverviewEntry> {
        let mut entries = Vec::new();
        for (workspace_idx, workspace) in self.workspaces.iter().enumerate() {
            for (window_idx, window) in workspace.windows.iter().enumerate() {
                for (pane_id, _rect) in window.pane_rects() {
                    if let Some(pane) = window.pane(pane_id) {
                        entries.push(OverviewEntry {
                            workspace_idx,
                            window_idx,
                            pane_id,
                            cwd: pane.cwd().to_path_buf(),
                            status: self.pane_states.state_for(pane.hook_id()).status(),
                        });
                    }
                }
            }
        }
        entries.sort_by(|a, b| {
            (&a.cwd, a.workspace_idx, a.window_idx, a.pane_id).cmp(&(
                &b.cwd,
                b.workspace_idx,
                b.window_idx,
                b.pane_id,
            ))
        });
        entries
    }

    pub fn focus_pane(&mut self, workspace_idx: usize, window_idx: usize, pane_id: layout::PaneId) {
        if workspace_idx < self.workspaces.len() {
            self.focused_workspace = workspace_idx;
            self.workspaces[workspace_idx].focus_pane(window_idx, pane_id);
        }
    }
}

pub struct OverviewEntry {
    pub workspace_idx: usize,
    pub window_idx: usize,
    pub pane_id: layout::PaneId,
    pub cwd: PathBuf,
    pub status: state::PaneStatus,
}

pub fn split_for_sidebar(full_area: Rect, sidebar_width: u16) -> (Rect, Rect) {
    let areas = Layout::horizontal([Constraint::Min(0), Constraint::Length(sidebar_width)]).split(full_area);
    (areas[0], areas[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    #[test]
    fn new_window_increments_count_and_moves_focus() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.workspaces[app.focused_workspace].windows.len(), 1);
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 0);

        app.new_window("cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.workspaces[app.focused_workspace].windows.len(), 2);
        assert_eq!(
            app.workspaces[app.focused_workspace].focused_window, 1,
            "new_window should focus the newly created window"
        );

        app.kill_all().unwrap();
    }

    #[test]
    fn new_window_respects_with_hooks_flag() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let window = app.focused_window();
        let pane = window.pane(window.focused_id()).unwrap();
        assert!(pane.is_agent());

        app.kill_all().unwrap();
    }

    #[test]
    fn next_window_and_prev_window_wrap_correctly() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", false).unwrap();
        app.new_window("cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.workspaces[app.focused_workspace].windows.len(), 3);

        app.select_window(0);
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 0);

        app.next_window();
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 1);
        app.next_window();
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 2);
        app.next_window();
        assert_eq!(
            app.workspaces[app.focused_workspace].focused_window, 0,
            "next_window should wrap from the last index back to 0"
        );

        app.prev_window();
        assert_eq!(
            app.workspaces[app.focused_workspace].focused_window, 2,
            "prev_window should wrap from 0 to the last index, not underflow"
        );
        app.prev_window();
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 1);
        app.prev_window();
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 0);

        app.kill_all().unwrap();
    }

    #[test]
    fn select_window_ignores_out_of_range_index() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", false).unwrap();
        app.new_window("cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.workspaces[app.focused_workspace].windows.len(), 3);

        app.select_window(1);
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 1);

        app.select_window(99);
        assert_eq!(
            app.workspaces[app.focused_workspace].focused_window, 1,
            "select_window with an out-of-range index must be a no-op"
        );

        app.kill_all().unwrap();
    }

    #[test]
    fn resize_updates_content_rect_to_area_minus_status_line() {
        let initial = test_area();
        let mut app = App::new("cat", false, initial).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.content_rect, initial);

        let new_full_area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let expected_areas =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(new_full_area);
        let expected_content_rect = expected_areas[0];

        app.resize(new_full_area, 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.content_rect, expected_content_rect);
        assert_ne!(app.content_rect, initial);

        let window = app.focused_window();
        let pane = window.pane(window.focused_id()).unwrap();
        let (rows, cols) = pane.parser().lock().unwrap().screen().size();
        assert_eq!(rows, std::num::NonZeroU16::new(expected_content_rect.height).unwrap());
        assert_eq!(cols, std::num::NonZeroU16::new(expected_content_rect.width).unwrap());

        app.kill_all().unwrap();
    }

    #[test]
    fn resize_with_sidebar_width_narrows_content_rect() {
        let initial = test_area();
        let mut app = App::new("cat", false, initial).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let new_full_area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let sidebar_width = 40;

        let (left_area, _sidebar_area) = split_for_sidebar(new_full_area, sidebar_width);
        let expected_areas = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(left_area);
        let expected_content_rect = expected_areas[0];

        let no_sidebar_areas =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(new_full_area);
        let no_sidebar_content_rect = no_sidebar_areas[0];

        app.resize(new_full_area, sidebar_width).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.content_rect, expected_content_rect);
        assert!(
            app.content_rect.width < no_sidebar_content_rect.width,
            "content rect should be narrower when a sidebar is reserved"
        );

        let window = app.focused_window();
        let pane = window.pane(window.focused_id()).unwrap();
        let (rows, cols) = pane.parser().lock().unwrap().screen().size();
        assert_eq!(rows, std::num::NonZeroU16::new(expected_content_rect.height).unwrap());
        assert_eq!(cols, std::num::NonZeroU16::new(expected_content_rect.width).unwrap());

        app.kill_all().unwrap();
    }

    #[test]
    fn overview_entries_includes_every_pane_with_cwd_and_status() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let entries = app.overview_entries();

        assert_eq!(entries.len(), 2);
        let cwd = std::env::current_dir().unwrap();
        for entry in &entries {
            assert_eq!(entry.cwd, cwd);
            assert!(matches!(entry.status, state::PaneStatus::Idle));
        }
        let pairs: Vec<(usize, layout::PaneId)> =
            entries.iter().map(|e| (e.window_idx, e.pane_id)).collect();
        assert!(pairs.contains(&(0, 0)));
        assert!(pairs.contains(&(1, 0)));

        app.kill_all().unwrap();
    }

    #[test]
    fn overview_entries_sorted_by_window_then_pane() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", false).unwrap();
        app.new_window("cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let entries = app.overview_entries();
        let window_idxs: Vec<usize> = entries.iter().map(|e| e.window_idx).collect();
        let mut sorted = window_idxs.clone();
        sorted.sort();
        assert_eq!(window_idxs, sorted);

        app.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_switches_focused_window_and_pane() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", false).unwrap();
        app.workspaces[app.focused_workspace]
            .focused_window_mut()
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.select_window(0);
        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 0);

        app.focus_pane(0, 1, 1);

        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 1);
        assert_eq!(app.focused_window().focused_id(), 1);

        app.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_ignores_out_of_range_window_index() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.focus_pane(0, 99, 0);

        assert_eq!(app.workspaces[app.focused_workspace].focused_window, 0);
        assert_eq!(app.focused_window().focused_id(), 0);

        app.kill_all().unwrap();
    }

    #[test]
    fn new_workspace_creates_isolated_workspace_with_given_path() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let new_path = std::env::temp_dir();
        assert_ne!(new_path, std::env::current_dir().unwrap());

        app.new_workspace(new_path.clone(), "cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.workspaces.len(), 2);

        let window = app.workspaces[1].focused_window();
        let pane = window.pane(window.focused_id()).unwrap();
        assert_eq!(pane.cwd(), new_path);

        app.kill_all().unwrap();
    }

    #[test]
    fn new_workspace_auto_focuses_the_new_workspace() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        assert_eq!(app.focused_workspace, 0);

        app.new_workspace(std::env::temp_dir(), "cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.focused_workspace, 1);

        app.kill_all().unwrap();
    }

    #[test]
    fn overview_entries_group_by_distinct_workspace_paths() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        let other_path = std::env::temp_dir();
        app.new_workspace(other_path.clone(), "cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let entries = app.overview_entries();

        let distinct_cwds: std::collections::HashSet<_> = entries.iter().map(|e| e.cwd.clone()).collect();
        assert!(
            distinct_cwds.len() >= 2,
            "expected entries spanning at least 2 distinct workspace cwds, got {:?}",
            distinct_cwds
        );

        let original_cwd = std::env::current_dir().unwrap();
        for entry in &entries {
            if entry.cwd == original_cwd {
                assert_eq!(entry.workspace_idx, 0);
            } else if entry.cwd == other_path {
                assert_eq!(entry.workspace_idx, 1);
            }
        }

        app.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_with_explicit_workspace_index_switches_across_workspaces() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_workspace(std::env::temp_dir(), "cat", false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.focused_workspace, 1);

        app.focus_pane(0, 0, 0);

        assert_eq!(app.focused_workspace, 0);
        assert_eq!(app.workspaces[0].focused_window, 0);
        assert_eq!(app.focused_window().focused_id(), 0);

        app.kill_all().unwrap();
    }

    #[test]
    fn refresh_transcript_stats_populates_only_agent_panes_with_session_id() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", true).unwrap(); // agent pane, window 1
        std::thread::sleep(std::time::Duration::from_millis(200));

        let window = app.focused_window();
        let agent_pane_hook_id = window.pane(window.focused_id()).unwrap().hook_id().to_string();
        app.record_hook_event(ipc::HookEvent {
            pane_id: agent_pane_hook_id.clone(),
            event: serde_json::json!({"hook_event_name": "SessionStart", "session_id": "sess-abc"}),
        });

        // Also give the plain (non-agent) pane from window 0 a session_id, so that a
        // regression that drops the `is_agent()` gate (and relies on `session_id`
        // alone) would be caught too, not just a regression in the session_id gate.
        let plain_pane_hook_id = app.workspaces[app.focused_workspace].windows[0].pane(0).unwrap().hook_id().to_string();
        app.record_hook_event(ipc::HookEvent {
            pane_id: plain_pane_hook_id.clone(),
            event: serde_json::json!({"hook_event_name": "SessionStart", "session_id": "sess-plain"}),
        });

        let cwd = std::env::current_dir().unwrap();
        let root = std::env::temp_dir().join(format!("mmux-test-transcripts-root-{}", std::process::id()));
        let project_dir = root.join(transcript::encode_project_dir(&cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join("sess-abc.jsonl"),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":3,"output_tokens":4}}}"#,
        )
        .unwrap();
        std::fs::write(
            project_dir.join("sess-plain.jsonl"),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":99,"output_tokens":99}}}"#,
        )
        .unwrap();

        app.refresh_transcript_stats_with_root(&root);

        let stats = app.pane_state(&agent_pane_hook_id).transcript_stats.clone();
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.total_input_tokens, 3);
        assert_eq!(stats.total_output_tokens, 4);
        assert_eq!(stats.message_count, 1);

        assert!(
            app.pane_state(&plain_pane_hook_id).transcript_stats.is_none(),
            "a non-agent pane must stay None even when it has a session_id and a matching transcript file exists"
        );

        std::fs::remove_dir_all(&root).ok();
        app.kill_all().unwrap();
    }

    #[test]
    fn refresh_transcript_stats_leaves_agent_pane_without_session_id_as_none() {
        let mut app = App::new("cat", false, test_area()).unwrap();
        app.new_window("cat", true).unwrap(); // agent pane, no SessionStart ever recorded
        std::thread::sleep(std::time::Duration::from_millis(200));

        let window = app.focused_window();
        let agent_pane_hook_id = window.pane(window.focused_id()).unwrap().hook_id().to_string();

        let root = std::env::temp_dir().join(format!("mmux-test-transcripts-root-no-session-{}", std::process::id()));

        app.refresh_transcript_stats_with_root(&root);

        assert!(app.pane_state(&agent_pane_hook_id).transcript_stats.is_none());

        std::fs::remove_dir_all(&root).ok();
        app.kill_all().unwrap();
    }
}
