//! Loop launch, dense wrapping and stop through the real prepared callback.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, LaunchQuantization, PreparedRevision, SessionPlaybackCommand, SineTable,
    playback::Playback,
};
use hex_project::{Command, Created, Editor, Project, SessionAudioLoop};
use std::{collections::BTreeMap, sync::Arc};
#[test]
fn console_audio_loop_launch_wrap_and_stop_have_no_callback_allocation_or_drop() {
    for (first, last) in [(7, 13), (7, 8)] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "memory/cycle.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "cycle".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "intro and cycle".into(),
                asset,
                source_start: 3,
                source_end: 29,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::SetSessionAudioClipLoop {
                clip,
                loop_range: Some(SessionAudioLoop::new(first, last).unwrap()),
            })
            .unwrap();
        let pcm: Vec<_> = (0..32)
            .map(|i| [i as f32 / 64.0, -(i as f32) / 64.0])
            .collect();
        let media = BTreeMap::from([(asset, Audio::new(48_000, pcm.clone()).unwrap())]);
        let revision = Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
                .unwrap(),
        );
        let (mut control, mut callback) = prepared::channel(48_000).unwrap();
        control
            .publish(Playback::try_from_prepared_with_session(revision).unwrap())
            .unwrap();
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        let mut output = [[0.0; 2]; 17];
        for block in 0..8 {
            let measured = measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
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
            for (offset, frame) in output.iter().enumerate() {
                let elapsed = block * 17 + offset as u64;
                let source = if elapsed < last - 3 {
                    3 + elapsed
                } else {
                    first + (elapsed - (last - 3)) % (last - first)
                };
                assert_eq!(*frame, pcm[source as usize]);
            }
        }
        control
            .session_command_with_ticket(SessionPlaybackCommand::Stop)
            .unwrap();
        let measured = measure(|| {
            // The queued Stop fades over 128 frames, ending in the eighth
            // 17-frame block; the ninth is silent (#4151).
            for _ in 0..9 {
                callback.begin_block();
                callback.render(&mut output).unwrap();
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
        assert_eq!(output, [[0.0; 2]; 17]);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}
