//! Pure interval splitting for passive time-move previews.

pub(in crate::editor) fn time_fragments_outside(
    start: u64,
    end: u64,
    exclusions: [Option<(u64, u64)>; 2],
) -> [Option<(u64, u64)>; 3] {
    let mut intervals = [None, None];
    let mut interval_count = 0;
    for (exclude_start, exclude_end) in exclusions.into_iter().flatten() {
        let exclude_start = exclude_start.max(start);
        let exclude_end = exclude_end.min(end);
        if exclude_start < exclude_end && interval_count < intervals.len() {
            intervals[interval_count] = Some((exclude_start, exclude_end));
            interval_count += 1;
        }
    }
    if interval_count == 2
        && intervals[0].is_some_and(|left| intervals[1].is_some_and(|right| right.0 < left.0))
    {
        intervals.swap(0, 1);
    }
    let mut result = [None, None, None];
    let mut count = 0;
    let mut cursor = start;
    for (from, to) in intervals.into_iter().flatten() {
        if to <= cursor {
            continue;
        }
        if from > cursor && count < result.len() {
            result[count] = Some((cursor, from));
            count += 1;
        }
        cursor = cursor.max(to);
    }
    if cursor < end && count < result.len() {
        result[count] = Some((cursor, end));
    }
    result
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::time_fragments_outside;

    #[test]
    fn overlapping_source_and_destination_exclusions_leave_only_the_union_tail() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [Some((0, 48_000)), Some((24_000, 72_000))]),
            [Some((72_000, 96_000)), None, None]
        );
    }

    #[test]
    fn destination_replacement_preserves_both_outside_tails() {
        assert_eq!(
            time_fragments_outside(0, 96_000, [None, Some((24_000, 72_000))]),
            [Some((0, 24_000)), Some((72_000, 96_000)), None]
        );
    }
}
