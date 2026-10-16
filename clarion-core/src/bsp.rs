//! Persistent stable-ID BSP policy. Device payloads never enter this arena.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use crate::geometry::{Axis, Rect};
use crate::layout::{BspNode, LiveLayoutPolicy};

#[derive(Debug, Clone, Copy)]
pub struct BspArrangement {
    pub bounds: Rect,
    pub gap: f64,
}

pub const TOP: u32 = 1;
pub const BOTTOM: u32 = 2;
pub const LEFT: u32 = 4;
pub const RIGHT: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    Normal,
    Maximized,
    Fullscreen,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BspSnapshot<I> {
    pub topology: Option<BspNode<I>>,
    pub active: Option<I>,
    pub expanded: Vec<(I, PaneMode)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal<I> {
    pub sibling_focus: Option<I>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreError {
    TooDeep,
    InvalidRatio,
}

#[derive(Debug, Clone)]
enum Kind<I> {
    Leaf(I),
    Split {
        axis: Axis,
        ratio: f64,
        children: [usize; 2],
    },
}
#[derive(Debug, Clone)]
struct Node<I> {
    parent: Option<usize>,
    bounds: Rect,
    kind: Kind<I>,
}

/// Arena indices are private. Window identity is the only external addressing key.
#[derive(Debug, Clone)]
pub struct BspTree<I> {
    nodes: Vec<Option<Node<I>>>,
    free: Vec<usize>,
    root: Option<usize>,
    by_id: HashMap<I, usize>,
    order: Vec<I>,
    bounds: Rect,
    gap: f64,
    max_depth: usize,
    active: Option<I>,
    pending_split: Option<Axis>,
}

/// A split drag accumulates raw logical deltas; invalid samples never poison it.
#[derive(Debug, Clone, Copy)]
pub struct SplitResize {
    pub resize: crate::resize::Resize,
    last_delta: [f64; 2],
}

impl SplitResize {
    pub fn new(resize: crate::resize::Resize) -> Self {
        Self {
            resize,
            last_delta: [0., 0.],
        }
    }
    pub fn delta(&mut self, current: [f64; 2]) -> Option<[f64; 2]> {
        if !current.iter().all(|delta| delta.is_finite()) {
            return None;
        }
        let delta = [
            current[0] - self.last_delta[0],
            current[1] - self.last_delta[1],
        ];
        if !delta.iter().all(|delta| delta.is_finite()) {
            return None;
        }
        self.last_delta = current;
        Some(delta)
    }
}

impl<I: Clone + Eq + Hash> BspTree<I> {
    pub fn new(bounds: Rect, gap: f64, max_depth: usize) -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
            root: None,
            by_id: HashMap::new(),
            order: Vec::new(),
            bounds: bounds.sanitized(),
            gap,
            max_depth,
            active: None,
            pending_split: None,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }
    pub fn len(&self) -> usize {
        self.by_id.len()
    }
    pub fn contains(&self, id: &I) -> bool {
        self.by_id.contains_key(id)
    }
    pub fn ordered_ids(&self) -> &[I] {
        &self.order
    }
    pub fn first(&self) -> Option<&I> {
        self.order.first()
    }
    pub fn active(&self) -> Option<&I> {
        self.active.as_ref()
    }
    pub fn activate(&mut self, id: &I) -> bool {
        if !self.contains(id) {
            return false;
        }
        let changed = self.active.as_ref() != Some(id);
        self.active = Some(id.clone());
        changed
    }
    pub fn restore_focus(&mut self, preferred: Option<&I>) {
        self.active = preferred
            .filter(|id| self.contains(id))
            .cloned()
            .or_else(|| self.first().cloned());
    }
    pub fn focus_neighbor(&mut self, direction: Direction) -> bool {
        let Some(target) = self
            .active
            .as_ref()
            .and_then(|id| self.neighbor(id, direction))
        else {
            return false;
        };
        self.activate(&target)
    }
    pub fn focus_extreme(&mut self, direction: Direction) {
        if let Some(target) = self.extreme(direction) {
            self.activate(&target);
        }
    }
    pub fn toggle_split_direction(&mut self) {
        self.pending_split = Some(match self.pending_split {
            Some(Axis::Horizontal) => Axis::Vertical,
            Some(Axis::Vertical) | None => Axis::Horizontal,
        });
    }
    pub fn geometry(&self, id: &I) -> Option<Rect> {
        self.by_id.get(id).map(|key| self.node(*key).bounds)
    }
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.free.clear();
        self.root = None;
        self.by_id.clear();
        self.order.clear();
        self.active = None;
    }
    fn node(&self, key: usize) -> &Node<I> {
        self.nodes[key].as_ref().expect("live BSP node")
    }
    fn node_mut(&mut self, key: usize) -> &mut Node<I> {
        self.nodes[key].as_mut().expect("live BSP node")
    }
    fn allocate(&mut self, node: Node<I>) -> usize {
        if let Some(key) = self.free.pop() {
            self.nodes[key] = Some(node);
            key
        } else {
            let key = self.nodes.len();
            self.nodes.push(Some(node));
            key
        }
    }
    fn retire(&mut self, key: usize) {
        self.nodes[key] = None;
        self.free.push(key);
    }
    fn depth(&self, mut key: usize) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.node(key).parent {
            key = parent;
            depth += 1;
        }
        depth
    }
    fn rebuild_order(&mut self) {
        self.order.clear();
        let Some(root) = self.root else {
            return;
        };
        let mut stack = vec![root];
        while let Some(key) = stack.pop() {
            match &self.node(key).kind {
                Kind::Leaf(id) => self.order.push(id.clone()),
                Kind::Split { children, .. } => {
                    stack.push(children[1]);
                    stack.push(children[0]);
                }
            }
        }
    }
    fn split_target(&self, preferred: Option<&I>) -> Option<usize> {
        if let Some(key) = preferred.and_then(|id| self.by_id.get(id)).copied() {
            if self.depth(key) < self.max_depth {
                return Some(key);
            }
        }
        // In-order breaks equal-depth ties, preserving deterministic sibling focus.
        self.order
            .iter()
            .filter_map(|id| {
                let key = self.by_id[id];
                let depth = self.depth(key);
                (depth < self.max_depth).then_some((key, depth))
            })
            .min_by_key(|(_, depth)| *depth)
            .map(|(key, _)| key)
    }
    pub fn insertion_bounds(&self, preferred: Option<&I>) -> Option<Rect> {
        if self.is_empty() {
            Some(self.bounds)
        } else {
            self.split_target(preferred)
                .map(|key| self.node(key).bounds)
        }
    }
    /// Insert after the preferred pane; depth saturation uses the shallowest pane.
    pub fn insert(&mut self, id: I, preferred: Option<&I>, manual: Option<Axis>) -> bool {
        if self.contains(&id) {
            return false;
        }
        if self.root.is_none() {
            let key = self.allocate(Node {
                parent: None,
                bounds: self.bounds,
                kind: Kind::Leaf(id.clone()),
            });
            self.root = Some(key);
            self.by_id.insert(id.clone(), key);
            self.order.push(id);
            self.active = self.order.first().cloned();
            return true;
        }
        let Some(target) = self.split_target(preferred) else {
            return false;
        };
        let bounds = self.node(target).bounds;
        let manual = manual.or_else(|| self.pending_split.take());
        let old = match &self.node(target).kind {
            Kind::Leaf(id) => id.clone(),
            _ => unreachable!(),
        };
        let first = self.allocate(Node {
            parent: Some(target),
            bounds,
            kind: Kind::Leaf(old.clone()),
        });
        let second = self.allocate(Node {
            parent: Some(target),
            bounds,
            kind: Kind::Leaf(id.clone()),
        });
        let axis = manual.unwrap_or(if bounds.width >= bounds.height {
            Axis::Horizontal
        } else {
            Axis::Vertical
        });
        self.node_mut(target).kind = Kind::Split {
            axis,
            ratio: 0.5,
            children: [first, second],
        };
        self.by_id.insert(old, first);
        self.by_id.insert(id, second);
        // Only the new split needs evaluation for the next batched smart insertion.
        self.arrange_subtree(target, bounds);
        self.rebuild_order();
        true
    }
    fn first_in_subtree(&self, mut key: usize) -> I {
        loop {
            match &self.node(key).kind {
                Kind::Leaf(id) => return id.clone(),
                Kind::Split { children, .. } => key = children[0],
            }
        }
    }
    /// Remove and promote the sibling; returns its first pane as the focus target.
    pub fn remove(&mut self, id: &I) -> Option<Removal<I>> {
        let leaf = self.by_id.remove(id)?;
        let Some(parent) = self.node(leaf).parent else {
            self.retire(leaf);
            self.root = None;
            self.rebuild_order();
            self.active = None;
            return Some(Removal {
                sibling_focus: None,
            });
        };
        let (children, grandparent) = match self.node(parent) {
            Node {
                parent,
                kind: Kind::Split { children, .. },
                ..
            } => (*children, *parent),
            _ => unreachable!(),
        };
        let sibling = if children[0] == leaf {
            children[1]
        } else {
            children[0]
        };
        let focus = self.first_in_subtree(sibling);
        self.node_mut(sibling).parent = grandparent;
        if let Some(grandparent) = grandparent {
            if let Kind::Split { children, .. } = &mut self.node_mut(grandparent).kind {
                children[usize::from(children[1] == parent)] = sibling;
            }
        } else {
            self.root = Some(sibling);
        }
        self.retire(leaf);
        self.retire(parent);
        self.rebuild_order();
        if self.active.as_ref() == Some(id) {
            self.active = Some(focus.clone());
        }
        Some(Removal {
            sibling_focus: Some(focus),
        })
    }
    pub fn swap(&mut self, a: &I, b: &I) -> bool {
        if a == b {
            return false;
        }
        let (Some(&ka), Some(&kb)) = (self.by_id.get(a), self.by_id.get(b)) else {
            return false;
        };
        self.node_mut(ka).kind = Kind::Leaf(b.clone());
        self.node_mut(kb).kind = Kind::Leaf(a.clone());
        self.by_id.insert(a.clone(), kb);
        self.by_id.insert(b.clone(), ka);
        self.rebuild_order();
        true
    }
    pub fn neighbor(&self, active: &I, direction: Direction) -> Option<I> {
        let active = self.geometry(active)?;
        let mut best = None;
        let mut score = f64::MAX;
        for id in &self.order {
            let rect = self.geometry(id).unwrap();
            let dx = (active.x + active.width / 2.) - (rect.x + rect.width / 2.);
            let dy = (active.y + active.height / 2.) - (rect.y + rect.height / 2.);
            let matches = match direction {
                Direction::Left => dx > 10. && dy.abs() * 1.2 < dx,
                Direction::Right => dx < -10. && dy.abs() * 1.2 < -dx,
                Direction::Up => dy > 10. && dx.abs() * 1.2 < dy,
                Direction::Down => dy < -10. && dx.abs() * 1.2 < -dy,
            };
            let distance = dx * dx + dy * dy;
            if matches && distance < score {
                best = Some(id);
                score = distance;
            }
        }
        best.cloned()
    }
    pub fn extreme(&self, direction: Direction) -> Option<I> {
        let mut best = None;
        let mut score = f64::INFINITY;
        for id in &self.order {
            let rect = self.geometry(id).unwrap();
            let value = match direction {
                Direction::Left => rect.x,
                Direction::Right => -rect.right(),
                Direction::Up => rect.y,
                Direction::Down => -rect.bottom(),
            };
            if value < score {
                best = Some(id);
                score = value;
            }
        }
        best.cloned()
    }

    /// Consume each requested edge at the closest enclosing split facing that edge.
    pub fn resize(&mut self, id: &I, edges: u32, dx: f64, dy: f64) -> u32 {
        let Some(&leaf) = self.by_id.get(id) else {
            return 0;
        };
        if !dx.is_finite() || !dy.is_finite() {
            return 0;
        }
        let mut child = leaf;
        let mut applied = 0;
        while let Some(parent) = self.node(child).parent {
            let (axis, children, bounds) = match self.node(parent) {
                Node {
                    bounds,
                    kind: Kind::Split { axis, children, .. },
                    ..
                } => (*axis, *children, *bounds),
                _ => unreachable!(),
            };
            let first = children[0] == child;
            let edge = match (axis, first) {
                (Axis::Horizontal, true) => RIGHT,
                (Axis::Horizontal, false) => LEFT,
                (Axis::Vertical, true) => BOTTOM,
                (Axis::Vertical, false) => TOP,
            };
            if edges & !applied & edge != 0 {
                let (span, delta) = match axis {
                    Axis::Horizontal => (bounds.width, dx),
                    Axis::Vertical => (bounds.height, dy),
                };
                let gap = self.gap;
                if let Kind::Split { ratio, .. } = &mut self.node_mut(parent).kind {
                    *ratio = (*ratio + delta / (span - gap).max(1.)).clamp(0.1, 0.9);
                }
                applied |= edge;
            }
            child = parent;
        }
        applied
    }
    fn project(&self, key: usize, visit: &mut impl FnMut(&I, Rect)) -> BspNode<I> {
        let node = self.node(key);
        match &node.kind {
            Kind::Leaf(id) => {
                visit(id, node.bounds);
                BspNode::Leaf(id.clone())
            }
            Kind::Split {
                axis,
                ratio,
                children,
            } => BspNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(self.project(children[0], visit)),
                second: Box::new(self.project(children[1], visit)),
            },
        }
    }
    fn update_split_bounds(&mut self, key: usize) -> Rect {
        let bounds = match self.node(key).kind {
            Kind::Leaf(_) => self.node(key).bounds,
            Kind::Split { children, .. } => {
                let a = self.update_split_bounds(children[0]);
                let b = self.update_split_bounds(children[1]);
                a.union(b)
            }
        };
        self.node_mut(key).bounds = bounds;
        bounds
    }
    fn arrange_subtree(&mut self, root: usize, bounds: Rect) {
        self.arrange_node(root, bounds);
        self.update_split_bounds(root);
        self.node_mut(root).bounds = bounds.sanitized();
    }
    fn arrange_node(&mut self, root: usize, bounds: Rect) {
        let bounds = bounds.sanitized();
        self.node_mut(root).bounds = bounds;
        if let Kind::Split {
            axis,
            ratio,
            children,
        } = self.node(root).kind
        {
            let (first, second) = crate::geometry::split_rect(bounds, axis, ratio, self.gap);
            self.arrange_node(children[0], first);
            self.arrange_node(children[1], second);
        }
    }
    pub fn relayout(&mut self, bounds: Rect, gap: f64) {
        self.bounds = bounds.sanitized();
        self.gap = gap;
        if let Some(root) = self.root {
            self.arrange_subtree(root, self.bounds);
        }
    }
    pub fn topology(&self) -> Option<BspNode<I>> {
        self.root.map(|root| self.project(root, &mut |_, _| {}))
    }

    /// Restore mapped session IDs atomically. Missing/duplicate identities prune
    /// leaves and collapse their parent; validation failure preserves the old tree.
    pub fn restore_mapped<J>(
        &mut self,
        topology: Option<&BspNode<J>>,
        mut resolve: impl FnMut(&J) -> Option<I>,
    ) -> Result<(), RestoreError> {
        if let Some(topology) = topology {
            let mut pending = vec![(topology, 0)];
            while let Some((node, depth)) = pending.pop() {
                if depth > self.max_depth {
                    return Err(RestoreError::TooDeep);
                }
                if let BspNode::Split {
                    ratio,
                    first,
                    second,
                    ..
                } = node
                {
                    if !ratio.is_finite() || !(0.1..=0.9).contains(ratio) {
                        return Err(RestoreError::InvalidRatio);
                    }
                    pending.push((first, depth + 1));
                    pending.push((second, depth + 1));
                }
            }
        }
        fn build<I: Clone + Eq + Hash, J>(
            tree: &mut BspTree<I>,
            node: &BspNode<J>,
            resolve: &mut impl FnMut(&J) -> Option<I>,
            seen: &mut HashSet<I>,
        ) -> Option<usize> {
            match node {
                BspNode::Leaf(source) => {
                    let id = resolve(source)?;
                    if !seen.insert(id.clone()) {
                        return None;
                    }
                    let key = tree.allocate(Node {
                        parent: None,
                        bounds: Rect::default(),
                        kind: Kind::Leaf(id.clone()),
                    });
                    tree.by_id.insert(id, key);
                    Some(key)
                }
                BspNode::Split {
                    axis,
                    ratio,
                    first,
                    second,
                } => match (
                    build(tree, first, resolve, seen),
                    build(tree, second, resolve, seen),
                ) {
                    (Some(a), Some(b)) => {
                        let parent = tree.allocate(Node {
                            parent: None,
                            bounds: Rect::default(),
                            kind: Kind::Split {
                                axis: *axis,
                                ratio: *ratio,
                                children: [a, b],
                            },
                        });
                        tree.node_mut(a).parent = Some(parent);
                        tree.node_mut(b).parent = Some(parent);
                        Some(parent)
                    }
                    (node, None) | (None, node) => node,
                },
            }
        }
        let mut next = Self::new(self.bounds, self.gap, self.max_depth);
        next.root =
            topology.and_then(|root| build(&mut next, root, &mut resolve, &mut HashSet::new()));
        next.rebuild_order();
        next.relayout(self.bounds, self.gap);
        next.restore_focus(self.active.as_ref());
        next.pending_split = self.pending_split;
        *self = next;
        Ok(())
    }
}

impl<I: Clone + Eq + Hash> LiveLayoutPolicy<I> for BspTree<I> {
    type Request<'a>
        = BspArrangement
    where
        Self: 'a;

    fn arrange(&mut self, request: BspArrangement, mut emit: impl FnMut(&I, Rect)) {
        self.relayout(request.bounds, request.gap);
        for id in &self.order {
            emit(id, self.node(self.by_id[id]).bounds);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_is_owned_by_stable_ids_through_swap_remove_and_restore() {
        let mut tree = BspTree::new(Rect::new(0., 0., 1200., 800.), 16., 32);
        tree.insert(1, None, None);
        tree.insert(2, Some(&1), None);
        assert_eq!(tree.active(), Some(&1));
        tree.activate(&2);
        tree.swap(&1, &2);
        assert_eq!(tree.active(), Some(&2));
        tree.remove(&1);
        assert_eq!(tree.active(), Some(&2));
        tree.insert(3, Some(&2), None);
        tree.remove(&2);
        assert_eq!(tree.active(), Some(&3));
        tree.restore_mapped(Some(&BspNode::Leaf(4)), |id| Some(*id))
            .unwrap();
        assert_eq!(tree.active(), Some(&4));
        assert!(!tree.activate(&99));
        assert_eq!(tree.active(), Some(&4));
        tree.remove(&4);
        assert_eq!(tree.active(), None);
    }

    #[test]
    fn pending_manual_split_survives_first_tile_and_topology_restore() {
        let mut tree = BspTree::new(Rect::new(0., 0., 600., 1200.), 0., 32);
        tree.toggle_split_direction();
        tree.insert(1, None, None);
        tree.insert(2, Some(&1), None);
        assert!(matches!(
            tree.topology(),
            Some(BspNode::Split {
                axis: Axis::Horizontal,
                ..
            })
        ));
        tree.toggle_split_direction();
        tree.toggle_split_direction();
        let topology = tree.topology();
        tree.restore_mapped(topology.as_ref(), |id| Some(*id))
            .unwrap();
        tree.insert(3, Some(&2), None);
        let (_, two) = tree
            .geometry(&2)
            .map(|rect| (rect.width, rect.height))
            .unwrap();
        assert_eq!(two, 600.);
    }

    #[test]
    fn invalid_incremental_resize_samples_do_not_poison_the_next_delta() {
        let mut resize = SplitResize::new(crate::resize::Resize::new(
            7,
            crate::Size::new(400., 300.),
            RIGHT,
        ));
        assert_eq!(resize.delta([20., 10.]), Some([20., 10.]));
        assert_eq!(resize.delta([f64::NAN, 10.]), None);
        assert_eq!(resize.delta([30., 15.]), Some([10., 5.]));
    }
    fn tree(depth: usize) -> BspTree<usize> {
        BspTree::new(Rect::new(17.25, 11.5, 1001., 601.), 15., depth)
    }
    #[test]
    fn insertion_uses_preferred_until_depth_bound_then_first_shallowest() {
        let mut tree = tree(2);
        assert!(tree.insert(0, None, None));
        assert!(tree.insert(1, Some(&0), None));
        assert!(tree.insert(2, Some(&1), None));
        assert!(tree.insert(3, Some(&2), None));
        assert_eq!(tree.ordered_ids(), &[0, 3, 1, 2]);
        assert!(!tree.insert(4, Some(&2), None));
        assert!(!tree.insert(0, None, None));
    }
    #[test]
    fn removal_promotes_local_sibling_and_recycles_arena_without_losing_ids() {
        let mut tree = tree(32);
        for id in 0..6 {
            assert!(tree.insert(id, id.checked_sub(1).as_ref(), None));
        }
        assert_eq!(
            tree.remove(&5),
            Some(Removal {
                sibling_focus: Some(4)
            })
        );
        assert_eq!(
            tree.remove(&2),
            Some(Removal {
                sibling_focus: Some(3)
            })
        );
        assert!(tree.insert(7, Some(&4), None));
        assert_eq!(tree.ordered_ids(), &[0, 1, 3, 4, 7]);
        tree.relayout(Rect::new(0., 0., 800., 600.), 16.);
        assert!(tree.geometry(&7).unwrap().width >= 0.);
    }
    #[test]
    fn resize_consumes_nearest_matching_axes_and_swap_moves_identity_only() {
        let mut tree = tree(32);
        let bsp = BspNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.37,
            first: Box::new(BspNode::Split {
                axis: Axis::Vertical,
                ratio: 0.4,
                first: Box::new(BspNode::Leaf(0)),
                second: Box::new(BspNode::Leaf(1)),
            }),
            second: Box::new(BspNode::Leaf(2)),
        };
        tree.restore_mapped(Some(&bsp), |id| Some(*id)).unwrap();
        assert_eq!(tree.resize(&0, RIGHT | BOTTOM, 50., 30.), RIGHT | BOTTOM);
        tree.relayout(Rect::new(17.25, 11.5, 1001., 601.), 15.);
        assert_eq!(tree.geometry(&0), Some(Rect::new(17.25, 11.5, 415., 264.)));
        let a = tree.geometry(&0);
        let b = tree.geometry(&2);
        assert!(tree.swap(&0, &2));
        assert_eq!(tree.geometry(&0), b);
        assert_eq!(tree.geometry(&2), a);
    }
    #[test]
    fn restore_prunes_missing_and_alias_ids_but_invalid_input_is_atomic() {
        let mut tree = tree(32);
        tree.insert(9, None, None);
        let bsp = BspNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(BspNode::Leaf(0)),
            second: Box::new(BspNode::Leaf(1)),
        };
        tree.restore_mapped(Some(&bsp), |_| Some(7)).unwrap();
        assert_eq!(tree.ordered_ids(), &[7]);
        let invalid = BspNode::Split {
            axis: Axis::Horizontal,
            ratio: f64::NAN,
            first: Box::new(BspNode::Leaf(0)),
            second: Box::new(BspNode::Leaf(1)),
        };
        assert_eq!(
            tree.restore_mapped(Some(&invalid), |id| Some(*id)),
            Err(RestoreError::InvalidRatio)
        );
        assert_eq!(tree.ordered_ids(), &[7]);
    }
    #[test]
    fn over_depth_restore_preserves_existing_tree() {
        let mut tree = tree(2);
        assert!(tree.insert(99, None, None));
        let mut topology = BspNode::Leaf(0);
        for id in 1..4 {
            topology = BspNode::Split {
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(BspNode::Leaf(id)),
                second: Box::new(topology),
            };
        }
        assert_eq!(
            tree.restore_mapped(Some(&topology), |id| Some(*id)),
            Err(RestoreError::TooDeep)
        );
        assert_eq!(tree.ordered_ids(), &[99]);
    }

    #[test]
    fn cached_geometry_is_stable_until_explicit_mutation() {
        let mut tree = tree(32);
        tree.insert(0, None, None);
        tree.insert(1, Some(&0), None);
        let old = tree.geometry(&0);
        for _ in 0..100 {
            assert_eq!(tree.geometry(&0), old);
            assert_eq!(tree.ordered_ids(), &[0, 1]);
        }
    }
}
