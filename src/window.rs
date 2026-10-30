use std::collections::HashMap;
use std::io;

use ratatui::layout::Rect;

use crate::hooks;
use crate::layout;
use crate::pane;

pub struct Window {
    panes: HashMap<layout::PaneId, pane::Pane>,
    layout: layout::Node,
    focused: layout::PaneId,
    area: Rect,
    next_pane_id: layout::PaneId,
}

fn inset(r: Rect) -> Rect {
    Rect {
        x: r.x.saturating_add(1),
        y: r.y.saturating_add(1),
        width: r.width.saturating_sub(2).max(1),
        height: r.height.saturating_sub(2).max(1),
    }
}

impl Window {
    pub fn new(cmd: &str, area: Rect) -> io::Result<Window> {
        let hook_id = hooks::next_hook_id();
        let settings_path = hooks::write_settings_file(&hook_id)?;
        let (env, args) = hooks::pane_launch_extras(&hook_id, &settings_path);
        let pane = pane::Pane::spawn_with_hooks(cmd, (area.height, area.width), hook_id, settings_path, env, args)?;
        let mut panes = HashMap::new();
        panes.insert(0, pane);

        Ok(Window {
            panes,
            layout: layout::Node::Leaf(0),
            focused: 0,
            area,
            next_pane_id: 1,
        })
    }

    pub fn effective_rects(&self) -> Vec<(layout::PaneId, Rect)> {
        let raw = layout::compute_rects(&self.layout, self.area);
        if raw.len() < 2 {
            raw
        } else {
            raw.into_iter().map(|(id, r)| (id, inset(r))).collect()
        }
    }

    pub fn pane_rects(&self) -> Vec<(layout::PaneId, Rect)> {
        layout::compute_rects(&self.layout, self.area)
    }

    pub fn split(&mut self, dir: layout::SplitDirection, cmd: &str) -> io::Result<()> {
        let new_id = self.next_pane_id;
        self.next_pane_id += 1;

        layout::split_leaf(&mut self.layout, self.focused, new_id, dir);

        let rects = self.effective_rects();
        for (id, rect) in &rects {
            if *id == new_id {
                let hook_id = hooks::next_hook_id();
                let settings_path = hooks::write_settings_file(&hook_id)?;
                let (env, args) = hooks::pane_launch_extras(&hook_id, &settings_path);
                let pane = pane::Pane::spawn_with_hooks(cmd, (rect.height, rect.width), hook_id, settings_path, env, args)?;
                self.panes.insert(new_id, pane);
            } else if let Some(pane) = self.panes.get_mut(id) {
                pane.resize(rect.height, rect.width)?;
            }
        }

        self.focused = new_id;
        Ok(())
    }

    fn collapse_after_removal(&mut self, target: layout::PaneId) -> io::Result<bool> {
        let old_layout = std::mem::replace(&mut self.layout, layout::Node::Leaf(0));
        match layout::remove_leaf(old_layout, target) {
            None => Ok(true),
            Some(new_tree) => {
                self.layout = new_tree;
                let rects = self.effective_rects();
                self.focused = rects[0].0;
                for (id, rect) in &rects {
                    if let Some(pane) = self.panes.get_mut(id) {
                        pane.resize(rect.height, rect.width)?;
                    }
                }
                Ok(false)
            }
        }
    }

    pub fn close_focused(&mut self) -> io::Result<bool> {
        let target = self.focused;
        if let Some(pane) = self.panes.get_mut(&target) {
            pane.kill()?;
            pane.wait()?;
        }
        self.panes.remove(&target);

        self.collapse_after_removal(target)
    }

    pub fn focus_direction(&mut self, dir: layout::PaneDirection) {
        let rects = layout::compute_rects(&self.layout, self.area);
        if let Some(id) = layout::navigate(&rects, self.focused, dir) {
            self.focused = id;
        }
    }

    pub fn resize(&mut self, area: Rect) -> io::Result<()> {
        self.area = area;
        let rects = self.effective_rects();
        for (id, rect) in &rects {
            if let Some(pane) = self.panes.get_mut(id) {
                pane.resize(rect.height, rect.width)?;
            }
        }
        Ok(())
    }

    pub fn reap_exited(&mut self) -> io::Result<Vec<layout::PaneId>> {
        let mut exited = Vec::new();
        for (id, pane) in self.panes.iter_mut() {
            if pane.try_wait_exit()?.is_some() {
                exited.push(*id);
            }
        }

        for id in &exited {
            if let Some(pane) = self.panes.get_mut(id) {
                pane.wait()?;
            }
            self.panes.remove(id);
            self.collapse_after_removal(*id)?;
        }

        Ok(exited)
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    pub fn kill_all(&mut self) -> io::Result<()> {
        for pane in self.panes.values_mut() {
            let _ = pane.kill();
            let _ = pane.wait();
        }
        Ok(())
    }

    pub fn write_to_focused(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.panes.get_mut(&self.focused).unwrap().write_input(bytes)
    }

    pub fn focused_id(&self) -> layout::PaneId {
        self.focused
    }

    pub fn pane(&self, id: layout::PaneId) -> Option<&pane::Pane> {
        self.panes.get(&id)
    }

    pub fn focus_pane(&mut self, id: layout::PaneId) {
        if self.panes.contains_key(&id) {
            self.focused = id;
        }
    }
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

    fn rects_overlap(a: Rect, b: Rect) -> bool {
        !(a.x + a.width <= b.x
            || b.x + b.width <= a.x
            || a.y + a.height <= b.y
            || b.y + b.height <= a.y)
    }

    #[test]
    fn split_produces_non_overlapping_partitioning_rects() {
        let area = test_area();
        let mut window = Window::new("cat", area).unwrap();

        window.split(layout::SplitDirection::Horizontal, "cat").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(window.panes.len(), 2);

        let raw = layout::compute_rects(&window.layout, window.area);
        assert_eq!(raw.len(), 2);
        let raw_width_sum: u16 = raw.iter().map(|(_, r)| r.width).sum();
        assert_eq!(
            raw_width_sum, area.width,
            "a horizontal split must partition the full width before border inset"
        );

        let rects = window.effective_rects();
        assert_eq!(rects.len(), 2);
        let (_, r0) = rects[0];
        let (_, r1) = rects[1];

        assert!(
            !rects_overlap(r0, r1),
            "split panes must not overlap: {r0:?} vs {r1:?}"
        );

        // Each pane loses exactly 1 cell on every side to the border inset,
        // so the two widths together are short of the total by 2 cells per pane.
        assert_eq!(r0.width + r1.width, area.width - 4);
        assert_eq!(r0.height, area.height - 2);
        assert_eq!(r1.height, area.height - 2);

        window.kill_all().unwrap();
    }

    #[test]
    fn close_focused_on_two_pane_window_leaves_surviving_pane_focused() {
        let area = test_area();
        let mut window = Window::new("cat", area).unwrap();
        window.split(layout::SplitDirection::Horizontal, "cat").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        // split() leaves the newly created pane focused; the original pane (id 0)
        // is the one that should survive the close.
        assert_eq!(window.focused, 1);

        let result = window.close_focused().unwrap();

        assert_eq!(result, false);
        assert_eq!(window.panes.len(), 1);
        assert!(window.panes.contains_key(&0));
        assert_eq!(window.focused, 0);

        window.kill_all().unwrap();
    }

    #[test]
    fn close_focused_on_one_pane_window_reports_window_empty() {
        let area = test_area();
        let mut window = Window::new("cat", area).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let result = window.close_focused().unwrap();

        assert!(result);
        assert!(window.is_empty());
    }

    #[test]
    fn focus_pane_moves_focus_to_existing_pane_id() {
        let area = test_area();
        let mut window = Window::new("cat", area).unwrap();
        window.split(layout::SplitDirection::Horizontal, "cat").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(window.focused, 1);

        window.focus_pane(0);
        assert_eq!(window.focused, 0);

        window.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_ignores_unknown_pane_id() {
        let area = test_area();
        let mut window = Window::new("cat", area).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        window.focus_pane(99);
        assert_eq!(window.focused, 0);

        window.kill_all().unwrap();
    }
}
