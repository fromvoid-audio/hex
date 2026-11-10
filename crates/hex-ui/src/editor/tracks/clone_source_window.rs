//! Display-only mapping from a visible destination crop to its known source crop.

/// Map only display coordinates into the known original crop. This is a visual
/// clone of accepted data, never a newly compiled destination-audio projection.
pub(super) fn clone_source_window(
    source: (u64, u64),
    destination: (u64, u64),
    visible: (u64, u64),
) -> Option<(u64, u64)> {
    let source_length = source.1.checked_sub(source.0).filter(|n| *n > 0)?;
    let destination_length = destination
        .1
        .checked_sub(destination.0)
        .filter(|n| *n > 0)?;
    let left = visible.0.max(destination.0);
    let right = visible.1.min(destination.1);
    if left >= right {
        return None;
    }
    let start = u128::from(left.checked_sub(destination.0)?) * u128::from(source_length)
        / u128::from(destination_length);
    let end = (u128::from(right.checked_sub(destination.0)?) * u128::from(source_length))
        .div_ceil(u128::from(destination_length));
    Some((
        source.0.checked_add(u64::try_from(start).ok()?)?,
        source
            .0
            .checked_add(u64::try_from(end).ok()?)?
            .min(source.1),
    ))
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::clone_source_window;

    #[test]
    fn original_crop_clone_mapping_is_checked_and_crops_both_edges() {
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1000, 1800)),
            Some((100, 500))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1200, 1600)),
            Some((200, 400))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (0, 1201)),
            Some((100, 201))
        );
        assert_eq!(
            clone_source_window((100, 500), (1000, 1800), (1601, u64::MAX)),
            Some((400, 500))
        );
        assert_eq!(
            clone_source_window((u64::MAX - 4, u64::MAX), (0, u64::MAX), (0, u64::MAX)),
            Some((u64::MAX - 4, u64::MAX))
        );
        for (source, destination, visible) in [
            ((1, 1), (1, 2), (1, 2)),
            ((2, 1), (1, 2), (1, 2)),
            ((1, 2), (2, 1), (1, 2)),
            ((1, 2), (1, 2), (2, 3)),
            ((1, 2), (1, 2), (2, 1)),
        ] {
            assert_eq!(clone_source_window(source, destination, visible), None);
        }
    }
}
