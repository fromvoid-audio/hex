use super::*;
use crate::{Audio, Clip, Project, Track, playback::Playback};
struct Sink<'a> {
    items: [Option<StorageItem<'a>>; 256],
    len: usize,
    limit: usize,
}
impl<'a> Sink<'a> {
    fn new(limit: usize) -> Self {
        Self {
            items: [None; 256],
            len: 0,
            limit,
        }
    }
    fn items(&self) -> impl Iterator<Item = &StorageItem<'a>> {
        self.items[..self.len].iter().flatten()
    }
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
fn fixture() -> (Arc<Project>, usize, usize) {
    let audio = Audio::new(48_000, vec![[0.25, -0.25]; 8]).unwrap();
    let other = Audio::new(48_000, vec![[0.25, -0.25]; 8]).unwrap();
    let mut clips = Vec::with_capacity(16);
    for source in [audio.clone(), audio, other] {
        clips.push(Clip::new(source, 0, 8, false).unwrap());
    }
    let clips_capacity = clips.capacity();
    let mut tracks = Vec::with_capacity(12);
    tracks.push(Track::new(clips, 1.0, false).unwrap());
    let track_capacity = tracks.capacity();
    (
        Arc::new(Project::new(48_000, tracks).unwrap()),
        track_capacity,
        clips_capacity,
    )
}
#[test]
fn actual_capacity_nested_owner_identity_and_strong_counts() {
    let (project, track_capacity, clip_capacity) = fixture();
    let mut playback = Playback::from_shared(project.clone());
    let strong = Arc::strong_count(&project);
    let mut sink = Sink::new(256);
    let summary = playback.visit_storage_on_control(&mut sink).unwrap();
    assert_eq!(Arc::strong_count(&project), strong);
    let measured = |name| {
        sink.items()
            .find(|i| i.location.component == name)
            .unwrap()
            .charge
    };
    assert!(
        matches!(measured("arrangement.tracks"), StorageCharge::MeasuredData { capacity, bytes, .. }
        if capacity == track_capacity && bytes == track_capacity * std::mem::size_of::<Track>())
    );
    assert!(
        matches!(measured("track.clips"), StorageCharge::MeasuredData { capacity, bytes, .. }
        if capacity == clip_capacity && bytes == clip_capacity * std::mem::size_of::<Clip>())
    );
    let mut pcm = sink.items().filter_map(|i| match i.owner {
        StorageOwner::Shared(owner)
            if owner.category() == "resident_pcm" && i.location.component == "arc.pointee" =>
        {
            Some(owner)
        }
        _ => None,
    });
    let first = pcm.next().unwrap();
    let second = pcm.next().unwrap();
    let third = pcm.next().unwrap();
    assert!(first.same_allocation(second));
    assert!(!first.same_allocation(third)); // Equal samples, distinct actual allocation.
    assert!(pcm.next().is_none());
    assert!(summary.unknowns() > 0);
    assert_eq!(
        summary.require_complete(),
        Err(StorageReportError::Incomplete {
            unknowns: summary.unknowns()
        })
    );
    assert!(sink.items().any(|i| matches!(
        i.charge,
        StorageCharge::Unknown(UnknownStorageReason::ArcOwnerOverhead)
    )));
    assert!(sink.items().filter(|i| matches!(i.charge, StorageCharge::OwnerOverheadDeclared { .. }))
        .all(|i| matches!(i.owner, StorageOwner::Shared(owner) if owner.category() == "resident_pcm")));
}
#[test]
fn bounded_sink_refusal_changes_no_runtime_or_owners() {
    let (project, _, _) = fixture();
    let mut playback = Playback::from_shared(project.clone());
    playback.apply(crate::PlaybackCommand::Seek(3));
    let strong = Arc::strong_count(&project);
    {
        let mut sink = Sink::new(1);
        assert_eq!(
            playback.visit_storage_on_control(&mut sink),
            Err(StorageReportError::SinkCapacity)
        );
    }
    assert_eq!(playback.position(), 3);
    assert_eq!(Arc::strong_count(&project), strong);
    let mut fresh = Sink::new(256);
    assert!(
        playback
            .visit_storage_on_control(&mut fresh)
            .unwrap()
            .require_complete()
            .is_err()
    );
}
#[test]
fn checked_overflow_and_default_unknown_are_not_zero_cost() {
    let mut sink = Sink::new(256);
    let mut report = Report::new(&mut sink);
    assert_eq!(
        report.data(StorageOwner::Exclusive, "overflow", [0; 2], usize::MAX, 2),
        Err(StorageReportError::ArithmeticOverflow)
    );
    report
        .unknown(
            StorageOwner::Exclusive,
            "opaque_provider",
            UnknownStorageReason::OpaqueProvider,
        )
        .unwrap();
    let summary = report.finish();
    assert_eq!(summary.unknowns(), 1);
    assert!(summary.require_complete().is_err());
    let (project, _, _) = fixture();
    let other = Arc::new(Project::new(48_000, Vec::new()).unwrap());
    let a = SharedOwnerRef::new(&project, "arrangement");
    let b = SharedOwnerRef::new(&other, "arrangement");
    assert!(!a.same_allocation(b));
    let wrong_type = Arc::new(7u64);
    assert!(!a.same_allocation(SharedOwnerRef::new(&wrong_type, "arrangement")));
}
#[test]
fn prepared_session_box_capacities_and_duplicate_revision_roots_remain_incomplete() {
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
    let mut playback = Playback::try_from_prepared_with_session(revision.clone()).unwrap();
    let strong = Arc::strong_count(&revision);
    let mut sink = Sink::new(256);
    let summary = playback.visit_storage_on_control(&mut sink).unwrap();
    assert_eq!(Arc::strong_count(&revision), strong);
    assert!(sink.items().any(|i| i.location.component == "session.box" &&
        matches!(i.charge, StorageCharge::MeasuredData { bytes, .. } if bytes == std::mem::size_of::<crate::SessionPlayback>())));
    assert!(
        sink.items()
            .any(|i| i.location.component == "session.releases")
    );
    let mut roots = sink.items().filter_map(|i| match i.owner {
        StorageOwner::Shared(r)
            if r.category() == "prepared_revision" && i.location.component == "arc.pointee" =>
        {
            Some(r)
        }
        _ => None,
    });
    assert!(roots.next().unwrap().same_allocation(roots.next().unwrap()));
    assert!(summary.require_complete().is_err());
    // Nested inventory/template/tempo/provider owners must not disappear.
    assert!(
        sink.items()
            .any(|i| i.location.component == "session_inventory"
                && matches!(
                    i.charge,
                    StorageCharge::Unknown(UnknownStorageReason::NestedOwnerContract)
                ))
    );
}
#[test]
fn unknown_summary_cannot_be_hidden_by_a_sink_and_visits_clone_no_root() {
    struct Observe<'a> {
        root: &'a Arc<Project>,
        expected: usize,
        visits: usize,
    }
    impl<'a, 'b> PlaybackStorageVisitor<'a> for Observe<'b> {
        fn visit(&mut self, _: StorageItem<'a>) -> Result<(), StorageReportError> {
            assert_eq!(Arc::strong_count(self.root), self.expected);
            self.visits += 1;
            Ok(())
        }
    }
    let (project, _, _) = fixture();
    let mut playback = Playback::from_shared(project.clone());
    let mut sink = Observe {
        root: &project,
        expected: Arc::strong_count(&project),
        visits: 0,
    };
    assert!(
        playback
            .visit_storage_on_control(&mut sink)
            .unwrap()
            .require_complete()
            .is_err()
    );
    assert!(sink.visits > 0);
    let extra = project.clone();
    let positive = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.visit(StorageItem {
            owner: StorageOwner::Exclusive,
            location: StorageLocation {
                component: "positive_clone",
                indices: [0; 2],
            },
            charge: StorageCharge::Unknown(UnknownStorageReason::OpaqueProvider),
        })
        .unwrap();
    }));
    assert!(positive.is_err());
    drop(extra);
}
#[test]
fn opaque_chain_defaults_unknown_even_when_declared_payload_is_zero() {
    let mut sink = Sink::new(256);
    let mut report = Report::new(&mut sink);
    report
        .chain(&hex_lattice::NativeRuntimeChain::empty(), [0; 2])
        .unwrap();
    assert!(report.finish().require_complete().is_err());
    assert!(
        sink.items()
            .any(|i| matches!(i.charge, StorageCharge::ProviderDeclared { bytes: 0 }))
    );
    assert!(sink.items().any(|i| matches!(
        i.charge,
        StorageCharge::Unknown(UnknownStorageReason::OpaqueProvider)
    )));
}
