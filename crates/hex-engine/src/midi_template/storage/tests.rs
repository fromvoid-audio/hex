use super::*;
use crate::storage_report::{SharedOwnerRef, StorageCharge, StorageItem, UnknownStorageReason};
use std::sync::Arc;
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
    fn visit(
        &mut self,
        item: StorageItem<'a>,
    ) -> Result<(), crate::storage_report::StorageReportError> {
        if self.len >= self.limit {
            return Err(StorageReportError::SinkCapacity);
        }
        self.items[self.len] = Some(item);
        self.len += 1;
        Ok(())
    }
}
fn project() -> (
    hex_project::Project,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "template".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    (editor.project().clone(), track, clip)
}
fn fixture() -> Arc<PreparedMidiSourceTemplate> {
    let (project, track, clip) = project();
    let timing = crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
    let mut template = PreparedMidiSourceTemplate::prepare_source(
        track,
        clip,
        project.midi_source(clip).unwrap(),
        &timing,
    )
    .unwrap();
    template.intro.reserve(17);
    template.repeats.reserve(19);
    template.intro_spans.reserve(23);
    template.repeat_spans.reserve(29);
    Arc::new(template)
}
fn owner(root: &Arc<PreparedMidiSourceTemplate>) -> crate::storage_report::StorageOwner<'_> {
    crate::storage_report::StorageOwner::Shared(SharedOwnerRef::new(
        root,
        "session_midi_source_template",
    ))
}
fn bytes(root: &PreparedMidiSourceTemplate) -> usize {
    (root.intro.capacity() + root.repeats.capacity()) * std::mem::size_of::<SourceEdge>()
        + (root.intro_spans.capacity() + root.repeat_spans.capacity())
            * std::mem::size_of::<SourceSpan>()
}
#[test]
fn four_actual_buffers_complete_family_without_repeated_headers_or_density_charge() {
    let root = fixture();
    let strong = Arc::strong_count(&root);
    let mut sink = Sink::new(256);
    let mut report = crate::storage_report::Report::new(&mut sink);
    root.visit_storage(&mut report, owner(&root)).unwrap();
    report.finish().require_complete().unwrap();
    assert_eq!(Arc::strong_count(&root), strong);
    assert_eq!(sink.len, 4);
    let total = sink
        .items()
        .map(|i| match i.charge {
            StorageCharge::MeasuredData { bytes, .. } => bytes,
            _ => panic!("family data"),
        })
        .sum::<usize>();
    assert_eq!(total, bytes(&root));
    assert_ne!(
        total,
        bytes(&root) + std::mem::size_of::<PreparedMidiSourceTemplate>()
    );
    assert!(root.repeats.capacity() > root.repeats.len());
    assert!(root.repeat_spans.capacity() > root.repeat_spans.len());
}
#[test]
fn per_container_duplicate_owner_identity_and_every_partial_refusal_preserve_source() {
    let root = fixture();
    let same = root.clone();
    let distinct = fixture();
    let mut sink = Sink::new(256);
    let mut report = crate::storage_report::Report::new(&mut sink);
    for r in [&root, &same, &distinct] {
        r.visit_storage(&mut report, owner(r)).unwrap();
    }
    report.finish().require_complete().unwrap();
    let mut unique_bytes = 0;
    for (index, item) in sink.items[..sink.len].iter().flatten().enumerate() {
        let crate::storage_report::StorageOwner::Shared(id) = item.owner else {
            panic!("owner")
        };
        let duplicate = sink.items[..index].iter().flatten().any(|old| {
            matches!(old.owner,
            crate::storage_report::StorageOwner::Shared(other) if id.same_allocation(other))
                && old.location.component == item.location.component
                && old.location.indices == item.location.indices
        });
        if !duplicate {
            let StorageCharge::MeasuredData { bytes, .. } = item.charge else {
                panic!("data")
            };
            unique_bytes += bytes;
        }
    }
    assert_eq!(unique_bytes, bytes(&root) + bytes(&distinct));
    let before = (
        root.intro.as_ptr(),
        root.repeats.as_ptr(),
        root.intro_spans.as_ptr(),
        root.repeat_spans.as_ptr(),
        root.density,
    );
    for cut in 0..4 {
        let mut sink = Sink::new(cut);
        let mut report = crate::storage_report::Report::new(&mut sink);
        assert_eq!(
            root.visit_storage(&mut report, owner(&root)),
            Err(StorageReportError::SinkCapacity)
        );
        // Partial sink is scoped/discarded; never a complete receipt.
        assert_eq!(
            before,
            (
                root.intro.as_ptr(),
                root.repeats.as_ptr(),
                root.intro_spans.as_ptr(),
                root.repeat_spans.as_ptr(),
                root.density
            )
        );
    }
}
#[test]
fn actual_session_playback_visitor_reaches_template_buffers_but_keeps_unknowns() {
    let (project, _, _) = project();
    let revision = Arc::new(
        crate::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = crate::playback::Playback::try_from_prepared_with_session(revision).unwrap();
    let mut sink = Sink::new(256);
    let summary = playback.visit_storage_on_control(&mut sink).unwrap();
    assert!(summary.require_complete().is_err());
    for component in [
        "midi_source.intro_edges",
        "midi_source.repeat_edges",
        "midi_source.intro_spans",
        "midi_source.repeat_spans",
    ] {
        assert!(sink.items().any(|i| i.location.component == component
            && matches!(i.charge, StorageCharge::MeasuredData { .. })));
    }
    assert!(sink.items().any(|i| matches!(
        i.charge,
        StorageCharge::Unknown(UnknownStorageReason::ArcOwnerOverhead)
    )));
    assert!(
        sink.items()
            .any(|i| i.location.component == "session_inventory"
                && matches!(i.charge, StorageCharge::Unknown(_)))
    );
}
#[test]
fn oversized_data_arithmetic_refuses_without_manufactured_family_total() {
    let root = fixture();
    let mut sink = Sink::new(256);
    let mut report = crate::storage_report::Report::new(&mut sink);
    assert_eq!(
        report.data(
            owner(&root),
            "midi_source.intro_spans",
            [0; 2],
            usize::MAX,
            std::mem::size_of::<SourceSpan>()
        ),
        Err(StorageReportError::ArithmeticOverflow)
    );
    assert_eq!(sink.len, 0);
}
