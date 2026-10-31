use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::hooks;
use crate::layout;
use crate::pane;

const ANIM_DURATION: Duration = Duration::from_millis(150);

struct AnimTransition {
    old_rects: Vec<(layout::PaneId, Rect)>,
    new_rects: Vec<(layout::PaneId, Rect)>,
    closing_ghost_target: Option<Rect>,
    started: Instant,
}

pub struct Window {
    panes: HashMap<layout::PaneId, pane::Pane>,
    layout: layout::Node,
    focused: layout::PaneId,
    area: Rect,
    next_pane_id: layout::PaneId,
    active_anim: Option<AnimTransition>,
}

fn inset(r: Rect) -> Rect {
    Rect {
        x: r.x.saturating_add(1),
        y: r.y.saturating_add(1),
        width: r.width.saturating_sub(2).max(1),
        height: r.height.saturating_sub(2).max(1),
    }
}

fn sliver(r: Rect) -> Rect {
    Rect { x: r.x, y: r.y, width: 0, height: 0 }
}

fn ghost_target(old: Rect, info: layout::CollapseInfo) -> Rect {
    match (info.direction, info.removed_was_first) {
        (layout::SplitDirection::Horizontal, true) => Rect {
            x: old.x + old.width,
            y: old.y,
            width: 0,
            height: old.height,
        },
        (layout::SplitDirection::Horizontal, false) => Rect {
            x: old.x,
            y: old.y,
            width: 0,
            height: old.height,
        },
        (layout::SplitDirection::Vertical, true) => Rect {
            x: old.x,
            y: old.y + old.height,
            width: old.width,
            height: 0,
        },
        (layout::SplitDirection::Vertical, false) => Rect {
            x: old.x,
            y: old.y,
            width: old.width,
            height: 0,
        },
    }
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u16, y: u16| (x as f32 + (y as f32 - x as f32) * t).round().max(0.0) as u16;
    Rect {
        x: lerp(a.x, b.x),
        y: lerp(a.y, b.y),
        width: lerp(a.width, b.width),
        height: lerp(a.height, b.height),
    }
}

fn spawn_pane(cmd: &str, size: (u16, u16), with_hooks: bool, cwd: &Path) -> io::Result<pane::Pane> {
    if with_hooks {
        let hook_id = hooks::next_hook_id();
        let settings_path = hooks::write_settings_file(&hook_id)?;
        let (env, args) = hooks::pane_launch_extras(&hook_id, &settings_path);
        pane::Pane::spawn_with_hooks(cmd, size, hook_id, settings_path, env, args, cwd)
    } else {
        pane::Pane::spawn(cmd, size, cwd)
    }
}

impl Window {
    pub fn new(cmd: &str, with_hooks: bool, area: Rect, cwd: &Path) -> io::Result<Window> {
        let pane = spawn_pane(cmd, (area.height, area.width), with_hooks, cwd)?;
        let mut panes = HashMap::new();
        panes.insert(0, pane);

        Ok(Window {
            panes,
            layout: layout::Node::Leaf(0),
            focused: 0,
            area,
            next_pane_id: 1,
            active_anim: None,
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

    pub fn pane_rects_animated(&self) -> Vec<(layout::PaneId, Rect)> {
        let Some(anim) = &self.active_anim else {
            return self.pane_rects();
        };
        let elapsed = anim.started.elapsed();
        if elapsed >= ANIM_DURATION {
            return self.pane_rects();
        }
        let t = elapsed.as_secs_f32() / ANIM_DURATION.as_secs_f32();
        anim.new_rects
            .iter()
            .map(|(id, rect)| {
                let from = anim
                    .old_rects
                    .iter()
                    .find(|(old_id, _)| old_id == id)
                    .map(|(_, r)| *r)
                    .unwrap_or_else(|| sliver(*rect));
                (*id, lerp_rect(from, *rect, t))
            })
            .collect()
    }

    pub fn closing_ghost_rect(&self) -> Option<Rect> {
        let anim = self.active_anim.as_ref()?;
        let elapsed = anim.started.elapsed();
        if elapsed >= ANIM_DURATION {
            return None;
        }
        let t = elapsed.as_secs_f32() / ANIM_DURATION.as_secs_f32();
        let (_, rect) = anim.old_rects.iter().find(|(id, _)| !self.panes.contains_key(id))?;
        let target = anim.closing_ghost_target.unwrap_or_else(|| sliver(*rect));
        Some(lerp_rect(*rect, target, t))
    }

    pub fn tick_animation(&mut self) {
        if let Some(anim) = &self.active_anim
            && anim.started.elapsed() >= ANIM_DURATION
        {
            self.active_anim = None;
        }
    }

    pub fn split(
        &mut self,
        dir: layout::SplitDirection,
        cmd: &str,
        with_hooks: bool,
        cwd: &Path,
    ) -> io::Result<()> {
        let new_id = self.next_pane_id;
        self.next_pane_id += 1;

        let old_rects = self.pane_rects();
        layout::split_leaf(&mut self.layout, self.focused, new_id, dir);

        let rects = self.effective_rects();
        for (id, rect) in &rects {
            if *id == new_id {
                let pane = spawn_pane(cmd, (rect.height, rect.width), with_hooks, cwd)?;
                self.panes.insert(new_id, pane);
            } else if let Some(pane) = self.panes.get_mut(id) {
                pane.resize(rect.height, rect.width)?;
            }
        }

        self.active_anim = Some(AnimTransition {
            old_rects,
            new_rects: self.pane_rects(),
            closing_ghost_target: None,
            started: Instant::now(),
        });
        self.focused = new_id;
        Ok(())
    }

    fn collapse_after_removal(&mut self, target: layout::PaneId) -> io::Result<bool> {
        let old_rects = self.pane_rects();
        let old_layout = std::mem::replace(&mut self.layout, layout::Node::Leaf(0));
        match layout::remove_leaf(old_layout, target) {
            None => Ok(true),
            Some((new_tree, collapse_info)) => {
                self.layout = new_tree;
                let rects = self.effective_rects();
                self.focused = rects[0].0;
                for (id, rect) in &rects {
                    if let Some(pane) = self.panes.get_mut(id) {
                        pane.resize(rect.height, rect.width)?;
                    }
                }
                let removed_rect = old_rects.iter().find(|(id, _)| *id == target).map(|(_, r)| *r);
                let closing_ghost_target = removed_rect.zip(collapse_info).map(|(old, info)| ghost_target(old, info));
                self.active_anim = Some(AnimTransition {
                    old_rects,
                    new_rects: self.pane_rects(),
                    closing_ghost_target,
                    started: Instant::now(),
                });
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
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();

        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
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
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
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
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let result = window.close_focused().unwrap();

        assert!(result);
        assert!(window.is_empty());
    }

    #[test]
    fn focus_pane_moves_focus_to_existing_pane_id() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_eq!(window.focused, 1);

        window.focus_pane(0);
        assert_eq!(window.focused, 0);

        window.kill_all().unwrap();
    }

    #[test]
    fn focus_pane_ignores_unknown_pane_id() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        window.focus_pane(99);
        assert_eq!(window.focused, 0);

        window.kill_all().unwrap();
    }

    #[test]
    fn new_without_hooks_produces_plain_pane() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(!window.pane(0).unwrap().is_agent());

        window.kill_all().unwrap();
    }

    #[test]
    fn new_with_hooks_produces_agent_pane() {
        let area = test_area();
        let mut window = Window::new("cat", true, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(window.pane(0).unwrap().is_agent());

        window.kill_all().unwrap();
    }

    #[test]
    fn split_without_hooks_produces_plain_pane() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(!window.pane(1).unwrap().is_agent());

        window.kill_all().unwrap();
    }

    #[test]
    fn split_with_hooks_produces_agent_pane() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                true,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert!(window.pane(1).unwrap().is_agent());

        window.kill_all().unwrap();
    }

    #[test]
    fn lerp_rect_at_endpoints_and_midpoint() {
        let a = Rect { x: 0, y: 0, width: 0, height: 0 };
        let b = Rect { x: 10, y: 20, width: 40, height: 60 };

        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);

        let mid = lerp_rect(a, b, 0.5);
        assert!(mid.x > a.x && mid.x < b.x);
        assert!(mid.y > a.y && mid.y < b.y);
        assert!(mid.width > a.width && mid.width < b.width);
        assert!(mid.height > a.height && mid.height < b.height);
    }

    #[test]
    fn split_starts_animation_with_captured_old_and_new_rects() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let anim = window.active_anim.as_ref().unwrap();
        assert_eq!(anim.old_rects, vec![(0, area)]);
        assert_eq!(anim.new_rects.len(), 2);

        window.kill_all().unwrap();
    }

    #[test]
    fn tick_animation_clears_active_anim_once_duration_elapses() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        window.active_anim = Some(AnimTransition {
            old_rects: vec![(0, area)],
            new_rects: vec![(0, area)],
            closing_ghost_target: None,
            started: Instant::now() - ANIM_DURATION - Duration::from_millis(10),
        });

        window.tick_animation();
        assert!(window.active_anim.is_none());

        window.kill_all().unwrap();
    }

    #[test]
    fn closing_ghost_rect_present_then_absent_after_duration() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        window.close_focused().unwrap();

        assert!(window.closing_ghost_rect().is_some());

        window.active_anim.as_mut().unwrap().started = Instant::now() - ANIM_DURATION - Duration::from_millis(10);
        assert!(window.closing_ghost_rect().is_none());

        window.kill_all().unwrap();
    }

    #[test]
    fn closing_ghost_shrinks_only_width_for_horizontal_split() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Horizontal,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let closing_rect = window
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == window.focused)
            .unwrap()
            .1;

        window.close_focused().unwrap();
        window.active_anim.as_mut().unwrap().started = Instant::now() - ANIM_DURATION / 2;

        let ghost = window.closing_ghost_rect().unwrap();
        assert_eq!(ghost.height, closing_rect.height);
        assert!(ghost.width < closing_rect.width);

        window.kill_all().unwrap();
    }

    #[test]
    fn closing_ghost_shrinks_only_height_for_vertical_split() {
        let area = test_area();
        let mut window = Window::new("cat", false, area, &std::env::current_dir().unwrap()).unwrap();
        window
            .split(
                layout::SplitDirection::Vertical,
                "cat",
                false,
                &std::env::current_dir().unwrap(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let closing_rect = window
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == window.focused)
            .unwrap()
            .1;

        window.close_focused().unwrap();
        window.active_anim.as_mut().unwrap().started = Instant::now() - ANIM_DURATION / 2;

        let ghost = window.closing_ghost_rect().unwrap();
        assert_eq!(ghost.width, closing_rect.width);
        assert!(ghost.height < closing_rect.height);

        window.kill_all().unwrap();
    }
}
