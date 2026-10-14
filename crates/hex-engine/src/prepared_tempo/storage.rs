use super::PreparedTempoMap;
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageOwner, StorageReportError};
impl PreparedTempoMap {
    pub(crate) fn visit_storage<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
        owner: StorageOwner<'a>,
    ) -> Result<(), StorageReportError> {
        let Self {
            canonical,
            max_micro_bpm: _,
        } = self;
        let layout = canonical.owned_allocation_layout();
        layout
            .checked_backing_bytes()
            .ok_or(StorageReportError::ArithmeticOverflow)?;
        report.data(
            owner,
            "session_tempo.points",
            [0; 2],
            layout.point_capacity(),
            layout.point_element_bytes(),
        )?;
        report.data(
            owner,
            "session_tempo.prefixes",
            [0; 2],
            layout.prefix_capacity(),
            layout.prefix_element_bytes(),
        )?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_report::{StorageCharge, StorageItem};
    struct Sink<'a> {
        items: [Option<StorageItem<'a>>; 2],
        len: usize,
        limit: usize,
    }
    impl<'a> PlaybackStorageVisitor<'a> for Sink<'a> {
        fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
            if self.len >= self.limit {
                return Err(StorageReportError::SinkCapacity);
            }
            self.items[self.len] = Some(item);
            self.len += 1;
            Ok(())
        }
    }
    #[test]
    fn family_reports_only_actual_backing_and_all_partial_refusals_preserve_map() {
        let mut points = Vec::with_capacity(27);
        points.push(hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::new(120_000_000).unwrap(),
        ));
        let canonical = hex_project::TempoMap::new(48_000, points).unwrap();
        let expected = canonical
            .owned_allocation_layout()
            .checked_backing_bytes()
            .unwrap();
        let map = PreparedTempoMap::prepare(canonical, 48_000).unwrap();
        let before = map
            .beat_to_frame(hex_project::BeatTime::from_quarters(9).unwrap())
            .unwrap();
        for limit in 0..=2 {
            let mut sink = Sink {
                items: [None; 2],
                len: 0,
                limit,
            };
            let mut report = Report::new(&mut sink);
            let result = map.visit_storage(&mut report, StorageOwner::Exclusive);
            if limit < 2 {
                assert_eq!(result, Err(StorageReportError::SinkCapacity));
            } else {
                result.unwrap();
                report.finish().require_complete().unwrap();
                let total = sink
                    .items
                    .iter()
                    .flatten()
                    .map(|i| match i.charge {
                        StorageCharge::MeasuredData { bytes, .. } => bytes,
                        _ => panic!("data"),
                    })
                    .sum::<usize>();
                assert_eq!(total, expected);
                assert_ne!(total, expected + std::mem::size_of::<PreparedTempoMap>());
            }
            assert_eq!(
                map.beat_to_frame(hex_project::BeatTime::from_quarters(9).unwrap())
                    .unwrap(),
                before
            );
            // Failed partial sinks are discarded at each iteration boundary.
        }
    }
}
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::storage_report::{StorageCharge, StorageItem};
    use std::sync::Arc;
    struct TempoSink<'a> {
        items: [Option<StorageItem<'a>>; 4],
        len: usize,
    }
    impl<'a> PlaybackStorageVisitor<'a> for TempoSink<'a> {
        fn visit(&mut self, item: StorageItem<'a>) -> Result<(), StorageReportError> {
            if item.location.component.starts_with("session_tempo.") {
                if self.len == self.items.len() {
                    return Err(StorageReportError::SinkCapacity);
                }
                self.items[self.len] = Some(item);
                self.len += 1;
            }
            Ok(())
        }
    }
    #[test]
    fn actual_playback_reports_both_backings_with_same_owner_dedup_and_unknown_refusal() {
        let project = hex_project::Project::new(48_000).unwrap();
        let revision = Arc::new(
            crate::PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let layout = revision
            .session()
            .timing()
            .canonical
            .owned_allocation_layout();
        let mut playback =
            crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
        let mut sink = TempoSink {
            items: [None; 4],
            len: 0,
        };
        let summary = playback.visit_storage_on_control(&mut sink).unwrap();
        assert_eq!(sink.len, 4); // Prepared and runtime revisions alias the same inventory.
        for index in 0..2 {
            let first = sink.items[index].unwrap();
            let repeated = sink.items[index + 2].unwrap();
            let (StorageOwner::Shared(owner), StorageOwner::Shared(other)) =
                (first.owner, repeated.owner)
            else {
                panic!("actual session owner")
            };
            assert!(owner.same_allocation(other));
            assert_eq!(owner.category(), "session_inventory");
            assert_eq!(first.location.component, repeated.location.component);
            let StorageCharge::MeasuredData { bytes, .. } = first.charge else {
                panic!("measured data")
            };
            assert_eq!(
                bytes,
                if index == 0 {
                    layout.point_bytes().unwrap()
                } else {
                    layout.prefix_bytes().unwrap()
                }
            );
        }
        assert!(summary.require_complete().is_err());
    }
}
