//! Live gain-touch receipts must reproduce the same audible take on playback.
use hex_audio::{
    gain_recording::{Receipt, Target},
    prepared,
};
use hex_engine::{
    Audio, PlaybackCommand, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable,
    playback::Playback,
};
use hex_project::{Command, Created, Editor, GainTouchContext, Project};
use hex_session::gain_recording::{AppliedGain, GainTakeEnd, finish_take};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering::Acquire},
};

#[test]
fn gain_touch_live_output_matches_recorded_replay_after_rapid_changes_and_release() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "touch".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4096,
            playback_frames: 4096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap())]);
    let armed = Arc::new(
        PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap(),
    );
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let audio_revision = control
        .publish(Playback::try_from_prepared_with_session(armed).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    callback.finish_gain_recording_quantum(true);
    let target = Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: 1,
        audio_revision,
        recording_epoch: 1,
        transport_epoch: control.status.transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48_000,
    };
    let mut events = Vec::new();
    let mut end = None;
    let mut live = Vec::new();
    control.gain_recording.begin(target, 0.8).unwrap();
    for block in 0..19 {
        if block == 1 {
            control.gain_recording.change(0.2).unwrap();
        }
        if block == 2 {
            control.gain_recording.release().unwrap();
        }
        let mut output = [[0.0; 2]; 16];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        callback.finish_gain_recording_quantum(true);
        live.extend(output);
        while let Some(receipt) = control.gain_recording.try_receipt() {
            match receipt {
                Receipt::Applied {
                    sequence,
                    gain,
                    performance_frame,
                    content_frame,
                    loop_pass,
                    ..
                } => events.push(AppliedGain {
                    sequence,
                    gain,
                    performance_frame,
                    content_frame,
                    loop_pass,
                }),
                Receipt::Finished {
                    performance_frame,
                    content_frame,
                    loop_pass,
                    ..
                } => {
                    end = Some(GainTakeEnd {
                        performance_frame,
                        content_frame,
                        loop_pass,
                    })
                }
                other => panic!("unexpected receipt: {other:?}"),
            }
        }
    }
    let context = GainTouchContext {
        recording_epoch: 1,
        project_generation: 0,
        canonical_revision: 1,
        audio_revision,
        transport_epoch: target.transport_epoch,
        gesture_epoch: 1,
        gesture_serial: 1,
        sample_rate: 48_000,
        loop_pass: 0,
    };
    let spans = finish_take(editor.project(), track, 0, context, &events, end.unwrap()).unwrap();
    editor
        .execute(Command::SpliceGainTouchBatch {
            track,
            expected_epoch: 0,
            spans,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback.render(&mut [[0.0; 2]; 16]).unwrap();
    let mut replay = vec![[0.0; 2]; live.len()];
    playback.render(&mut replay).unwrap();
    assert_eq!(
        replay, live,
        "recorded take must reproduce the delivered live PCM"
    );
}
