use crate::{Bounds, Half};
use std::{
    cmp,
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

    fn insert_with_order(&mut self, new_bounds: Bounds<U>, reserved: Option<u32>) -> u32 {
        // If the tree is empty, make the root the new leaf.
        let Some(root) = self.root else {
            let order = reserved.unwrap_or(1);
            let new_node = self.push_leaf(new_bounds, order);
            self.root = Some(new_node);
            return order;
        };

        // Search for the best place to add the new leaf based on heuristics.
        let mut index = root;
        while let Node::Internal {
            left,
            right,
            bounds: node_bounds,
            ..
        } = &mut self.nodes[index]
        {
            let left = *left;
            let right = *right;
            *node_bounds = node_bounds.union(&new_bounds);
            self.stack.push(index);

            // Descend to the best-fit child, based on which one would increase
            // the surface area the least. This attempts to keep the tree balanced
            // in terms of surface area. Draw order is resolved after descent.
            let left_cost = new_bounds.union(self.nodes[left].bounds()).half_perimeter();
            let right_cost = new_bounds
                .union(self.nodes[right].bounds())
                .half_perimeter();
            if left_cost < right_cost {
                index = left;
            } else {
                index = right;
            }
        }

        // We've found a leaf ('index' now refers to a leaf node).
        // We'll insert a new parent node above the leaf and attach our new leaf to it.
        let sibling = index;

        // Resolve the nearby leaf first, then the same sibling subtrees as the
        // original descent, from nearest to farthest. A useful high order can
        // prune older branches sooner. Do not test placement-path bounds: their
        // origin + size representation may round a union edge inward.
        let mut max_intersecting_ordering = 0;
        if reserved.is_none() {
            max_intersecting_ordering = self.find_max_ordering(sibling, &new_bounds, 0);
            let mut child = sibling;
            for &parent in self.stack.iter().rev() {
                if let Node::Internal { left, right, .. } = &self.nodes[parent] {
                    let other = if *left == child { *right } else { *left };
                    max_intersecting_ordering =
                        self.find_max_ordering(other, &new_bounds, max_intersecting_ordering);
                }
                child = parent;
            }
        }

        let ordering = reserved.unwrap_or_else(|| max_intersecting_ordering + 1);
        // Equal rectangles intersect exactly the same future queries. Keep
        // their maximum order in one leaf without removing any scene primitive.
        if let Node::Leaf { bounds, order } = &mut self.nodes[sibling]
            && *bounds == new_bounds
        {
            *order = (*order).max(ordering);
        } else {
            let new_node = self.push_leaf(new_bounds, ordering);
            let new_parent = self.push_internal(sibling, new_node);

            // If there was an old parent, we need to update its children indices.
            if let Some(old_parent) = self.stack.last().copied() {
                let Node::Internal { left, right, .. } = &mut self.nodes[old_parent] else {
                    unreachable!();
                };

                if *left == sibling {
                    *left = new_parent;
                } else {
                    *right = new_parent;
                }
            } else {
                // If the old parent was the root, the new parent is the new root.
                self.root = Some(new_parent);
            }
        }

        for node_index in self.stack.drain(..).rev() {
            let Node::Internal {
                max_order: max_ordering,
                ..
            } = &mut self.nodes[node_index]
            else {
                unreachable!()
            };
            if *max_ordering >= ordering {
                break;
            }
            *max_ordering = ordering;
        }

        ordering
    }

    fn find_max_ordering(&self, index: usize, bounds: &Bounds<U>, mut max_ordering: u32) -> u32 {
        match &self.nodes[index] {
            Node::Leaf {
                bounds: node_bounds,
                order: ordering,
                ..
            } => {
                if max_ordering < *ordering && bounds.intersects(node_bounds) {
                    max_ordering = *ordering;
                }
            }
            Node::Internal {
                left,
                right,
                bounds: node_bounds,
                max_order: node_max_ordering,
                ..
            } => {
                if max_ordering < *node_max_ordering && bounds.intersects(node_bounds) {
                    let left_max_ordering = self.nodes[*left].max_ordering();
                    let right_max_ordering = self.nodes[*right].max_ordering();
                    // The first result can satisfy the other child's already
                    // loaded upper bound, avoiding another recursive query.
                    if left_max_ordering > right_max_ordering {
                        max_ordering = self.find_max_ordering(*left, bounds, max_ordering);
                        if max_ordering < right_max_ordering {
                            max_ordering = self.find_max_ordering(*right, bounds, max_ordering);
                        }
                    } else {
                        max_ordering = self.find_max_ordering(*right, bounds, max_ordering);
                        if max_ordering < left_max_ordering {
                            max_ordering = self.find_max_ordering(*left, bounds, max_ordering);
                        }
                    }
                }
            }
        }
        max_ordering
    }

    fn push_leaf(&mut self, bounds: Bounds<U>, order: u32) -> usize {
        self.nodes.push(Node::Leaf { bounds, order });
        self.nodes.len() - 1
    }

    fn push_internal(&mut self, left: usize, right: usize) -> usize {
        let left_node = &self.nodes[left];
        let right_node = &self.nodes[right];
        let new_bounds = left_node.bounds().union(right_node.bounds());
        let max_ordering = cmp::max(left_node.max_ordering(), right_node.max_ordering());
        self.nodes.push(Node::Internal {
            bounds: new_bounds,
            left,
            right,
            max_order: max_ordering,
        });
        self.nodes.len() - 1
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
        }
    }
}

#[derive(Debug, Clone)]
enum Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    Leaf {
        bounds: Bounds<U>,
        order: u32,
    },
    Internal {
        left: usize,
        right: usize,
        bounds: Bounds<U>,
        max_order: u32,
    },
}

impl<U> Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn bounds(&self) -> &Bounds<U> {
        match self {
            Node::Leaf { bounds, .. } => bounds,
            Node::Internal { bounds, .. } => bounds,
        }
    }

    fn max_ordering(&self) -> u32 {
        match self {
            Node::Leaf {
                order: ordering, ..
            } => *ordering,
            Node::Internal {
                max_order: max_ordering,
                ..
            } => *max_ordering,
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
    use crate::{Point, Size};

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
            let left = tree.push_leaf(leaves[0].0, leaves[0].1);
            let right = tree.push_leaf(leaves[1].0, leaves[1].1);
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
                        tree.find_max_ordering(root, &query, initial) == expected,
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
