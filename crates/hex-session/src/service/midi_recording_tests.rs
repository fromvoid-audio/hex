//! Actual worker preparation, source stamps and one-history take commits.
use super::service_tests::next_event;
use super::*;
use crate::midi_recording::{MidiTakeBinding, RecordedMidiNote};
use hex_project::{BeatTime, TrackDeviceSlot};

#[test]
fn midi_take_worker_refuses_stale_and_malformed_then_commits_one_history_edit_with_exact_terminal()
{
    let service = Service::new(8_000, 8, 8_000_000).unwrap();
    next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "record".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }))
        .unwrap();
    let added = next_event(&service);
    assert!(added.error.is_none(), "{:?}", added.error);
    let track = added.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let before = next_event(&service);
    let clip = before.state.project.tracks()[0].midi_clips()[0].id();
    let binding = Arc::new(MidiTakeBinding::capture(&before.state.project, track, clip).unwrap());
    let note = RecordedMidiNote {
        start_frame: 1_000,
        end_frame: 2_000,
        pitch: 60,
        channel: 3,
        velocity: 93,
    };
    let notes: Arc<[RecordedMidiNote]> = Arc::from([note]);
    for (request_id, generation, revision, captured) in [
        (11, 1, before.state.revision, Arc::clone(&notes)),
        (12, 0, before.state.revision + 1, Arc::clone(&notes)),
        (
            13,
            0,
            before.state.revision,
            Arc::from([RecordedMidiNote {
                end_frame: 999,
                ..note
            }]),
        ),
    ] {
        service
            .submit(Request::CommitMidiTake {
                request_id,
                project_generation: generation,
                revision,
                binding: Arc::clone(&binding),
                notes: captured,
                stop_frame: 3_000,
            })
            .unwrap();
        let refused = next_event(&service);
        assert_eq!(
            refused.midi_take_complete,
            Some(MidiTakeCompletion {
                request_id,
                accepted: false
            })
        );
        assert!(refused.error.is_some());
        assert_eq!(
            refused.state.project.snapshot(),
            before.state.project.snapshot()
        );
        assert_eq!(refused.state.revision, before.state.revision);
    }
    assert_eq!(&*notes, &[note]);
    service
        .submit(Request::CommitMidiTake {
            request_id: 14,
            project_generation: 0,
            revision: before.state.revision,
            binding,
            notes: Arc::clone(&notes),
            stop_frame: 3_000,
        })
        .unwrap();
    let accepted = next_event(&service);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(
        accepted.midi_take_complete,
        Some(MidiTakeCompletion {
            request_id: 14,
            accepted: true
        })
    );
    let actual = &accepted.state.project.midi_clip(clip).unwrap().notes()[0];
    assert_eq!(
        (actual.pitch(), actual.channel(), actual.velocity()),
        (60, 3, 93)
    );
    assert_eq!(actual.start(), BeatTime::from_ratio(1, 4).unwrap());
    assert_eq!(actual.duration(), BeatTime::from_ratio(1, 4).unwrap());
    let after = accepted.state.project.snapshot();
    service.submit(Request::Undo).unwrap();
    assert!(
        next_event(&service)
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    assert_eq!(next_event(&service).state.project.snapshot(), after);
    assert_eq!(&*notes, &[note]);
}
