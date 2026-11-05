//! Saved touch ramps execute on the actual callback without heap activity.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{
    BeatTime, Command, Created, Editor, GainTouchContext, GainTouchSpan, GainTouchStep, Project,
};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn saved_touch_ramp_renders_without_callback_allocation() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    let end = editor.project().tempo_map().frame_to_beat(512).unwrap();
    editor
        .execute(Command::SpliceGainTouch {
            track,
            expected_epoch: 0,
            span: GainTouchSpan {
                source_start: BeatTime::ZERO,
                start: BeatTime::ZERO,
                end,
                context: GainTouchContext {
                    recording_epoch: 1,
                    project_generation: 0,
                    canonical_revision: 1,
                    audio_revision: 1,
                    transport_epoch: 1,
                    gesture_epoch: 1,
                    gesture_serial: 1,
                    sample_rate: 48_000,
                    loop_pass: 0,
                },
                steps: vec![GainTouchStep {
                    beat: BeatTime::ZERO,
                    gain: 0.8,
                    origin_beat: BeatTime::ZERO,
                    source_sequence: 1,
                }],
            },
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 1024]).unwrap())]),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let counts = measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!((output[0][0] - 0.2 / 240.0).abs() < 1e-6);
    assert!((output[239][0] - 0.2).abs() < 1e-6);
    for block in 1..=2 {
        let counts = measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        if block == 2 {
            assert!(output[0][0] > 0.0);
            assert_eq!(output[239], [0.0; 2]);
        }
    }
}
