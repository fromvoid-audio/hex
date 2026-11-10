//! One group overlay uses the real Resident/verified worker, publication and callback.
use super::super::resident_preview::ResidentHarness;
use super::*;
use hex_project::{Command, RelativeTrackGains, TrackId};
use hex_session::device_gesture::{GestureKey, GestureRequest, ParameterValue};

enum GroupHarness {
    Resident(Box<ResidentHarness>),
    Verified(Box<Harness>),
}
impl GroupHarness {
    fn new(verified: bool) -> Self {
        if verified {
            let mut h = actual_native_harness();
            h.next();
            h.open();
            h.run(Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(AudioPolicy::source_reservation()),
            });
            h.open();
            h.applied(h.context.unwrap());
            Self::Verified(Box::new(h))
        } else {
            let mut h = ResidentHarness::with_frame_limit(2_000_000);
            h.next();
            h.applied(0);
            Self::Resident(Box::new(h))
        }
    }
    fn audio(&self) -> &EditorAudio {
        match self {
            Self::Resident(h) => h.audio.as_ref().unwrap(),
            Self::Verified(h) => h.audio.as_ref().unwrap(),
        }
    }
    fn audio_mut(&mut self) -> &mut EditorAudio {
        match self {
            Self::Resident(h) => h.audio.as_mut().unwrap(),
            Self::Verified(h) => h.audio.as_mut().unwrap(),
        }
    }
    fn run(&mut self, request: Request) -> Event {
        match self {
            Self::Resident(h) => h.run(request),
            Self::Verified(h) => h.run(request),
        }
    }
    fn pump(&mut self) {
        match self {
            Self::Resident(h) => h.pump(),
            Self::Verified(h) => {
                while let Ok(event) = h.events.try_recv() {
                    match event {
                        EditorAudioEvent::Unavailable { error, .. } => panic!("{error}"),
                        EditorAudioEvent::Opened { .. } => {
                            panic!("gain must retain the output endpoint")
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    fn render(&self) -> [hex_engine::Frame; 64] {
        match self {
            Self::Resident(h) => h.render(),
            Self::Verified(h) => h.render(),
        }
    }
    fn revision(&self) -> u64 {
        let status = self.audio().status().unwrap();
        match self {
            Self::Resident(_) => status.applied_revision.load(Acquire),
            Self::Verified(_) => status
                .applied_audio_source()
                .map_or(0, |source| source.audio_revision),
        }
    }
    fn settle_setup(&mut self, event: &Event) {
        match self {
            Self::Verified(h) => {
                h.applied(h.context.unwrap());
            }
            Self::Resident(h) => {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    h.pump();
                    h.render();
                    if let Some((revision, metadata)) = h.submitted
                        && metadata.context.editor_revision == event.state.revision
                        && h.audio
                            .as_ref()
                            .unwrap()
                            .status()
                            .unwrap()
                            .applied_revision
                            .load(Acquire)
                            == revision
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline, "canonical setup was not applied");
                    std::thread::yield_now();
                }
            }
        }
    }
}

fn endpoints(gain: f32) -> [f32; 2] {
    [0.25 * gain + 0.0625, 0.5 * gain + 0.0625]
}
fn check_atomic(before: &mut [f32; 2], pcm: &[[f32; 2]], steps: bool) {
    for frame in pcm {
        assert!(
            (frame[1] - 0.0625 - 2.0 * (frame[0] - 0.0625)).abs() < 1e-6,
            "selected tracks must change together and leave the third track fixed: {frame:?}"
        );
        if steps {
            for lane in 0..2 {
                assert!(
                    (frame[lane] - before[lane]).abs() < 0.003,
                    "group transition stepped: {before:?} -> {frame:?}"
                );
            }
        }
        *before = *frame;
    }
}
fn wait_audio(
    h: &mut GroupHarness,
    after: u64,
    target: f32,
    settle: bool,
    before: &mut [f32; 2],
) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut position = h.audio().status().unwrap().position.load(Acquire);
    loop {
        h.pump();
        let pcm = h.render();
        check_atomic(before, &pcm, true);
        let next = h.audio().status().unwrap().position.load(Acquire);
        assert!(next >= position, "group gain must not reset transport");
        position = next;
        let current = h.revision();
        if current > after
            && (!settle
                || pcm.iter().all(|frame| {
                    frame
                        .iter()
                        .zip(endpoints(target))
                        .all(|(a, b)| (*a - b).abs() < 1e-6)
                }))
        {
            return current;
        }
        assert!(
            Instant::now() < deadline,
            "group did not reach its callback endpoint {target}: {before:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn gesture(h: &mut GroupHarness, request: GestureRequest) -> Event {
    let event = h.run(Request::DeviceGesture(request));
    assert!(
        event.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        event.gesture_ack
    );
    event
}

#[test]
fn resident_group_gains_publish_together_reverse_cancel_and_undo_without_callback_allocations() {
    check(false);
}
#[test]
fn verified_group_gains_publish_together_reverse_cancel_and_undo_without_callback_allocations() {
    check(true);
}
fn check(verified: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = GroupHarness::new(verified);
    let mut ids = Vec::<TrackId>::new();
    let mut canonical = None;
    for (name, samples, gain) in [
        ("left.wav", [8192, 0], 0.5),
        ("right.wav", [0, 8192], 1.0),
        ("offset.wav", [8192, 8192], 0.25),
    ] {
        let imported = h.run(Request::Import(
            files.constant_wav(name, 48_000, 480_000, &samples),
        ));
        h.settle_setup(&imported);
        let track = imported.state.project.tracks().last().unwrap().id();
        let changed = h.run(Request::Edit(Command::Gain { track, gain }));
        h.settle_setup(&changed);
        ids.push(track);
        canonical = Some(changed);
    }
    let canonical = canonical.unwrap();
    let group = RelativeTrackGains::capture(&canonical.state.project, ids[0], &ids[..2]).unwrap();
    h.audio_mut()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut before = loop {
        h.pump();
        let pcm = h.render();
        if pcm.iter().all(|frame| {
            frame
                .iter()
                .zip(endpoints(0.5))
                .all(|(a, b)| (*a - b).abs() < 1e-6)
        }) {
            break pcm[63];
        }
        assert!(
            Instant::now() < deadline,
            "initial group mix missing: {:?}",
            pcm[63]
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let output = h.audio().control.output_identity().unwrap();
    let generation = canonical
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    let key = GestureKey {
        generation,
        token: 1,
    };
    let mut applied = h.revision();
    gesture(&mut h, GestureRequest::BeginTrackGains { key, group });
    let changed = gesture(
        &mut h,
        GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(1.0),
        },
    );
    assert_eq!(changed.state.project, canonical.state.project);
    applied = wait_audio(&mut h, applied, 1.0, false, &mut before);
    gesture(
        &mut h,
        GestureRequest::Change {
            key,
            sequence: 2,
            value: ParameterValue::Float(0.25),
        },
    );
    applied = wait_audio(&mut h, applied, 0.25, true, &mut before);
    gesture(&mut h, GestureRequest::Cancel { key });
    applied = wait_audio(&mut h, applied, 0.5, true, &mut before);
    let key = GestureKey {
        generation,
        token: 2,
    };
    gesture(&mut h, GestureRequest::BeginTrackGains { key, group });
    gesture(
        &mut h,
        GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.75),
        },
    );
    applied = wait_audio(&mut h, applied, 0.75, false, &mut before);
    let committed = gesture(
        &mut h,
        GestureRequest::Commit {
            key,
            sequence: 2,
            manual_edit: true,
            value: ParameterValue::Float(0.75),
        },
    );
    assert_eq!(committed.state.project.track(ids[0]).unwrap().gain(), 0.75);
    assert_eq!(committed.state.project.track(ids[1]).unwrap().gain(), 1.5);
    assert_eq!(committed.state.project.track(ids[2]).unwrap().gain(), 0.25);
    applied = wait_audio(&mut h, applied, 0.75, true, &mut before);
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.project, canonical.state.project);
    applied = wait_audio(&mut h, applied, 0.5, true, &mut before);
    let redone = h.run(Request::Redo);
    assert_eq!(redone.state.project, committed.state.project);
    wait_audio(&mut h, applied, 0.75, true, &mut before);
    assert_eq!(h.audio().control.output_identity(), Some(output));
}
