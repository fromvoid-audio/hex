//! Callback publication of a compensated tempo edit, including allocation custody.
use super::*;
use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_project::{BeatTime, Command, Created, Editor, Project, Tempo};
use std::collections::BTreeMap;

#[test]
fn compensated_tempo_restart_publishes_without_callback_allocations() {
    for (partition, compensated) in [(1, true), (257, true), (257, false)] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8_192,
            })
            .unwrap()
        else {
            panic!("audio import");
        };
        if compensated {
            let limiter = hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap();
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: limiter,
                })
                .unwrap();
        }
        let media = BTreeMap::from([(
            asset,
            Audio::new(
                48_000,
                (0..8_192)
                    .map(|frame| {
                        let value = (frame as f32 * 0.017).sin() * 0.2;
                        [value, -0.7 * value]
                    })
                    .collect(),
            )
            .unwrap(),
        )]);
        let table = Arc::new(SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
        };
        let old = prepare(editor.project());
        assert_eq!(old.arrangement().main_latency_frames() > 0, compensated);
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(old).unwrap())
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        for chunk in [[0.0; 2]; 600].chunks_mut(partition) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
        assert_eq!(control.status.position.load(Acquire), 600);
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        let candidate = prepare(editor.project());
        assert_eq!(
            candidate.arrangement().main_latency_frames() > 0,
            compensated
        );
        let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&candidate)).unwrap();
        oracle.apply(PlaybackCommand::PlayFrom(800));
        let mut expected = [[0.0; 2]; 1024];
        oracle.render(&mut expected).unwrap();
        let applied = control
            .publish(Playback::try_from_prepared_with_session(candidate).unwrap())
            .unwrap();
        let mut output = [[0.0; 2]; 1024];
        let measured = allocation_counter::measure(|| {
            for chunk in output.chunks_mut(partition) {
                callback.begin_block();
                callback.render(chunk).unwrap();
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), applied);
        assert!(control.status.playing.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 1_824);
        assert_eq!(
            output, expected,
            "partition {partition}, compensated {compensated}"
        );
        assert!(output.iter().flatten().any(|sample| *sample != 0.0));
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}
