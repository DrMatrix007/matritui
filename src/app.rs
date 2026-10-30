use std::io;
use std::path::PathBuf;

use ratatui::layout::{Constraint, Layout, Rect};

use crate::ipc;
use crate::layout;
use crate::state;
use crate::window::Window;

fn clamp_min_size(mut r: Rect) -> Rect {
    r.width = r.width.max(1);
    r.height = r.height.max(1);
    r
}

pub struct App {
    windows: Vec<Window>,
    focused_window: usize,
    content_rect: Rect,
    cmd: String,
    pane_states: state::StateEngine,
}

impl App {
    pub fn new(cmd: &str, area: Rect) -> io::Result<App> {
        let area = clamp_min_size(area);
        let window = Window::new(cmd, area)?;
        Ok(App {
            windows: vec![window],
            focused_window: 0,
            content_rect: area,
            cmd: cmd.to_string(),
            pane_states: state::StateEngine::default(),
        })
    }

    pub fn new_window(&mut self) -> io::Result<()> {
        let window = Window::new(&self.cmd, self.content_rect)?;
        self.windows.push(window);
        self.focused_window = self.windows.len() - 1;
        Ok(())
    }

    pub fn next_window(&mut self) {
        self.focused_window = (self.focused_window + 1) % self.windows.len();
    }

    pub fn prev_window(&mut self) {
        self.focused_window = (self.focused_window + self.windows.len() - 1) % self.windows.len();
    }

    pub fn select_window(&mut self, n: usize) {
        if n < self.windows.len() {
            self.focused_window = n;
        }
    }

    pub fn resize(&mut self, full_area: Rect, sidebar_width: u16) -> io::Result<()> {
        let (left_area, _sidebar_area) = split_for_sidebar(full_area, sidebar_width);
        let areas = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(left_area);
        self.content_rect = clamp_min_size(areas[0]);
        for window in &mut self.windows {
            window.resize(self.content_rect)?;
        }
        Ok(())
    }

    pub fn reap_exited_panes(&mut self) -> io::Result<bool> {
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

    pub fn close_focused_pane(&mut self) -> io::Result<bool> {
        let now_empty = self.windows[self.focused_window].close_focused()?;
        if now_empty {
            self.windows.remove(self.focused_window);
            self.focused_window = self.focused_window.min(self.windows.len().saturating_sub(1));
        }
        Ok(self.windows.is_empty())
    }

    pub fn kill_all(&mut self) -> io::Result<()> {
        for window in &mut self.windows {
            window.kill_all()?;
        }
        Ok(())
    }

    pub fn window_names(&self) -> (Vec<String>, usize) {
        (
            (0..self.windows.len()).map(|i| i.to_string()).collect(),
            self.focused_window,
        )
    }

    pub fn focused_window(&self) -> &Window {
        &self.windows[self.focused_window]
    }

    pub fn focused_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.focused_window]
    }

    pub fn record_hook_event(&mut self, event: ipc::HookEvent) {
        self.pane_states.record(&event);
    }

    pub fn pane_state(&self, pane_id: &str) -> &state::PaneState {
        self.pane_states.state_for(pane_id)
    }

    pub fn overview_entries(&self) -> Vec<OverviewEntry> {
        let mut entries = Vec::new();
        for (window_idx, window) in self.windows.iter().enumerate() {
            for (pane_id, _rect) in window.pane_rects() {
                if let Some(pane) = window.pane(pane_id) {
                    entries.push(OverviewEntry {
                        window_idx,
                        pane_id,
                        cwd: pane.cwd().to_path_buf(),
                        status: self.pane_states.state_for(pane.hook_id()).status(),
                    });
                }
            }
        }
        entries.sort_by(|a, b| (&a.cwd, a.window_idx, a.pane_id).cmp(&(&b.cwd, b.window_idx, b.pane_id)));
        entries
    }

    pub fn focus_pane(&mut self, window_idx: usize, pane_id: layout::PaneId) {
        if window_idx < self.windows.len() {
            self.focused_window = window_idx;
            self.windows[window_idx].focus_pane(pane_id);
        }
    }
}

pub struct OverviewEntry {
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
        let mut app = App::new("cat", test_area()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.windows.len(), 1);
        assert_eq!(app.focused_window, 0);

        app.new_window().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(app.windows.len(), 2);
        assert_eq!(
            app.focused_window, 1,
            "new_window should focus the newly created window"
        );

        app.kill_all().unwrap();
    }

    #[test]
    fn next_window_and_prev_window_wrap_correctly() {
        let mut app = App::new("cat", test_area()).unwrap();
        app.new_window().unwrap();
        app.new_window().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.windows.len(), 3);

        app.select_window(0);
        assert_eq!(app.focused_window, 0);

        app.next_window();
        assert_eq!(app.focused_window, 1);
        app.next_window();
        assert_eq!(app.focused_window, 2);
        app.next_window();
        assert_eq!(
            app.focused_window, 0,
            "next_window should wrap from the last index back to 0"
        );

        app.prev_window();
        assert_eq!(
            app.focused_window, 2,
            "prev_window should wrap from 0 to the last index, not underflow"
        );
        app.prev_window();
        assert_eq!(app.focused_window, 1);
        app.prev_window();
        assert_eq!(app.focused_window, 0);

        app.kill_all().unwrap();
    }

    #[test]
    fn select_window_ignores_out_of_range_index() {
        let mut app = App::new("cat", test_area()).unwrap();
        app.new_window().unwrap();
        app.new_window().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(app.windows.len(), 3);

        app.select_window(1);
        assert_eq!(app.focused_window, 1);

        app.select_window(99);
        assert_eq!(
            app.focused_window, 1,
            "select_window with an out-of-range index must be a no-op"
        );

        app.kill_all().unwrap();
    }

    #[test]
    fn resize_updates_content_rect_to_area_minus_status_line() {
        let initial = test_area();
        let mut app = App::new("cat", initial).unwrap();
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
        let mut app = App::new("cat", initial).unwrap();
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
        let mut app = App::new("cat", test_area()).unwrap();
        app.new_window().unwrap();
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
        let mut app = App::new("cat", test_area()).unwrap();
        app.new_window().unwrap();
        app.new_window().unwrap();
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
        let mut app = App::new("cat", test_area()).unwrap();
        app.new_window().unwrap();
        app.focused_window_mut().split(layout::SplitDirection::Horizontal, "cat").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.select_window(0);
        assert_eq!(app.focused_window, 0);

        app.focus_pane(1, 1);

        assert_eq!(app.focused_window, 1);
        assert_eq!(app.focused_window().focused_id(), 1);

        app.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_ignores_out_of_range_window_index() {
        let mut app = App::new("cat", test_area()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        app.focus_pane(99, 0);

        assert_eq!(app.focused_window, 0);
        assert_eq!(app.focused_window().focused_id(), 0);

        app.kill_all().unwrap();
    }
}
