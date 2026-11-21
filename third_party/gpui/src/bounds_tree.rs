use crate::{Bounds, Half, Point};
use std::{
    fmt::Debug,
    ops::{Add, Sub},
};

#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    root: Option<usize>,
    nodes: Vec<Node<U>>,
    stack: Vec<usize>,
    group_members: Vec<(TreeBounds<U>, u32)>,
}

impl<U> BoundsTree<U>
where
    U: Clone
        + Debug
        + PartialEq
        + PartialOrd
        + Add<U, Output = U>
        + Sub<Output = U>
        + Half
        + Default,
{
    pub fn clear(&mut self) {
        self.root = None;
        self.nodes.clear();
        self.stack.clear();
        self.group_members.clear();
    }

    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        self.insert_with_order(new_bounds, None)
    }

    /// Reserve an ordered interval for an already ordered replay subtree.
    /// Check arithmetic before insertion mutates bounds, nodes or traversal state.
    pub fn reserve(&mut self, bounds: Bounds<U>, span: u32) -> Option<u32> {
        let previous_max = self.root.map_or(0, |root| self.nodes[root].max_ordering());
        let base = previous_max.checked_add(1)?;
        let ceiling = base.checked_add(span)?;
        ceiling.checked_add(1)?; // Leave room for a following live insertion.
        self.insert_with_order(bounds, Some(ceiling));
        Some(base)
    }

    fn insert_with_order(&mut self, raw_bounds: Bounds<U>, reserved: Option<u32>) -> u32 {
        let bounds = TreeBounds::from_bounds(&raw_bounds);
        let Some(index) = self.placement_leaf(&bounds) else {
            let order = reserved.unwrap_or(1);
            let leaf = self.push_leaf(bounds, order);
            self.root = Some(leaf);
            return order;
        };

        let mut order = 0;
        if reserved.is_none() {
            order = self.find_order(index, &bounds, order);
            let mut child = index;
            for &parent in self.stack.iter().rev() {
                if let Node::Internal { left, right, .. } = self.nodes[parent] {
                    let other = if left == child { right } else { left };
                    order = self.find_order(other, &bounds, order);
                }
                child = parent;
            }
        }
        let order = reserved.unwrap_or_else(|| order + 1);
        if let Node::Leaf {
            bounds: old,
            order: stored,
        } = &mut self.nodes[index]
            && *old == bounds
        {
            *stored = (*stored).max(order);
        } else {
            let leaf = self.push_leaf(bounds, order);
            self.attach(leaf, index);
        }
        self.rebalance_placement();
        order
    }

    /// Insert consecutive clipped members with their ordinary painter orders.
    /// Only the balanced traversal entry is shared across the members.
    pub fn insert_group(&mut self, bounds: &[Bounds<U>], orders: &mut [u32]) -> Option<()> {
        if bounds.is_empty() || bounds.len() != orders.len() {
            return None;
        }
        let start = self.group_members.len();
        let mut envelope = TreeBounds::from_bounds(&bounds[0]);
        let mut max_order = 0;
        for (raw, order) in bounds.iter().zip(orders) {
            let rect = TreeBounds::from_bounds(raw);
            let mut local_max = 0;
            let mut covered_by_prior_member = false;
            for (member, rank) in &self.group_members[start..] {
                if *rank > local_max && rect.intersects(member) {
                    local_max = *rank;
                    covered_by_prior_member = covered_by_prior_member || member.contains(&rect);
                }
            }
            // The earlier containing member already follows every older leaf
            // that can overlap this rectangle. Require intersection as well:
            // rounded-flat bounds can contain each other without overlapping.
            let before = if covered_by_prior_member {
                local_max
            } else {
                self.root
                    .map_or(local_max, |root| self.find_order(root, &rect, local_max))
            };
            let Some(ranked) = before.checked_add(1) else {
                self.group_members.truncate(start);
                return None;
            };
            *order = ranked;
            max_order = max_order.max(ranked);
            envelope = envelope.union(&rect);
            self.group_members.push((rect, ranked));
        }
        self.group_members[start..].sort_unstable_by_key(|(_, order)| std::cmp::Reverse(*order));
        let end = self.group_members.len();
        let path = self.placement_leaf(&envelope);
        let leaf = self.nodes.len();
        self.nodes.push(Node::Group {
            bounds: envelope,
            start,
            end,
            max_order,
        });
        if let Some(index) = path {
            self.attach(leaf, index);
        } else {
            self.root = Some(leaf);
        }
        self.rebalance_placement();
        Some(())
    }

    fn placement_leaf(&mut self, bounds: &TreeBounds<U>) -> Option<usize> {
        let mut index = self.root?;
        while let Node::Internal { left, right, .. } = self.nodes[index] {
            self.stack.push(index);
            let left_cost = bounds.union(self.nodes[left].bounds()).half_perimeter();
            let right_cost = bounds.union(self.nodes[right].bounds()).half_perimeter();
            index = if left_cost < right_cost { left } else { right };
        }
        Some(index)
    }

    fn attach(&mut self, leaf: usize, index: usize) {
        let parent = self.push_internal(index, leaf);
        if let Some(&ancestor) = self.stack.last() {
            let Node::Internal { left, right, .. } = &mut self.nodes[ancestor] else {
                unreachable!()
            };
            if *left == index {
                *left = parent;
            } else {
                *right = parent;
            }
        } else {
            self.root = Some(parent);
        }
    }

    fn rebalance_placement(&mut self) {
        while let Some(parent) = self.stack.pop() {
            self.rebalance(parent);
        }
    }

    fn find_order(&self, index: usize, bounds: &TreeBounds<U>, mut order: u32) -> u32 {
        let node = &self.nodes[index];
        if order >= node.max_ordering() || !bounds.intersects(node.bounds()) {
            return order;
        }
        match node {
            Node::Leaf { order, .. } => *order,
            Node::Group { start, end, .. } => {
                for (member, member_order) in &self.group_members[*start..*end] {
                    if *member_order <= order {
                        break;
                    }
                    if bounds.intersects(member) {
                        return *member_order;
                    }
                }
                order
            }
            Node::Internal { left, right, .. } => {
                let left = *left;
                let right = *right;
                let (first, second) =
                    if self.nodes[left].max_ordering() > self.nodes[right].max_ordering() {
                        (left, right)
                    } else {
                        (right, left)
                    };
                order = self.find_order(first, bounds, order);
                if order < self.nodes[second].max_ordering() {
                    order = self.find_order(second, bounds, order);
                }
                order
            }
        }
    }

    fn push_leaf(&mut self, bounds: TreeBounds<U>, order: u32) -> usize {
        self.nodes.push(Node::Leaf { bounds, order });
        self.nodes.len() - 1
    }

    fn push_internal(&mut self, left: usize, right: usize) -> usize {
        self.nodes.push(self.internal(left, right));
        self.nodes.len() - 1
    }

    fn internal(&self, left: usize, right: usize) -> Node<U> {
        let a = &self.nodes[left];
        let b = &self.nodes[right];
        Node::Internal {
            left,
            right,
            bounds: a.bounds().union(b.bounds()),
            max_order: a.max_ordering().max(b.max_ordering()),
            height: 1 + a.height().max(b.height()),
        }
    }

    fn set_internal(&mut self, index: usize, left: usize, right: usize) {
        self.nodes[index] = self.internal(left, right);
    }

    fn children(&self, index: usize) -> (usize, usize) {
        match self.nodes[index] {
            Node::Internal { left, right, .. } => (left, right),
            _ => unreachable!(),
        }
    }

    fn rotate_left(&mut self, index: usize) {
        // Reuse both internal indices. The enclosing node still names index.
        let (left, right) = self.children(index);
        let (middle, far) = self.children(right);
        self.set_internal(right, left, middle);
        self.set_internal(index, right, far);
    }

    fn rotate_right(&mut self, index: usize) {
        let (left, right) = self.children(index);
        let (far, middle) = self.children(left);
        self.set_internal(left, middle, right);
        self.set_internal(index, far, left);
    }

    fn rebalance(&mut self, index: usize) {
        let (left, right) = self.children(index);
        let left_height = self.nodes[left].height();
        let right_height = self.nodes[right].height();
        if right_height > left_height + 1 {
            let (near, far) = self.children(right);
            if self.nodes[near].height() > self.nodes[far].height() {
                self.rotate_right(right);
            }
            self.rotate_left(index);
        } else if left_height > right_height + 1 {
            let (far, near) = self.children(left);
            if self.nodes[near].height() > self.nodes[far].height() {
                self.rotate_left(left);
            }
            self.rotate_right(index);
        } else {
            self.set_internal(index, left, right);
        }
    }
}

impl<U> Default for BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn default() -> Self {
        BoundsTree {
            root: None,
            nodes: Vec::new(),
            stack: Vec::new(),
            group_members: Vec::new(),
        }
    }
}

// Stored corners keep child coverage exact across rotations.
#[derive(Debug, Clone, PartialEq)]
struct TreeBounds<U: Clone + Debug + Default + PartialEq> {
    min: Point<U>,
    max: Point<U>,
}

impl<U: Clone + Debug + Default + PartialEq + PartialOrd + Add<Output = U> + Sub<Output = U>>
    TreeBounds<U>
{
    fn from_bounds(bounds: &Bounds<U>) -> Self {
        Self {
            min: bounds.origin.clone(),
            max: bounds.bottom_right(),
        }
    }
    fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(&other.min),
            max: self.max.max(&other.max),
        }
    }
    fn intersects(&self, other: &Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }
    fn contains(&self, other: &Self) -> bool {
        self.min.x <= other.min.x
            && self.min.y <= other.min.y
            && self.max.x >= other.max.x
            && self.max.y >= other.max.y
    }
    fn half_perimeter(&self) -> U {
        (self.max.x.clone() - self.min.x.clone()) + (self.max.y.clone() - self.min.y.clone())
    }
}

#[derive(Debug, Clone)]
enum Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    Leaf {
        bounds: TreeBounds<U>,
        order: u32,
    },
    Group {
        bounds: TreeBounds<U>,
        start: usize,
        end: usize,
        max_order: u32,
    },
    Internal {
        left: usize,
        right: usize,
        bounds: TreeBounds<U>,
        max_order: u32,
        height: usize,
    },
}

impl<U> Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn bounds(&self) -> &TreeBounds<U> {
        match self {
            Node::Leaf { bounds, .. } => bounds,
            Node::Group { bounds, .. } | Node::Internal { bounds, .. } => bounds,
        }
    }

    fn height(&self) -> usize {
        match self {
            Self::Leaf { .. } | Self::Group { .. } => 0,
            Self::Internal { height, .. } => *height,
        }
    }

    fn max_ordering(&self) -> u32 {
        match self {
            Node::Leaf {
                order: ordering, ..
            } => *ordering,
            Node::Internal { max_order, .. } | Node::Group { max_order, .. } => *max_order,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn verify_ordering() -> Result<(), String> {
    ordering_verification::verify()
}

// Std-only fixtures use Hex's existing locked test-support dependency graph.
#[cfg(any(test, feature = "test-support"))]
mod ordering_verification {
    use super::*;
    use crate::Size;
    use std::cmp;

    fn require(value: bool, message: &str) -> Result<(), String> {
        if value {
            Ok(())
        } else {
            Err(message.to_owned())
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<f32> {
        Bounds {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    pub(super) fn verify() -> Result<(), String> {
        // A higher-order child can miss the query entirely; tied maxima can
        // occur in disjoint children. Resolve both against an independent scan,
        // including an initial maximum that already satisfies either subtree.
        for orders in [[3, 7], [7, 3], [7, 7]] {
            let mut tree = BoundsTree::default();
            let leaves = [
                (rect(0.0, 0.0, 4.0, 4.0), orders[0]),
                (rect(20.0, 0.0, 4.0, 4.0), orders[1]),
            ];
            let left = tree.push_leaf(TreeBounds::from_bounds(&leaves[0].0), leaves[0].1);
            let right = tree.push_leaf(TreeBounds::from_bounds(&leaves[1].0), leaves[1].1);
            let root = tree.push_internal(left, right);
            for query in [
                rect(1.0, 1.0, 1.0, 1.0),
                rect(21.0, 1.0, 1.0, 1.0),
                rect(0.0, 0.0, 24.0, 4.0),
                rect(10.0, 1.0, 1.0, 1.0),
            ] {
                for initial in [0, 3, 7, 9] {
                    let expected = leaves
                        .iter()
                        .filter(|(bounds, _)| bounds.intersects(&query))
                        .map(|(_, order)| *order)
                        .fold(initial, cmp::max);
                    require(
                        tree.find_order(root, &TreeBounds::from_bounds(&query), initial)
                            == expected,
                        "sibling maxima query differs from independent leaf scan",
                    )?;
                }
            }
        }
        for scale in [1.0, 2.0] {
            for seed in 1..=24_u64 {
                let mut tree = BoundsTree::default();
                let mut leaves: Vec<(Bounds<f32>, u32)> = Vec::new();
                let mut random = seed;
                for i in 0..128 {
                    if i % 43 == 0 {
                        tree.clear();
                        leaves.clear();
                    }
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let x = ((random >> 16) % 512) as f32 * 0.25 - 64.0;
                    let y = ((random >> 32) % 256) as f32 * 0.5 - 64.0;
                    let bounds = if i % 7 == 0 {
                        rect((i % 16) as f32 * 4.0 * scale, 0.0, 4.0 * scale, 8.0 * scale)
                    } else {
                        rect(
                            x * scale,
                            y * scale,
                            (i % 9) as f32 * 0.5 * scale,
                            (i % 11) as f32 * 0.25 * scale,
                        )
                    };
                    let order = if i % 13 == 0 {
                        let base = leaves.iter().map(|(_, order)| *order).max().unwrap_or(0) + 1;
                        let span = (i % 5) as u32;
                        require(
                            tree.reserve(bounds, span) == Some(base),
                            "reserved draw-order interval differs from scan",
                        )?;
                        base + span
                    } else {
                        // Independent linear oracle; no tree topology/query is
                        // used to establish the expected maximum or next order.
                        let order = leaves
                            .iter()
                            .filter(|(b, _)| b.intersects(&bounds))
                            .map(|(_, order)| *order)
                            .max()
                            .unwrap_or(0)
                            + 1;
                        require(
                            tree.insert(bounds) == order,
                            "live draw order differs from linear scan",
                        )?;
                        order
                    };
                    leaves.push((bounds, order));
                }
            }
        }
        let a = rect(0.25, 1.0, 4.0, 2.0);
        let b = rect(3.25, 1.0, 4.0, 2.0);
        let separate = rect(12.0, 1.0, 4.0, 2.0);
        let mut repeated = BoundsTree::default();
        require(repeated.insert(a) == 1, "first repeated bounds")?;
        require(
            repeated.insert(b) == 2,
            "overlap precedes the repeated bounds",
        )?;
        for order in 3..=10 {
            require(repeated.insert(a) == order, "repeated bounds lost ordering")?;
        }
        require(repeated.reserve(a, 3) == Some(11), "repeated reserve order")?;
        require(
            repeated.insert(b) == 15,
            "later overlap lost reserved ceiling",
        )?;
        require(
            repeated.insert(separate) == 1,
            "disjoint geometry lost order reuse",
        )?;

        let mut flat_group = BoundsTree::default();
        let cover = rect(0.0, 0.0, 2.0, 1.0);
        let flat = rect(1.0, 0.0, f32::EPSILON / 4.0, 1.0);
        require(
            flat.intersects(&cover) && !flat.intersects(&flat),
            "expected a rounded-flat clip with an earlier overlap",
        )?;
        let mut orders = [0; 2];
        require(flat_group.insert(cover) == 1, "cover order")?;
        require(
            flat_group
                .insert_group(&[flat, flat], &mut orders)
                .is_some()
                && orders == [2, 2],
            "rounded-flat group member lost its earlier cover",
        )?;

        // An inset member may reuse its cover's ordering proof. A member
        // crossing outside the cover still needs an older-tree query.
        let mut contained_group = BoundsTree::default();
        let blocker = rect(1.0, 0.0, 2.0, 4.0);
        for order in 1..=3 {
            require(
                contained_group.insert(blocker) == order,
                "older overlap rank",
            )?;
        }
        let frame = rect(4.0, 0.0, 4.0, 4.0);
        let inset = rect(5.0, 1.0, 2.0, 2.0);
        let crossing = rect(2.0, 1.0, 4.0, 2.0);
        let mut orders = [0; 3];
        require(
            contained_group
                .insert_group(&[frame, inset, crossing], &mut orders)
                .is_some()
                && orders == [1, 2, 4],
            "contained group member changed the ordinary order of its crossing neighbor",
        )?;
        require(
            contained_group.insert(rect(5.5, 1.25, 1.0, 0.5)) == 5,
            "later draw lost the crossing member's rank",
        )?;

        // A union represented as origin + size may round its far edge inward.
        // Placement-path bounds must not introduce a new overlap-pruning test.
        let mut rounded = BoundsTree::default();
        require(
            rounded.insert(rect(0.1, 0.0, 0.18, 1.0)) == 1,
            "near leaf order",
        )?;
        require(
            rounded.insert(rect(-1_000_000.0, 0.0, 1.0, 1.0)) == 1,
            "far leaf order",
        )?;
        require(
            rounded.insert(rect(0.26, 0.0, 0.005, 1.0)) == 2,
            "rounded ancestor bounds hid an intersecting leaf",
        )?;
        // Adding a large origin back to a rounded union size can hide a
        // fractional intersection in a sibling subtree.
        let mut fractional = BoundsTree::default();
        let inputs = [
            (rect(131.0569, 13.0, 10.08135, 13.0), 1),
            (rect(141.13824, 13.0, 10.08135, 13.0), 1),
            (rect(383.0892, 0.0, 10.08135, 26.0), 1),
            (rect(383.0892, 13.0, 10.08135, 13.0), 2),
            (rect(393.17053, 0.0, 10.081087, 26.0), 3),
        ];
        for (bounds, expected) in inputs {
            require(
                fractional.insert(bounds) == expected,
                "fractional subtree overlap lost",
            )?;
        }
        // Strictly increasing clip positions used to grow a long placement
        // path. Preserve disjoint order reuse with logarithmic tree depth.
        let mut dense = BoundsTree::default();
        for column in 0..4_096 {
            let bounds = rect((2 * column) as f32, 0.0, 1.0, 8.0);
            require(dense.insert(bounds) == 1, "disjoint clip lost order reuse")?;
            require(
                dense.insert(bounds) == 2,
                "repeated clip lost its predecessor",
            )?;
        }
        let root = dense.root.ok_or("dense draw tree has no root")?;
        require(
            dense.nodes[root].height() <= 24,
            "dense draw tree grew a linear path",
        )?;

        let mut edge = BoundsTree::default();
        let bounds = rect(-2.0, 0.25, 8.0, 4.0);
        require(
            edge.reserve(bounds, u32::MAX - 2) == Some(1),
            "near-maximum reservation failed",
        )?;
        require(
            edge.reserve(bounds, 1).is_none(),
            "overflowing reservation was admitted",
        )?;
        require(
            edge.insert(bounds) == u32::MAX,
            "refusal changed following live order",
        )?;
        edge.clear();
        require(edge.insert(bounds) == 1, "clear retained old order state")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Point, Size};
    use rand::{Rng, SeedableRng};

    #[test]
    fn test_insert() {
        let mut tree = BoundsTree::<f32>::default();
        let bounds1 = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds2 = Bounds {
            origin: Point { x: 5.0, y: 5.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds3 = Bounds {
            origin: Point { x: 10.0, y: 10.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };

        // Insert the bounds into the tree and verify the order is correct
        assert_eq!(tree.insert(bounds1), 1);
        assert_eq!(tree.insert(bounds2), 2);
        assert_eq!(tree.insert(bounds3), 3);

        // Insert non-overlapping bounds and verify they can reuse orders
        let bounds4 = Bounds {
            origin: Point { x: 20.0, y: 20.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds5 = Bounds {
            origin: Point { x: 40.0, y: 40.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds6 = Bounds {
            origin: Point { x: 25.0, y: 25.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        assert_eq!(tree.insert(bounds4), 1); // bounds4 does not overlap with bounds1, bounds2, or bounds3
        assert_eq!(tree.insert(bounds5), 1); // bounds5 does not overlap with any other bounds
        assert_eq!(tree.insert(bounds6), 2); // bounds6 overlaps with bounds4, so it should have a different order
    }

    #[test]
    fn test_random_iterations() {
        let max_bounds = 100;
        for seed in 1..=1000 {
            // let seed = 44;
            let mut tree = BoundsTree::default();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
            let mut expected_quads: Vec<(Bounds<f32>, u32)> = Vec::new();

            // Insert a random number of random AABBs into the tree.
            let num_bounds = rng.random_range(1..=max_bounds);
            for _ in 0..num_bounds {
                let min_x: f32 = rng.random_range(-100.0..100.0);
                let min_y: f32 = rng.random_range(-100.0..100.0);
                let width: f32 = rng.random_range(0.0..50.0);
                let height: f32 = rng.random_range(0.0..50.0);
                let bounds = Bounds {
                    origin: Point { x: min_x, y: min_y },
                    size: Size { width, height },
                };

                let expected_ordering = expected_quads
                    .iter()
                    .filter_map(|quad| quad.0.intersects(&bounds).then_some(quad.1))
                    .max()
                    .unwrap_or(0)
                    + 1;
                expected_quads.push((bounds, expected_ordering));

                // Insert the AABB into the tree and collect intersections.
                let actual_ordering = tree.insert(bounds);
                assert_eq!(actual_ordering, expected_ordering);
            }
        }
    }
}
