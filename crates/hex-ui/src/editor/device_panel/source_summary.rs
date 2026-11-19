//! Canonical source identity and sharing information for the selected track.
use hex_project::{Project, Track, TrackId};
use std::sync::{Arc, Weak};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SourceSummary {
    pub name: String,
    pub receiver: Option<String>,
    pub contributors: usize,
    pub role: &'static str,
}

fn source_summary(project: &Project, track: &Track) -> Option<SourceSummary> {
    let source = project.native_source(track.source_id()?)?;
    let role = if track.retained_source_path() {
        "source path"
    } else if source.receiver() == Some(track.id()) {
        "receiver"
    } else {
        "midi contributor"
    };
    Some(SourceSummary {
        name: source.name().to_owned(),
        receiver: source
            .receiver()
            .and_then(|id| project.track(id))
            .map(|track| track.name().to_owned()),
        contributors: project.source_contributors(source.id()).count(),
        role,
    })
}

/// Reuse the summary for repaints of the same immutable project and track.
/// A weak key does not retain old project snapshots after replacement.
#[derive(Default)]
pub(super) struct SourceSummaryCache {
    project: Weak<Project>,
    track: Option<TrackId>,
    summary: Option<SourceSummary>,
}

impl SourceSummaryCache {
    pub(super) fn get(&mut self, project: &Arc<Project>, track: TrackId) -> Option<&SourceSummary> {
        let same_project = self
            .project
            .upgrade()
            .is_some_and(|old| Arc::ptr_eq(&old, project));
        if !same_project || self.track != Some(track) {
            self.summary = project
                .track(track)
                .and_then(|track| source_summary(project, track));
            self.project = Arc::downgrade(project);
            self.track = Some(track);
        }
        self.summary.as_ref()
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, TrackDeviceSlot};

    #[test]
    fn source_summary_tracks_bindings_receiver_and_retained_role() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(unbound) = editor
            .execute(Command::AddTrack {
                name: "Audio".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        assert_eq!(
            source_summary(editor.project(), editor.project().track(unbound).unwrap()),
            None
        );
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "Receiver A".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        editor
            .execute(Command::RenameNativeSource {
                source,
                name: "Bright Bass".into(),
            })
            .unwrap();
        assert_eq!(
            source_summary(editor.project(), editor.project().track(receiver).unwrap()),
            Some(SourceSummary {
                name: "Bright Bass".into(),
                receiver: Some("Receiver A".into()),
                contributors: 1,
                role: "receiver",
            })
        );
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: None,
            })
            .unwrap();
        let without_receiver =
            source_summary(editor.project(), editor.project().track(receiver).unwrap()).unwrap();
        assert_eq!(without_receiver.receiver, None);
        assert_eq!(without_receiver.role, "midi contributor");
        assert!(editor.undo());
        let Created::Track(peer) = editor
            .execute(Command::AddMidiTrack {
                name: "Peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        assert_eq!(
            source_summary(editor.project(), editor.project().track(peer).unwrap()),
            Some(SourceSummary {
                name: "Bright Bass".into(),
                receiver: Some("Receiver A".into()),
                contributors: 2,
                role: "midi contributor",
            })
        );
        let mut cache = SourceSummaryCache::default();
        let shared = Arc::new(editor.project().clone());
        assert_eq!(cache.get(&shared, peer).unwrap().contributors, 2);
        assert_eq!(cache.get(&shared, receiver).unwrap().role, "receiver");
        assert_eq!(cache.get(&shared, unbound), None);
        let mut snapshot = editor.project().snapshot();
        snapshot.retained_source_paths.push(receiver.value());
        let retained = Project::from_snapshot(snapshot).unwrap();
        assert_eq!(
            source_summary(&retained, retained.track(receiver).unwrap()),
            Some(SourceSummary {
                name: "Bright Bass".into(),
                receiver: Some("Receiver A".into()),
                contributors: 1,
                role: "source path",
            })
        );
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: None,
            })
            .unwrap();
        let unlinked = Arc::new(editor.project().clone());
        assert_eq!(cache.get(&unlinked, peer), None);
        assert_eq!(cache.get(&unlinked, receiver).unwrap().contributors, 1);
        let old_project = Arc::downgrade(&unlinked);
        drop(unlinked);
        assert!(
            old_project.upgrade().is_none(),
            "the view cache must not retain old projects"
        );
        assert_eq!(
            source_summary(editor.project(), editor.project().track(peer).unwrap()),
            None
        );
        assert_eq!(
            source_summary(editor.project(), editor.project().track(receiver).unwrap())
                .unwrap()
                .contributors,
            1
        );
        assert!(editor.undo());
        assert_eq!(
            source_summary(editor.project(), editor.project().track(peer).unwrap())
                .unwrap()
                .contributors,
            2
        );
    }
}
