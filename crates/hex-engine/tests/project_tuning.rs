use hex_engine::{
    LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_tuning::{Audition, ProjectTuning};
use std::{collections::BTreeMap, sync::Arc};
fn tuning(project: &Project, hz: f64) -> Project {
    let mut scale = project.tuning().scale();
    scale.set_reference(60, hz, 0).unwrap();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::SetTuning(
            ProjectTuning::new(true, scale, Audition::default()).unwrap(),
        ))
        .unwrap();
    editor.project().clone()
}

fn fixture(
    kind: hex_lattice_contract::DeviceTypeId,
    short: bool,
) -> (Project, hex_project::ClipId) {
    let mut e = Editor::new(Project::new(48000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::InsertTrackWithDevice {
            name: "tuned".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = e.project().track(track).unwrap().instrument().unwrap().id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        kind,
    )
    .unwrap();
    e.execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    e.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: if short {
            BeatTime::from_raw(BeatTime::ONE.raw() / 32)
        } else {
            BeatTime::ONE
        },
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "tuning".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = e
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    (e.project().clone(), session)
}
fn prepare(project: &Project, previous: Option<&PreparedRevision>) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_devices::SineTable::new()),
            previous.map(|revision| revision.session().as_ref()),
        )
        .unwrap(),
    )
}
fn sampler(project: &Project) -> hex_lattice::PreparedAnalyticNote {
    hex_devices::native_instrument_registry()
        .unwrap()
        .prepare_instrument(
            project.tracks()[0].instrument().unwrap().state(),
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48000,
                tuning: &project.tuning().table().unwrap(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_note(60, 100)
        .unwrap()
}
#[test]
fn arrangement_session_and_reused_banks_follow_custom_pitch_and_carried_release_law() {
    for kind in [
        hex_devices::SINE_INSTRUMENT_TYPE_ID,
        hex_devices::fm4::device::FM4_TYPE_ID,
    ] {
        for short in [false, true] {
            for session in [false, true] {
                let (p0, clip) = fixture(kind, short);
                let p1 = tuning(&p0, 333.0);
                let p2 = tuning(&p1, 196.0);
                let r0 = prepare(&p0, None);
                // Mutable source continuity requires the same validated tuning
                // family used by the live host, rather than an ordinary edit.
                let r1 = Arc::new(
                    PreparedRevision::prepare_tuning_from(
                        &p1,
                        &BTreeMap::new(),
                        Arc::new(hex_devices::SineTable::new()),
                        &p0,
                        &r0,
                    )
                    .unwrap(),
                );
                let r2 = Arc::new(
                    PreparedRevision::prepare_tuning_from(
                        &p2,
                        &BTreeMap::new(),
                        Arc::new(hex_devices::SineTable::new()),
                        &p1,
                        &r1,
                    )
                    .unwrap(),
                );
                let mut old = Playback::from_prepared(Arc::clone(&r0));
                if session {
                    old.try_apply_action(PlaybackAction::LaunchAndPlay(
                        SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        },
                    ))
                    .unwrap();
                } else {
                    old.apply(PlaybackCommand::Play);
                }
                old.render(&mut [[0.0; 2]; 777]).unwrap();
                let mut next = Playback::from_prepared(r1);
                next.try_inherit_runtime_from(&mut old).unwrap();
                let a = sampler(&p0);
                let b = sampler(&p1).continue_from(&a, 777).unwrap();
                let gate = if short { 750 } else { 24000 };
                let mut out = [[0.0; 2]; 136];
                next.render(&mut out).unwrap();
                for (i, actual) in out.into_iter().enumerate() {
                    assert_eq!(
                        actual,
                        b.sample(777 + i as u64, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                        "first tuning: session {session} short {short} at {i}"
                    );
                }
                let mut last = Playback::from_prepared(r2);
                last.try_inherit_runtime_from(&mut next).unwrap();
                let c = sampler(&p2).continue_from(&b, 913).unwrap();
                last.render(&mut out).unwrap();
                for (i, actual) in out.into_iter().enumerate() {
                    assert_eq!(
                        actual,
                        c.sample(913 + i as u64, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES),
                        "rapid tuning: session {session} short {short} at {i}"
                    );
                }
                // New onset in a freshly constructed target also proves cached inline
                // banks were invalidated when no shared context existed.
                let r = prepare(&p2, Some(&r0));
                let mut fresh = Playback::from_prepared(r);
                fresh.apply(PlaybackCommand::Play);
                fresh.render(&mut out).unwrap();
                for (i, actual) in out.into_iter().enumerate() {
                    assert_eq!(
                        actual,
                        sampler(&p2).sample(i as u64, gate, &hex_lattice::EMPTY_DEVICE_RESOURCES)
                    );
                }
            }
        }
    }
}
#[test]
fn tuned_sine_arrangement_matches_independent_frequency_pcm_oracle() {
    let (project, _) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, false);
    let project = tuning(&project, 333.0);
    let mut playback = Playback::from_prepared(prepare(&project, None));
    playback.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 2048];
    playback.render(&mut output).unwrap();
    for (age, actual) in output.into_iter().enumerate() {
        let expected = (std::f64::consts::TAU * 333.0 * age as f64 / 48000.0).sin() as f32
            * (100.0 / 127.0)
            * 0.1
            * (age as f32 / 240.0).min(1.0);
        assert!((actual[0] - expected).abs() < 0.000002);
    }
}

#[test]
fn non_octave_degree_maps_render_identically_in_arrangement_and_session() {
    for (count, period) in [(7, 1901.955_f32), (19, 900.0_f32)] {
        let (project, session_clip) = fixture(hex_devices::SINE_INSTRUMENT_TYPE_ID, false);
        let cents = std::array::from_fn(|i| {
            if i < count {
                period * i as f32 / count as f32
            } else {
                0.0
            }
        });
        // MIDI key 60 maps two degrees below the reference key, across a period
        // boundary: an independent quotient/remainder oracle exercises root degree.
        let scale = hex_tuning::Scale::new(count, cents, 62, 333.0, 1, period).unwrap();
        let mut snapshot = project.snapshot();
        snapshot.tuning = ProjectTuning::new(true, scale, Audition::default()).unwrap();
        let project = Project::from_snapshot(snapshot).unwrap();
        let degree = -1_i32;
        let mapped = f64::from(cents[degree.rem_euclid(count as i32) as usize])
            + f64::from(period) * f64::from(degree.div_euclid(count as i32))
            - f64::from(cents[1]);
        let hz = 333.0 * 2.0_f64.powf(mapped / 1200.0);
        let revision = prepare(&project, None);
        for session in [false, true] {
            let mut playback =
                Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
            if session {
                playback
                    .try_apply_action(PlaybackAction::LaunchAndPlay(
                        SessionPlaybackCommand::LaunchClip {
                            clip: session_clip,
                            quantization: LaunchQuantization::Immediate,
                        },
                    ))
                    .unwrap();
            } else {
                playback.apply(PlaybackCommand::Play);
            }
            let mut output = [[0.0; 2]; 1024];
            for block in output.chunks_mut(97) {
                playback.render(block).unwrap();
            }
            for (age, frame) in output.iter().enumerate() {
                let expected = (std::f64::consts::TAU * hz * age as f64 / 48000.0).sin()
                    * (100.0 / 127.0)
                    * 0.1
                    * (age as f64 / 240.0).min(1.0);
                assert!(
                    (f64::from(frame[0]) - expected).abs() < 0.000002,
                    "{count} degrees period {period}, session {session}, frame {age}"
                );
                assert_eq!(frame[0], frame[1]);
            }
        }
    }
}
