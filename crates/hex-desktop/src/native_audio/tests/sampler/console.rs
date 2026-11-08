//! Real native callback launch, capability, queue and Stop evidence.
use super::*;
use hex_engine::{
    LaunchQuantization as Quantization, PlaybackAction, SessionPlaybackCommand as Session,
};

pub(super) fn exercise(
    cx: &gpui::TestAppContext,
    visual: &mut gpui::VisualTestContext,
    h: &mut Harness,
    _window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    saved: &std::path::Path,
) {
    let project = hex_storage::load(saved).unwrap();
    let scene = project.scenes()[0].id();
    let hex_project::SessionCellContent::Midi(clip) = project.session_cell(track, scene).unwrap()
    else {
        panic!("midi cell")
    };
    let click = |visual: &mut gpui::VisualTestContext, selector: &'static str| {
        let point = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector}"))
            .center();
        visual.simulate_click(point, gpui::Modifiers::default());
        visual.run_until_parked();
        cx.background_executor
            .advance_clock(Duration::from_millis(16));
        visual.run_until_parked();
    };
    click(visual, "show console view · tab");
    let cell =
        Box::leak(format!("console-cell-{}-{}", track.value(), scene.value()).into_boxed_str());
    click(visual, cell);
    let launch =
        Box::leak(format!("launch-midi-{}-{}", track.value(), scene.value()).into_boxed_str());
    click(visual, launch);
    let mut output = [[0.0; 2]; 64];
    let identity;
    {
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        identity = backend.status().unwrap().rendered_current(h).unwrap();
        assert_eq!(
            backend.status().unwrap().session_capability(),
            Some(hex_engine::SessionCapabilityStatus::Available)
        );
        super::envelope::render(backend, &mut output);
        let mut routes = [hex_audio::prepared::SessionRouteEntry::default(); 4];
        let count = backend
            .status()
            .unwrap()
            .session_routes(&mut routes)
            .unwrap()
            .count;
        assert!(
            routes[..count].iter().any(|r| r.owner
                == hex_audio::prepared::SessionRouteOwner::Session {
                    clip_id: clip.id().value()
                }),
            "routes={:?}; pcm={:?}",
            &routes[..count],
            &output[..4]
        );
    }
    assert_eq!(
        &output[..4],
        &[[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]]
    );
    click(visual, "stop · escape");
    {
        let mut slot = h.backend.lock().unwrap();
        super::envelope::render(slot.as_mut().unwrap(), &mut output);
    }
    assert_eq!(output, [[0.0; 2]; 64]);
    let scene = Box::leak(
        format!("console-scene-menu-{}", project.scenes()[1].id().value()).into_boxed_str(),
    );
    click(visual, scene);
    click(visual, "launch selected scene");
    {
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        super::envelope::render(backend, &mut output);
        assert_eq!(
            backend.status().unwrap().rendered_current(h),
            Some(identity)
        );
    }
    assert_eq!(
        &output[..4],
        &[[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0; 2]]
    );
    click(visual, "stop all session clips");
    {
        let mut slot = h.backend.lock().unwrap();
        super::envelope::render(slot.as_mut().unwrap(), &mut output);
    }
    assert_eq!(output, [[0.0; 2]; 64]);
}
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

#[test]
fn sampler_console_held_quantized_cut_is_literal_partition_independent_and_callback_allocation_free()
 {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut results = Vec::new();
    for partitions in [&[8_usize][..], &[3, 1, 4][..]] {
        let files = Files::new();
        let mut h = Harness::resident_sampler(12);
        let (track, asset) = authored(&mut h, &files, true, false, false);
        let mut clips = Vec::new();
        for (name, pitch, quarters) in [("held", 60, 5), ("incoming", 72, 1)] {
            let Some(Created::Scene(scene)) = h
                .run(Request::Edit(Command::AddScene { name: name.into() }))
                .created
            else {
                panic!("scene")
            };
            let length = BeatTime::from_quarters(quarters).unwrap();
            let Some(Created::SessionClip(clip)) = h
                .run(Request::Edit(Command::CreateSessionMidiClip {
                    track,
                    scene,
                    name: name.into(),
                    source_length: length,
                }))
                .created
            else {
                panic!("clip")
            };
            h.run(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: length,
                pitch,
                channel: 0,
                velocity: 127,
            }));
            clips.push(clip);
        }
        let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let assigned = h.run(Request::AssignSampler { track, asset });
        let device = assigned
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        h.open_sampler(old);
        h.applied(h.context.unwrap());
        let mut state = assigned.state;
        for (parameter, value) in [
            (hex_engine::SamplerParameter::LoopEnabled, 1.0),
            (hex_engine::SamplerParameter::ReleaseMs, 0.125),
        ] {
            let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
            let mut edit = super::gain::edit(&state, track, device);
            edit.parameter = parameter;
            edit.value = value;
            h.run(Request::SamplerEdit(
                hex_session::service::SamplerEdit::Parameter(edit),
            ));
            h.render();
            let committed = h.next();
            assert!(committed.error.is_none(), "{:?}", committed.error);
            state = committed.state;
            h.open_sampler(old);
            h.applied(h.context.unwrap());
        }
        let action = |clip, quantization| {
            PlaybackAction::Session(Session::LaunchClip { clip, quantization })
        };
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(PlaybackAction::LaunchAndPlay(Session::LaunchClip {
                clip: clips[0],
                quantization: Quantization::Immediate,
            }))
            .unwrap();
        h.render(); // frame64, A is held and loops independently of the note gate.
        h.audio
            .as_mut()
            .unwrap()
            .control
            .action_with_ticket(action(clips[1], Quantization::FourQuarter))
            .unwrap();
        let mut remaining = 31997 - 64;
        {
            let mut slot = h.backend.lock().unwrap();
            let backend = slot.as_mut().unwrap();
            while remaining != 0 {
                let count = remaining.min(64);
                super::envelope::render(backend, &mut [[0.0; 2]; 64][..count]);
                remaining -= count;
            }
            let mut routes = [hex_audio::prepared::SessionRouteEntry::default(); 4];
            let count = backend
                .status()
                .unwrap()
                .session_routes(&mut routes)
                .unwrap()
                .count;
            let route = routes[..count]
                .iter()
                .find(|r| r.track_id == track.value())
                .unwrap();
            assert_eq!(
                route.owner,
                hex_audio::prepared::SessionRouteOwner::Session {
                    clip_id: clips[0].value()
                }
            );
            assert_eq!(route.pending.unwrap().performance_frame, 32000);
            let mut pcm = Vec::new();
            for count in partitions {
                let mut block = vec![[0.0; 2]; *count];
                super::envelope::render(backend, &mut block);
                pcm.extend(block);
            }
            // The held root loops its 8-frame phase; at the frame-32000 cut
            // (index 3) the incoming octave starts at phase 0 and the held
            // voice releases over its authored 2 frames raised to the
            // prepared floor.
            let held = [
                [0.125, -0.25],
                [0.3125, -0.125],
                [0.5, 0.0],
                [0.1875, 0.125],
                [-0.125, 0.25],
                [-0.0625, 0.125],
                [0.0, 0.0],
                [0.0625, -0.125],
            ];
            let incoming = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]];
            for (index, frame) in pcm.iter().enumerate() {
                for c in 0..2 {
                    let root = held[(index + 5) % 8][c];
                    let expected = match index.checked_sub(3) {
                        None => root,
                        Some(age) => {
                            incoming[age % 4][c] + root * super::floored_release_level(age, 2)
                        }
                    };
                    assert!(
                        (frame[c] - expected).abs() < 1e-6,
                        "cut {index}: {frame:?} != {expected}"
                    );
                }
            }
            let count = backend
                .status()
                .unwrap()
                .session_routes(&mut routes)
                .unwrap()
                .count;
            let route = routes[..count]
                .iter()
                .find(|r| r.track_id == track.value())
                .unwrap();
            assert_eq!(
                route.owner,
                hex_audio::prepared::SessionRouteOwner::Session {
                    clip_id: clips[1].value()
                }
            );
            assert!(route.pending.is_none());
            results.push(pcm);
        }
    }
    assert_eq!(results[0], results[1]);
}
