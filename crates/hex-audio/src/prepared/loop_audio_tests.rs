//! Prepared callback applies the loop-end envelope without allocation.
use super::*;
use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

#[test]
fn loop_end_fades_without_callback_allocations() {
    const RATE: u32 = 48_000;
    const START: usize = 41;
    const END: usize = 642;
    const FRAMES: usize = 2_048;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: RATE,
            frames: 1_024,
        })
        .unwrap()
    else {
        panic!("audio import result");
    };
    editor
        .execute(Command::SetLoopRegion {
            start: START as u64,
            end: END as u64,
            enabled: true,
        })
        .unwrap();
    let samples = vec![[0.5, -0.25]; 1_024];
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.clone()).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let (mut control, mut callback) = channel(RATE).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(revision).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; FRAMES];
    let allocations = allocation_counter::measure(|| {
        for block in output.chunks_mut(64) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    for (frame, actual) in output.iter().enumerate() {
        let source = if frame < END {
            frame
        } else {
            START + (frame - END) % (END - START)
        };
        let remaining = END - source;
        let gain = if remaining <= 128 {
            (remaining - 1) as f32 / 127.0
        } else {
            1.0
        };
        let expected = [samples[source][0] * gain, samples[source][1] * gain];
        assert_eq!(*actual, expected, "frame {frame}");
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
