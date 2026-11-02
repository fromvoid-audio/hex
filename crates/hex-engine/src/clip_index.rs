//! Prepared per-track index of Arrangement audio clips (Pulse #3933).
//!
//! Rendering a block visits only the clips whose overlap cluster reaches it,
//! found by `partition_point`, instead of scanning every clip on the track.
//!
//! Entries are ordered by *overlap cluster*: a maximal run of clips whose
//! `[start, end)` frame ranges chain into one another. Clusters are disjoint
//! and ordered in time; within a cluster the entries keep the clips' stored
//! order. Clips in different clusters cover disjoint frames, so the summation
//! order of every output frame is exactly that of a full scan in stored order,
//! and rendering stays bit-identical. Every clip renders only inside its own
//! `[start, end)`, which is what makes the cluster bounds exact.
//!
//! The index is built on the control side; `fill` writes into reserved
//! capacity without growing it, so persistent custody can account for it.
//! `visit` neither allocates nor waits. A query walks the clusters that reach
//! the block: `O(log n)` plus the clips in those clusters.
//!
//! This module does not own clips, their rendering, or the track's custody.

/// One clip position in a track's prepared clip index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipIndexEntry {
    clip: usize,
    cluster_start: u64,
    cluster_end: u64,
}

/// `index` lacked the reserved capacity for one entry per clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipIndexCapacity;

/// Rebuild `index` for `items` within its existing capacity. On refusal the
/// index is left empty, and an empty index is never used to skip clips.
pub(crate) fn fill<T>(
    items: &[T],
    bounds: impl Fn(&T) -> (u64, u64),
    index: &mut Vec<ClipIndexEntry>,
) -> Result<(), ClipIndexCapacity> {
    index.clear();
    if index.capacity() < items.len() {
        return Err(ClipIndexCapacity);
    }
    // Seed with each clip's own range, then order by onset (stable by index).
    index.extend(items.iter().enumerate().map(|(clip, item)| {
        let (start, end) = bounds(item);
        ClipIndexEntry {
            clip,
            cluster_start: start,
            cluster_end: end,
        }
    }));
    index.sort_unstable_by_key(|entry| (entry.cluster_start, entry.clip));
    let mut first = 0;
    while first < index.len() {
        let start = index[first].cluster_start;
        let mut end = index[first].cluster_end;
        let mut last = first + 1;
        while last < index.len() && index[last].cluster_start < end {
            end = end.max(index[last].cluster_end);
            last += 1;
        }
        let cluster = &mut index[first..last];
        for entry in cluster.iter_mut() {
            entry.cluster_start = start;
            entry.cluster_end = end;
        }
        cluster.sort_unstable_by_key(|entry| entry.clip);
        first = last;
    }
    Ok(())
}

/// Build an index on the control side.
pub(crate) fn build<T>(items: &[T], bounds: impl Fn(&T) -> (u64, u64)) -> Vec<ClipIndexEntry> {
    let mut index = Vec::with_capacity(items.len());
    // The capacity was just reserved for every item.
    let _ = fill(items, bounds, &mut index);
    index
}

/// Visit every item whose cluster reaches `[start, end)`: a superset of the
/// items intersecting it, never missing one. An index that does not cover
/// `items` falls back to the full stored-order scan.
pub(crate) fn visit<'a, T>(
    index: &[ClipIndexEntry],
    items: &'a [T],
    start: u64,
    end: u64,
    mut visit: impl FnMut(&'a T),
) {
    if index.len() != items.len() {
        items.iter().for_each(visit);
        return;
    }
    let first = index.partition_point(|entry| entry.cluster_end <= start);
    for entry in &index[first..] {
        if entry.cluster_start >= end {
            break;
        }
        if let Some(item) = items.get(entry.clip) {
            visit(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visited(index: &[ClipIndexEntry], items: &[(u64, u64)], start: u64, end: u64) -> Vec<usize> {
        let mut out = Vec::new();
        visit(index, items, start, end, |item| {
            let ordinal = items
                .iter()
                .position(|candidate| std::ptr::eq(candidate, item))
                .unwrap();
            out.push(ordinal);
        });
        out
    }

    #[test]
    fn sixteen_thousand_disjoint_clips_visit_only_the_intersecting_ones() {
        // Stored out of onset order, as canonical edits append clips.
        let count = 16_384_usize;
        let items: Vec<(u64, u64)> = (0..count)
            .map(|slot| {
                let ordinal = (slot * 7_919) % count;
                (ordinal as u64 * 80, ordinal as u64 * 80 + 64)
            })
            .collect();
        let index = build(&items, |item| *item);
        let mut visits = 0;
        visit(&index, &items, 400_000, 400_256, |_| visits += 1);
        let intersecting = items
            .iter()
            .filter(|(start, end)| *start < 400_256 && *end > 400_000)
            .count();
        assert_eq!(intersecting, 4);
        assert_eq!(visits, intersecting);
        let mut visits = 0;
        visit(&index, &items, count as u64 * 80, u64::MAX, |_| visits += 1);
        assert_eq!(visits, 0);
    }

    #[test]
    fn visits_cover_every_intersection_and_keep_stored_order_between_overlaps() {
        // Unsorted storage with chained and nested overlaps, an adjacent clip
        // (end == next start, a separate cluster) and an empty range.
        let items = [
            (500, 600),
            (100, 200),
            (150, 400),
            (0, 50),
            (300, 350),
            (50, 90),
            (700, 700),
            (650, 900),
            (120, 130),
        ];
        let index = build(&items, |item| *item);
        let overlaps = |a: (u64, u64), b: (u64, u64)| a.0 < b.1 && b.0 < a.1;
        for start in (0..1_000).step_by(7) {
            for width in [1, 13, 64, 256, 1_000] {
                let end = start + width;
                let order = visited(&index, &items, start, end);
                for (ordinal, item) in items.iter().enumerate() {
                    if overlaps(*item, (start, end)) {
                        assert!(order.contains(&ordinal), "{start}..{end} missed {ordinal}");
                    }
                }
                for (position, &a) in order.iter().enumerate() {
                    for &b in &order[position + 1..] {
                        if overlaps(items[a], items[b]) {
                            assert!(a < b, "{start}..{end}: {a} visited before {b}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fill_refuses_without_reserved_capacity_and_visit_then_scans_everything() {
        let items = [(0, 10), (20, 30)];
        let mut index = Vec::new();
        assert_eq!(
            fill(&items, |item| *item, &mut index),
            Err(ClipIndexCapacity)
        );
        assert!(index.is_empty());
        assert_eq!(visited(&index, &items, 100, 200), [0, 1]);
        let mut index = Vec::with_capacity(2);
        let capacity = index.capacity();
        fill(&items, |item| *item, &mut index).unwrap();
        assert_eq!(index.capacity(), capacity);
        assert_eq!(visited(&index, &items, 100, 200), Vec::<usize>::new());
    }

    #[test]
    fn indexed_track_render_is_bit_identical_to_the_stored_order_scan() {
        // Unsorted storage with chained overlaps and gains that do not round
        // trip, added onto a non-zero mix buffer where summation order shows.
        let source = (0..97)
            .map(|frame| [frame as f32 * 0.013 - 0.31, 0.17 - frame as f32 * 0.007])
            .collect::<Vec<_>>();
        let audio = crate::Audio::new(48_000, source).unwrap();
        let clips = [
            (900, 64, true),
            (40, 200, false),
            (0, 97, false),
            (180, 300, true),
        ]
        .into_iter()
        .chain((0..60).map(|slot| ((slot * 37 % 60) * 23 + 1_000, 31, slot % 2 == 0)))
        .map(|(start, duration, looping)| {
            crate::Clip::new(audio.clone(), start, duration, looping).unwrap()
        })
        .collect::<Vec<_>>();
        let indexed = crate::Track::new(clips, 0.7, false).unwrap();
        assert_eq!(indexed.clip_index.len(), indexed.clips.len());
        let mut scanned = indexed.clone();
        scanned.clip_index.clear();
        for block in [1, 64, 256] {
            let render = |track: &crate::Track| {
                let mut out = (0..2_600)
                    .map(|frame| [0.1 + frame as f32 * 1e-4, -0.3])
                    .collect::<Vec<_>>();
                for (chunk, start) in out.chunks_mut(block).zip((0_u64..).step_by(block)) {
                    track.render_additive(start, chunk).unwrap();
                }
                out
            };
            let (indexed, scanned) = (render(&indexed), render(&scanned));
            let bits = |out: &[crate::Frame]| {
                out.iter()
                    .flat_map(|frame| frame.map(f32::to_bits))
                    .collect::<Vec<_>>()
            };
            assert_eq!(bits(&indexed), bits(&scanned), "block={block}");
        }
    }
}
