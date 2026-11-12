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

#[test]
fn edited_loop_with_a_source_effect_follows_the_running_cursor_without_allocations() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "source effect".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("instrument track");
    };
    let source = editor.project().track(track).unwrap().source_id().unwrap();
    let parent = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    let utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    editor
        .execute(Command::InsertNestedDevice {
            parent,
            state: utility,
        })
        .unwrap();
    editor
        .execute(Command::BindTrackSource {
            track,
            source: Some(source),
        })
        .unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "position witness".into(),
            path: "loop.wav".into(),
            sample_rate: 48_000,
            frames: 2_048,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let samples: Vec<_> = (0..2048).map(|frame| [frame as f32 / 4096.0; 2]).collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples.clone()).unwrap())]);
    let table = Arc::new(SineTable::new());
    let mut prepare = |start, end, enabled| {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled,
            })
            .unwrap();
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    for section in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = prepare(64, 512, true);
        let playback = if section {
            let region = revision.arrangement().loop_region().unwrap();
            let runtime = hex_engine::SessionPlayback::prepare_section_arrangement_preroll(
                revision,
                None,
                region,
                0,
                &AtomicBool::new(false),
            )
            .unwrap();
            let playback = Playback::from_interactive_arrangement_preroll(runtime).unwrap();
            assert_eq!(
                playback.section_loop(),
                None,
                "section uses the saved marker"
            );
            playback
        } else {
            Playback::try_from_prepared_with_session(revision).unwrap()
        };
        control.publish(playback).unwrap();
        if !section {
            control.command(PlaybackCommand::Play).unwrap();
        }
        for _ in 0..if section { 9 } else { 10 } {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 64]).unwrap();
        }
        assert_eq!(control.status.position.load(Acquire), 192);

        // Behind, ahead, around the cursor, and disabled. Publish before entering
        // the measured scope, and reclaim the displaced graph on control.
        for (start, end, enabled, frames, expected) in [
            (16, 128, true, 64, 256),
            (384, 640, true, 416, 416),
            (400, 600, true, 200, 416),
            (400, 600, false, 256, 672),
        ] {
            let first = control.status.position.load(Acquire);
            let playback =
                Playback::try_from_prepared_with_session(prepare(start, end, enabled)).unwrap();
            let revision = control.publish(playback).unwrap();
            let mut output = [[0.0; 2]; 416];
            let measured = allocation_counter::measure(|| {
                for chunk in output[..frames].chunks_mut(37) {
                    callback.begin_block();
                    callback.render(chunk).unwrap();
                }
            });
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert_eq!(control.status.position.load(Acquire), expected);
            assert!(control.status.playing.load(Acquire));
            assert_eq!(output[0], samples[first as usize], "edit never seeks");
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            control.reclaim();
        }
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}
