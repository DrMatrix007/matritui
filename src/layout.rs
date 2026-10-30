use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub type PaneId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl From<SplitDirection> for Direction {
    fn from(direction: SplitDirection) -> Direction {
        match direction {
            SplitDirection::Horizontal => Direction::Horizontal,
            SplitDirection::Vertical => Direction::Vertical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

pub fn compute_rects(node: &Node, area: Rect) -> Vec<(PaneId, Rect)> {
    match node {
        Node::Leaf(id) => vec![(*id, area)],
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first_percent = (ratio.clamp(0.0, 1.0) * 100.0).round() as u16;
            let areas = Layout::new(
                Direction::from(*direction),
                [
                    Constraint::Percentage(first_percent),
                    Constraint::Percentage(100 - first_percent),
                ],
            )
            .split(area);

            let mut rects = compute_rects(first, areas[0]);
            rects.extend(compute_rects(second, areas[1]));
            rects
        }
    }
}

pub fn split_leaf(node: &mut Node, target: PaneId, new_id: PaneId, dir: SplitDirection) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            *node = Node::Split {
                direction: dir,
                ratio: 0.5,
                first: Box::new(Node::Leaf(target)),
                second: Box::new(Node::Leaf(new_id)),
            };
            true
        }
        Node::Leaf(_) => false,
        Node::Split { first, second, .. } => {
            split_leaf(first, target, new_id, dir) || split_leaf(second, target, new_id, dir)
        }
    }
}

pub fn remove_leaf(node: Node, target: PaneId) -> Option<Node> {
    match node {
        Node::Leaf(id) if id == target => None,
        Node::Leaf(_) => Some(node),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            if matches!(*first, Node::Leaf(id) if id == target) {
                return Some(*second);
            }
            if matches!(*second, Node::Leaf(id) if id == target) {
                return Some(*first);
            }

            let first = remove_leaf(*first, target)?;
            let second = remove_leaf(*second, target)?;
            Some(Node::Split {
                direction,
                ratio,
                first: Box::new(first),
                second: Box::new(second),
            })
        }
    }
}

fn is_beyond(from: Rect, candidate: Rect, dir: PaneDirection) -> bool {
    match dir {
        PaneDirection::Right => candidate.x >= from.x + from.width,
        PaneDirection::Left => candidate.x + candidate.width <= from.x,
        PaneDirection::Down => candidate.y >= from.y + from.height,
        PaneDirection::Up => candidate.y + candidate.height <= from.y,
    }
}

fn perpendicular_overlap(from: Rect, candidate: Rect, dir: PaneDirection) -> u16 {
    match dir {
        PaneDirection::Left | PaneDirection::Right => {
            let start = from.y.max(candidate.y);
            let end = (from.y + from.height).min(candidate.y + candidate.height);
            end.saturating_sub(start)
        }
        PaneDirection::Up | PaneDirection::Down => {
            let start = from.x.max(candidate.x);
            let end = (from.x + from.width).min(candidate.x + candidate.width);
            end.saturating_sub(start)
        }
    }
}

fn primary_axis_distance(from: Rect, candidate: Rect, dir: PaneDirection) -> u16 {
    match dir {
        PaneDirection::Right => candidate.x - (from.x + from.width),
        PaneDirection::Left => from.x - (candidate.x + candidate.width),
        PaneDirection::Down => candidate.y - (from.y + from.height),
        PaneDirection::Up => from.y - (candidate.y + candidate.height),
    }
}

pub fn navigate(rects: &[(PaneId, Rect)], from: PaneId, dir: PaneDirection) -> Option<PaneId> {
    let from_rect = rects.iter().find(|(id, _)| *id == from)?.1;

    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter(|(_, rect)| is_beyond(from_rect, *rect, dir))
        .max_by(|(_, a), (_, b)| {
            let overlap_order =
                perpendicular_overlap(from_rect, *a, dir).cmp(&perpendicular_overlap(from_rect, *b, dir));
            overlap_order.then_with(|| {
                primary_axis_distance(from_rect, *b, dir).cmp(&primary_axis_distance(from_rect, *a, dir))
            })
        })
        .map(|(id, _)| *id)
}
