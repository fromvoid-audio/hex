//! Display-only VST3 metadata, page validation, and touch-row selection.
use hex_audio::prepared::AppliedAudioSource;
use hex_session::service::PluginParameterRow;
use std::collections::BTreeSet;

pub(super) const PAGE_SIZE: usize = 4;
const REFRESH_YIELDED_TO_CONTROL: &str = "vst3 parameter refresh yielded to control";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TouchObservation {
    pub(super) source: AppliedAudioSource,
    pub(super) binding: hex_lattice_contract::ForeignProcessorBinding,
    pub(super) parameter: u32,
}
pub(super) fn display_only(row: &PluginParameterRow) -> PluginParameterRow {
    PluginParameterRow {
        value: String::new(),
        normalized: row.default_normalized,
        editable: false,
        automation_class: None,
        metadata_revision: 0,
        ..row.clone()
    }
}

pub(super) fn matching_rows(rows: &[PluginParameterRow], filter: &str) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| {
            (filter.is_empty() || row.name.to_lowercase().contains(filter)).then_some(index)
        })
        .collect()
}

pub(super) fn merge_page(
    cached: &[PluginParameterRow],
    cached_signature: Option<[u8; 32]>,
    cached_total: Option<u16>,
    total: u16,
    first: u16,
    live_signature: [u8; 32],
    rows: &[PluginParameterRow],
) -> Option<Vec<PluginParameterRow>> {
    let first = usize::from(first);
    let total_usize = usize::from(total);
    let page_len = total_usize.saturating_sub(first).min(PAGE_SIZE);
    if cached_signature != Some(live_signature)
        || first % PAGE_SIZE != 0
        || total_usize != cached.len()
        || cached_total != Some(total)
        || rows.len() != page_len
        || cached
            .get(first..first.saturating_add(rows.len()))
            .is_none_or(|prior| prior.iter().zip(rows).any(|(old, new)| old.id != new.id))
    {
        return None;
    }
    let mut merged = cached.to_vec();
    for (offset, row) in rows.iter().cloned().enumerate() {
        merged[first + offset] = row;
    }
    Some(merged)
}

pub(super) fn refresh_page(
    has_rows: bool,
    source_validated: bool,
    priority_page: Option<u16>,
    visible_pages: &BTreeSet<u16>,
    next_page: u16,
    rows_empty: bool,
) -> Option<Option<u16>> {
    if !has_rows {
        return Some(None);
    }
    if let Some(page) = priority_page {
        return Some(Some(page));
    }
    if !source_validated {
        return Some(Some(0));
    }
    if visible_pages.is_empty() {
        return rows_empty.then_some(Some(0));
    }
    visible_pages
        .range(next_page..)
        .next()
        .or_else(|| visible_pages.first())
        .copied()
        .map(Some)
}

pub(super) fn is_refresh_yield(error: &str) -> bool {
    error == REFRESH_YIELDED_TO_CONTROL
}

pub(super) fn fresh_touch_row(
    touch: TouchObservation,
    source: hex_lattice_contract::ForeignProcessorBinding,
    rows: &[PluginParameterRow],
    page_first: Option<u16>,
) -> Option<PluginParameterRow> {
    if touch.binding != source {
        return None;
    }
    rows.iter()
        .find(|row| {
            row.id == touch.parameter
                && row.metadata_revision != 0
                && page_first.is_none_or(|first| row.page_first == first)
        })
        .cloned()
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_engine::AudioSourceContext;

    #[test]
    fn cached_rows_are_display_only_until_a_live_page_arrives() {
        let cached = PluginParameterRow {
            id: 7,
            name: "gain".into(),
            value: String::new(),
            normalized: 0.25,
            default_normalized: 0.25,
            steps: 0,
            editable: false,
            automation_class: None,
            page_first: 0,
            metadata_revision: 0,
        };
        let stale = display_only(&PluginParameterRow {
            value: "live".into(),
            normalized: 0.8,
            editable: true,
            automation_class: Some([1; 16]),
            metadata_revision: 7,
            ..cached.clone()
        });
        assert!(stale.value.is_empty());
        assert_eq!(stale.normalized, stale.default_normalized);
        assert!(!stale.editable);
        assert_eq!(stale.automation_class, None);
        assert_eq!(stale.metadata_revision, 0);
    }

    #[test]
    fn matching_rows_keeps_cache_indices_for_live_page_requests() {
        let rows = ["oscillator", "filter cutoff", "filter resonance", "level"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| PluginParameterRow {
                id: 100 + 3 * i as u32,
                name: name.into(),
                value: String::new(),
                normalized: 0.0,
                default_normalized: 0.0,
                steps: 0,
                editable: false,
                automation_class: None,
                page_first: 0,
                metadata_revision: 0,
            })
            .collect::<Vec<_>>();
        let found = matching_rows(&rows, "filter");
        assert_eq!(found, [1, 2]);
        assert_eq!(
            found
                .iter()
                .map(|&index| (index / PAGE_SIZE) * PAGE_SIZE)
                .collect::<Vec<_>>(),
            [0, 0]
        );
    }

    #[test]
    fn live_page_merge_requires_the_cached_total_and_ordered_ids() {
        let rows = (0_u32..6)
            .map(|index| PluginParameterRow {
                id: 20 + index,
                name: format!("parameter {index}"),
                value: String::new(),
                normalized: 0.0,
                default_normalized: 0.0,
                steps: 0,
                editable: false,
                automation_class: None,
                page_first: ((index / 4) * 4) as u16,
                metadata_revision: 0,
            })
            .collect::<Vec<_>>();
        let mut second_page = rows[4..].to_vec();
        second_page[0].value = "live".into();
        let merged =
            merge_page(&rows, Some([8; 32]), Some(6), 6, 4, [8; 32], &second_page).unwrap();
        assert_eq!(merged[4].value, "live");
        assert!(merge_page(&rows, Some([8; 32]), Some(5), 6, 4, [8; 32], &second_page).is_none());
        assert!(merge_page(&rows, Some([8; 32]), Some(6), 6, 1, [8; 32], &rows[1..5]).is_none());
        let mut reordered = second_page;
        reordered.swap(0, 1);
        assert!(merge_page(&rows, Some([8; 32]), Some(6), 6, 4, [8; 32], &reordered).is_none());
        let unchanged_ids_with_changed_signature = rows[4..].to_vec();
        assert!(
            merge_page(
                &rows,
                Some([8; 32]),
                Some(6),
                6,
                4,
                [9; 32],
                &unchanged_ids_with_changed_signature,
            )
            .is_none()
        );
        let value_only_update = rows[4..]
            .iter()
            .cloned()
            .map(|mut row| {
                row.value = "changed value".into();
                row
            })
            .collect::<Vec<_>>();
        assert!(
            merge_page(
                &rows,
                Some([8; 32]),
                Some(6),
                6,
                4,
                [8; 32],
                &value_only_update
            )
            .is_some()
        );
    }

    #[test]
    fn unvalidated_cache_requests_page_zero_without_visible_matches() {
        let no_visible_pages = BTreeSet::new();
        assert_eq!(
            refresh_page(true, false, None, &no_visible_pages, 0, false),
            Some(Some(0))
        );
        assert_eq!(
            refresh_page(true, true, None, &no_visible_pages, 0, false),
            None
        );
        assert_eq!(
            refresh_page(true, true, Some(12), &no_visible_pages, 0, false),
            Some(Some(12))
        );
    }

    #[test]
    fn native_touch_resolves_only_from_a_fresh_page_for_the_exact_binding() {
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let other_binding = hex_lattice_contract::ForeignProcessorBinding {
            epoch: 5,
            ..binding
        };
        let touch = TouchObservation {
            source: AppliedAudioSource {
                context: AudioSourceContext {
                    project_generation: 8,
                    editor_revision: 9,
                },
                audio_revision: 10,
                output_identity: 11,
            },
            binding,
            parameter: 7,
        };
        let cached = PluginParameterRow {
            id: 7,
            name: "gain".into(),
            value: String::new(),
            normalized: 0.2,
            default_normalized: 0.2,
            steps: 0,
            editable: false,
            automation_class: None,
            page_first: 4,
            metadata_revision: 0,
        };
        let live = PluginParameterRow {
            value: "0.75".into(),
            normalized: 0.75,
            editable: true,
            automation_class: Some([6; 16]),
            metadata_revision: 3,
            ..cached.clone()
        };
        assert_eq!(
            fresh_touch_row(touch, binding, std::slice::from_ref(&live), Some(4)),
            Some(live.clone())
        );
        assert_eq!(
            fresh_touch_row(touch, other_binding, std::slice::from_ref(&cached), Some(4)),
            None
        );
        assert_eq!(fresh_touch_row(touch, binding, &[cached], Some(4)), None);
        assert_eq!(fresh_touch_row(touch, binding, &[live], Some(0)), None);
    }

    #[test]
    fn control_yield_is_a_quiet_refresh_retry() {
        assert!(is_refresh_yield(REFRESH_YIELDED_TO_CONTROL));
        assert!(!is_refresh_yield("vst3 parameter source changed"));
    }
}
