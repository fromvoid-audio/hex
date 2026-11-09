//! Source-bound Arrangement paint admission; project owns validation and history.
use hex_engine::AudioSourceContext;
use hex_project::{BeatTime, Command, TrackId};

/// Immutable, scalar paint intent captured before pointer release. The 32-slot
/// service FIFO retains no media, graph or borrowed UI resources for this intent.
/// A stale source or active parameter gesture refuses before canonical editing;
/// the ordinary prepare-before-commit owner validates the track and beat range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintMidiClip {
    pub project_generation: u64,
    pub revision: u64,
    pub track: TrackId,
    pub start: BeatTime,
    pub length: BeatTime,
}

impl PaintMidiClip {
    pub(super) fn resolve(
        self,
        source: AudioSourceContext,
        gesture_active: bool,
    ) -> Result<Command, String> {
        if self.project_generation != source.project_generation
            || self.revision != source.editor_revision
        {
            return Err("clip paint canceled · project changed".into());
        }
        if gesture_active {
            return Err("finish or cancel the active gesture before painting clips".into());
        }
        Ok(Command::PlaceMidiClipWithOverwrite {
            track: self.track,
            start: self.start,
            length: self.length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{Created, Event, Request, Service, service_tests::next_event};
    use crate::{
        DocumentAudioMode,
        device_gesture::{GestureKey, GestureRequest, GestureTarget},
    };

    fn run(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        let event = next_event(service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }

    fn fixture() -> (Service, Event, TrackId) {
        let service = Service::new(48_000, 32, 0).unwrap();
        next_event(&service);
        let state = run(
            &service,
            Request::Edit(Command::AddTrack {
                name: "track".into(),
            }),
        );
        let track = state.state.project.tracks()[0].id();
        (service, state, track)
    }

    fn intent(event: &Event, track: TrackId) -> PaintMidiClip {
        PaintMidiClip {
            project_generation: 0,
            revision: event.state.revision,
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        }
    }

    fn refused(service: &Service, intent: PaintMidiClip, before: &Event) -> Event {
        service.submit(Request::PaintMidiClip(intent)).unwrap();
        let event = next_event(service);
        assert!(event.error.is_some() && event.request_complete);
        assert!(event.created.is_none() && event.playback.is_none());
        assert_eq!(event.state.project, before.state.project);
        assert_eq!(event.state.revision, before.state.revision);
        assert_eq!(
            (event.state.can_undo, event.state.can_redo),
            (before.state.can_undo, before.state.can_redo)
        );
        event
    }

    #[test]
    fn paint_overwrites_once_through_canonical_history() {
        let (service, _, track) = fixture();
        let before = run(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            }),
        );
        run(
            &service,
            Request::PrepareTuningAudition {
                revision: before.state.revision,
                input: std::sync::Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
                    0,
                )),
            },
        );
        let painted = run(&service, Request::PaintMidiClip(intent(&before, track)));
        assert!(matches!(painted.created, Some(Created::Clip(_))));
        let clips = painted.state.project.track(track).unwrap().midi_clips();
        assert_eq!(clips.len(), 3);
        let mut ranges: Vec<_> = clips.iter().map(|c| (c.start(), c.length())).collect();
        ranges.sort();
        assert_eq!(
            ranges,
            [
                (BeatTime::ZERO, BeatTime::ONE),
                (BeatTime::ONE, BeatTime::ONE),
                (
                    BeatTime::from_quarters(2).unwrap(),
                    BeatTime::from_quarters(2).unwrap()
                ),
            ]
        );
        assert_eq!(painted.state.revision, before.state.revision + 1);
        let undone = run(&service, Request::Undo);
        let restored = undone.state.project.snapshot();
        let mut expected = before.state.project.snapshot();
        assert!(restored.next_id >= expected.next_id);
        expected.next_id = restored.next_id;
        assert_eq!(restored, expected);
        let redone = run(&service, Request::Redo);
        assert_eq!(redone.state.project, painted.state.project);
    }

    #[test]
    fn stale_and_invalid_paints_leave_content_and_history_unchanged() {
        let (service, before, track) = fixture();
        let captured = intent(&before, track);
        refused(
            &service,
            PaintMidiClip {
                project_generation: 1,
                ..captured
            },
            &before,
        );
        refused(
            &service,
            PaintMidiClip {
                revision: before.state.revision + 1,
                ..captured
            },
            &before,
        );
        refused(
            &service,
            PaintMidiClip {
                length: BeatTime::ZERO,
                ..captured
            },
            &before,
        );
        refused(
            &service,
            PaintMidiClip {
                start: BeatTime::from_raw(u64::MAX),
                ..captured
            },
            &before,
        );
        let changed = run(
            &service,
            Request::Edit(Command::RenameTrack {
                track,
                name: "new".into(),
            }),
        );
        refused(&service, captured, &changed);
        let removed = run(&service, Request::Edit(Command::RemoveTrack(track)));
        refused(
            &service,
            PaintMidiClip {
                revision: removed.state.revision,
                ..captured
            },
            &removed,
        );
    }

    #[test]
    fn replacement_with_reused_ids_never_retargets_paint() {
        let (service, before, track) = fixture();
        let captured = intent(&before, track);
        let replaced = run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::Resident,
                reservation: None,
            },
        );
        assert!(replaced.project_replaced);
        let same_ids = run(
            &service,
            Request::Edit(Command::AddTrack {
                name: "replacement".into(),
            }),
        );
        assert_eq!(same_ids.state.project.tracks()[0].id(), track);
        refused(&service, captured, &same_ids);
        // Generation remains a separate guard even when a caller supplies the
        // replacement's current revision and its reused track identity.
        refused(
            &service,
            PaintMidiClip {
                revision: same_ids.state.revision,
                ..captured
            },
            &same_ids,
        );
    }

    #[test]
    fn active_gesture_is_not_committed_by_paint_refusal() {
        let (service, before, track) = fixture();
        let key = GestureKey {
            generation: 0,
            token: 81,
        };
        run(
            &service,
            Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::TrackGain { track },
            }),
        );
        let event = refused(&service, intent(&before, track), &before);
        assert!(event.active_device_gesture.is_some());
        assert!(event.error.unwrap().contains("active gesture"));
        run(
            &service,
            Request::DeviceGesture(GestureRequest::Cancel { key }),
        );
        run(&service, Request::PaintMidiClip(intent(&before, track)));
    }
}
