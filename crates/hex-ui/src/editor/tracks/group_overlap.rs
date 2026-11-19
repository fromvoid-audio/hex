//! Paint geometry for overlapping half-open cut and victim intervals.

/// Paint-only scalar sweep: intersect unions without member × victim work.
/// Two events per visible member/victim, one sort; no capture/update storage.
pub(in crate::editor) fn group_overlap_spans(mut events: Vec<(u64, i32, i32)>) -> Vec<(u64, u64)> {
    events.sort_unstable();
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let (mut cuts, mut victims, mut previous) = (0, 0, 0);
    for (position, cut, victim) in events {
        if previous < position && cuts > 0 && victims > 0 {
            if let Some(last) = spans.last_mut()
                && last.1 == previous
            {
                last.1 = position;
            } else {
                spans.push((previous, position));
            }
        }
        cuts += cut;
        victims += victim;
        previous = position;
    }
    spans
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::group_overlap_spans;

    #[test]
    fn group_overlap_sweep_matches_independent_half_open_occupancy() {
        let mut intervals = vec![(0, 0)];
        for a in 0..4 {
            for b in a + 1..=4 {
                intervals.push((a, b));
            }
        }
        for &a in &intervals {
            for &b in &intervals {
                for &c in &intervals {
                    for &d in &intervals {
                        let mut events = Vec::new();
                        for (start, end) in [a, b] {
                            if start < end {
                                events.push((start, 1, 0));
                                events.push((end, -1, 0));
                            }
                        }
                        for (start, end) in [c, d] {
                            if start < end {
                                events.push((start, 0, 1));
                                events.push((end, 0, -1));
                            }
                        }
                        let actual = group_overlap_spans(events);
                        assert!(actual.iter().all(|(start, end)| start < end));
                        assert!(actual.windows(2).all(|pair| pair[0].1 < pair[1].0));
                        for frame in 0..4 {
                            let has = |ranges: [(u64, u64); 2]| {
                                ranges
                                    .iter()
                                    .any(|(start, end)| *start <= frame && frame < *end)
                            };
                            assert_eq!(
                                actual
                                    .iter()
                                    .any(|(start, end)| *start <= frame && frame < *end),
                                has([a, b]) && has([c, d])
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn group_overlap_sweep_max_model_shape_reports_paint_storage_separately() {
        let pairs = hex_project::MAX_CLIPS / 2;
        let mut events = Vec::with_capacity(pairs * 4);
        for index in (0..pairs).rev() {
            let start = index as u64 * 8;
            events.extend([
                (start, 1, 0),
                (start + 6, -1, 0),
                (start + 3, 0, 1),
                (start + 8, 0, -1),
            ]);
        }
        let event_bytes = events.capacity() * std::mem::size_of::<(u64, i32, i32)>();
        let start = std::time::Instant::now();
        let spans = group_overlap_spans(events);
        let elapsed = start.elapsed();
        assert_eq!(spans.len(), pairs);
        assert_eq!(spans[0], (3, 6));
        assert_eq!(
            spans[pairs - 1],
            ((pairs as u64 - 1) * 8 + 3, (pairs as u64 - 1) * 8 + 6)
        );
        eprintln!(
            "group_paint_sweep events={} event_capacity_bytes={event_bytes} output_capacity_bytes={} elapsed_ns={} debug_only/no_native_deadline; two paint-local Vecs, capture/update allocation unchanged",
            pairs * 4,
            spans.capacity() * std::mem::size_of::<(u64, u64)>(),
            elapsed.as_nanos()
        );
    }
}
