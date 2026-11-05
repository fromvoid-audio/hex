//! Headless prepared-callback evidence for the retained one-shot source policy.
use hex_audio::prepared;
use hex_engine::{PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
use hex_project::{
    BeatTime, Command, Created, Editor, MidiOneShotBoundary, Project, TrackContentSnapshot,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn once_boundary_callback_matches_explicit_note_gates_without_allocating_or_dropping() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "retained".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for (start, pitch) in [(0, 60), (3, 64)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(start).unwrap(),
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content else {
        panic!("hybrid")
    };
    midi_clips[0].loop_end_raw = BeatTime::ONE.raw() / 4;
    midi_clips[0].one_shot_boundary = MidiOneShotBoundary::LoopEnd;
    let candidate = Project::from_snapshot(snapshot.clone()).unwrap();
    let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content else {
        panic!("hybrid")
    };
    midi_clips[0].one_shot_boundary = MidiOneShotBoundary::SourceEnd;
    midi_clips[0].notes.truncate(1);
    midi_clips[0].notes[0].duration_raw = BeatTime::ONE.raw() / 4;
    let oracle = Project::from_snapshot(snapshot).unwrap();
    let mut endpoints = Vec::new();
    let table = Arc::new(SineTable::new());
    for project in [&candidate, &oracle] {
        let revision = Arc::new(
            PreparedRevision::prepare(project, &BTreeMap::new(), Arc::clone(&table), None).unwrap(),
        );
        let (mut control, callback) = prepared::channel(48000).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(revision).unwrap())
            .unwrap_or_else(|e| panic!("{}", e.reason));
        control.command(PlaybackCommand::Play).unwrap();
        endpoints.push((control, callback));
    }
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    let mut heard = false;
    for _ in 0..40 {
        let stats = allocation_counter::measure(|| {
            for ((_, callback), output) in endpoints.iter_mut().zip([&mut actual, &mut expected]) {
                callback.begin_block();
                callback.render(output).unwrap();
                callback.finish_gain_recording_quantum(true);
            }
        });
        assert_eq!(
            (
                stats.count_total,
                stats.bytes_total,
                stats.count_current,
                stats.bytes_current
            ),
            (0, 0, 0, 0),
            "{stats:?}"
        );
        assert_eq!(actual, expected);
        heard |= actual.iter().flatten().any(|sample| *sample != 0.0);
    }
    assert!(heard);
    for (control, _) in &mut endpoints {
        control.stop();
    }
    let stats = allocation_counter::measure(|| {
        for (_, callback) in &mut endpoints {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
            callback.finish_gain_recording_quantum(true);
        }
    });
    assert_eq!(
        (
            stats.count_total,
            stats.bytes_total,
            stats.count_current,
            stats.bytes_current
        ),
        (0, 0, 0, 0),
        "{stats:?}"
    );
    for (control, _) in &mut endpoints {
        control.reclaim();
    }
}
