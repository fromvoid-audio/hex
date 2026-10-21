//! Real native callback launch, capability, queue and Stop evidence.
use super::*;
use hex_engine::{
    LaunchQuantization as Quantization, PlaybackAction, SessionPlaybackCommand as Session,
};
#[test]
fn sampler_console_actual_current_launch_stop_cancel_and_tuning_are_callback_allocation_free() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for tuned in [false, true] {
        let files = Files::new();
        let mut h = Harness::resident_sampler(12);
        let (track, asset) = authored(&mut h, &files, true, tuned, false);
        let Some(Created::Scene(scene)) = h
            .run(Request::Edit(Command::AddScene {
                name: "scene".into(),
            }))
            .created
        else {
            panic!("scene")
        };
        let Some(Created::SessionClip(clip)) = h
            .run(Request::Edit(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "pattern".into(),
                source_length: BeatTime::ONE,
            }))
            .created
        else {
            panic!("clip")
        };
        h.run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        }));
        let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        h.run(Request::AssignSampler { track, asset });
        h.open_sampler(old);
        h.applied(h.context.unwrap());
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(status.sampler_console_available());
        assert_eq!(
            status.session_capability(),
            Some(hex_engine::SessionCapabilityStatus::Available)
        );
        let launch = PlaybackAction::LaunchAndPlay(Session::LaunchClip {
            clip,
            quantization: Quantization::Immediate,
        });
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(launch)
            .unwrap();
        let pcm = h.render();
        let expected = if tuned {
            vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]]
        } else {
            vec![
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
            ]
        };
        assert_eq!(&pcm[..expected.len()], expected.as_slice());
        // The existing musical clock queues at the next four-quarter boundary;
        // cancellation and invalid launch are atomic, without changing CURRENT.
        let identity = h.audio.as_ref().unwrap().control.output_identity();
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(PlaybackAction::Session(Session::LaunchScene {
                scene,
                quantization: Quantization::FourQuarter,
            }))
            .unwrap();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        let mut routes = [hex_audio::prepared::SessionRouteEntry::default(); 4];
        let queued = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .session_routes(&mut routes)
            .unwrap();
        assert_eq!(queued.count, 1);
        let pending = routes[0].pending.unwrap();
        assert_eq!(
            (pending.performance_frame, pending.beat_raw),
            (32000, BeatTime::from_quarters(4).unwrap().raw())
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(PlaybackAction::Session(Session::CancelPending {
                track: None,
            }))
            .unwrap();
        h.render();
        let snapshot = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .session_routes(&mut routes)
            .unwrap();
        assert!(routes[..snapshot.count].iter().all(|r| r.pending.is_none()));
        assert_eq!(
            routes[0].owner,
            hex_audio::prepared::SessionRouteOwner::Session {
                clip_id: clip.value()
            }
        );
        assert_eq!(
            snapshot.capability,
            hex_engine::SessionCapabilityStatus::Available
        );
        h.audio.as_ref().unwrap().control.stop();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(PlaybackAction::LaunchAndPlay(Session::LaunchScene {
                scene,
                quantization: Quantization::Immediate,
            }))
            .unwrap();
        assert_eq!(&h.render()[..expected.len()], expected.as_slice());
        h.audio.as_ref().unwrap().control.stop();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            identity
        );
    }
}
