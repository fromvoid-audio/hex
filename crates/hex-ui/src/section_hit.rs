//! Shared section-edge hit policy; callers retain gesture and project custody.
use hex_widgets::{RectangleEdges, RectanglePart};

pub(crate) const INNER_EDGE_WIDTH: f32 = 5.0;
pub(crate) const SHARED_EDGE_RADIUS: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit<K> {
    Leading(K),
    Trailing(K),
    Seam(K, K),
}

/// Ordered canonical endpoints decide adjacency; projection decides pixel hits.
/// The iterator is replayed for seam priority without allocating a second model.
pub(crate) fn hit<K: Copy, T: Copy + PartialEq>(
    sections: impl Iterator<Item = (K, T, T)> + Clone,
    x: f32,
    project: impl Fn(T) -> Option<f32>,
) -> Option<Hit<K>> {
    for ((left, _, end), (right, start, _)) in sections.clone().zip(sections.clone().skip(1)) {
        if end == start
            && project(end)
                .is_some_and(|edge| RectangleEdges::seam_hit(x, edge, SHARED_EDGE_RADIUS))
        {
            return Some(Hit::Seam(left, right));
        }
    }
    for (id, start, end) in sections {
        let (Some(start), Some(end)) = (project(start), project(end)) else {
            continue;
        };
        match (RectangleEdges {
            width: INNER_EDGE_WIDTH,
            minimum_body: 0.0,
        })
        .hit(x, start, end)
        {
            Some(RectanglePart::Leading) => return Some(Hit::Leading(id)),
            Some(RectanglePart::Trailing) => return Some(Hit::Trailing(id)),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_seam_precedes_both_inner_edges() {
        let sections = [(1, 0, 10), (2, 10, 20)];
        let project = |v| Some(v as f32);
        assert_eq!(
            hit(sections.into_iter(), 10.5, project),
            Some(Hit::Seam(1, 2))
        );
        assert_eq!(
            hit(sections.into_iter(), 8.0, project),
            Some(Hit::Trailing(1))
        );
        assert_eq!(
            hit(sections.into_iter(), 12.0, project),
            Some(Hit::Leading(2))
        );
    }

    #[test]
    fn canonical_gaps_and_narrow_edges_keep_original_order() {
        let sections = [(1, 0, 2), (2, 3, 5)];
        let project = |v| Some(v as f32);
        assert_eq!(
            hit(sections.into_iter(), 1.0, project),
            Some(Hit::Leading(1))
        );
        assert_eq!(hit(sections.into_iter(), 2.5, project), None);
        // Projected coincidence alone must not invent a canonical seam.
        let collapsed_gap = |v| Some(if v == 3 { 2.0 } else { v as f32 });
        assert_eq!(
            hit(sections.into_iter(), 2.0, collapsed_gap),
            Some(Hit::Trailing(1))
        );
    }

    #[test]
    fn overlapping_edges_keep_song_order_priority() {
        let sections = [(1, 0, 10), (2, 5, 15)];
        assert_eq!(
            hit(sections.into_iter(), 6.0, |v| Some(v as f32)),
            Some(Hit::Trailing(1))
        );
        let body_and_edge = [(1, 0, 20), (2, 9, 29)];
        assert_eq!(
            hit(body_and_edge.into_iter(), 10.0, |v| Some(v as f32)),
            Some(Hit::Leading(2))
        );
    }

    #[test]
    fn invalid_projection_never_connects_nonadjacent_sections() {
        let sections = [(1, 0, 10), (2, 10, 20), (3, 20, 30)];
        let project = |v| (v != 10).then_some(v as f32);
        assert_eq!(hit(sections.into_iter(), 10.0, project), None);
        assert_eq!(
            hit(sections.into_iter(), f32::NAN, |v| Some(v as f32)),
            None
        );
        assert_eq!(
            hit(sections.into_iter(), 10.0, |_| Some(f32::INFINITY)),
            None
        );
        assert_eq!(
            hit(sections.into_iter(), 20.0, project),
            Some(Hit::Seam(2, 3))
        );
    }
}
