use super::*;
use crate::native_audio::{VerifiedPageHold, verified_graph_edit::TEMPO_CUT_LEAD_FRAMES};
use hex_engine::{Frame, PlaybackCommand, PlaybackHandoffRejection};
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
    DeviceAutomationTarget, DeviceId, Tempo,
};

#[test]
fn missed_verified_tempo_point_preserves_pcm_project_and_undo() {
    let _native = native_fixture();
    let files = Files::new();
    let (armed, hold) = mpsc::sync_channel(1);
    let mut h = native_harness_with(|sink| {
        sink.verified_page_hold = Some(VerifiedPageHold { index: 3, armed });
    });
    open_verified_file_with_limiter(&mut h, &files);
    let (entered, release) = hold
        .recv_timeout(h.wait)
        .expect("verified domain page gate");
    let before = h.run(Request::Edit(Command::SetLoopRegion {
        start: 6000,
        end: 6300,
        enabled: true,
    }));
    let old_source = h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(6000))
        .unwrap();
    let deadline = Instant::now() + h.wait;
    let mut out = [[0.0; 2]; 64];
    // At the future cut the old loop is at 6250, which maps to 12500
    // (page 3) at half tempo. The old loop only reads page 1 and prefetches 2.
    let start_position = 6000 + (250 + 300 - TEMPO_CUT_LEAD_FRAMES % 300) % 300;
    while !status(&h).playing.load(Acquire) || status(&h).position.load(Acquire) != start_position {
        let pos = status(&h).position.load(Acquire);
        let distance = if (6000..6300).contains(&pos) {
            (start_position + 300 - pos) % 300
        } else {
            64
        };
        render(&h, &mut out[..distance.clamp(1, 64) as usize]);
        assert!(Instant::now() < deadline, "waiting for the original loop");
        std::thread::yield_now();
    }
    assert!(matches!(entered.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let performance = status(&h).performance_frame.load(Acquire);
    let staged = stage_tempo_point(&mut h, 60_000_000);
    assert_eq!(staged.state.revision, before.state.revision);

    let cut = performance + TEMPO_CUT_LEAD_FRAMES;
    let prefix_start = cut - 12;
    // The reservation may become observable just before the queue entry.
    // Keep rendering small prefixes until the actual file request enters.
    loop {
        let position = status(&h).performance_frame.load(Acquire);
        assert!(
            position + 32 <= prefix_start && Instant::now() < deadline,
            "forecast did not request the cold page: {:?}",
            status(&h).rejected_revision(),
        );
        render(&h, &mut out[..32]);
        match entered.recv_timeout(Duration::from_millis(2)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("page gate disconnected: {error}"),
        }
    }
    while status(&h).performance_frame.load(Acquire) < prefix_start {
        let frame = status(&h).performance_frame.load(Acquire);
        let count = (prefix_start - frame).min(64);
        render(&h, &mut out[..count as usize]);
        assert_eq!(status(&h).performance_frame.load(Acquire), frame + count);
        assert_eq!(status(&h).applied_audio_source(), Some(old_source));
    }
    assert!(h.service.as_ref().unwrap().try_event().is_err());
    render(&h, &mut out[..16]);
    assert_eq!(status(&h).performance_frame.load(Acquire), cut + 4);
    assert_eq!(status(&h).position.load(Acquire), 6254);
    assert_pcm(&out[..16], 0.25);
    let reason = PlaybackHandoffRejection::TempoCutMissed;
    assert_eq!(
        status(&h).rejected_revision().map(|(_, actual)| actual),
        Some(reason)
    );
    assert_eq!(status(&h).applied_audio_source(), Some(old_source));
    assert!(status(&h).playing.load(Acquire));

    let failed = h.next();
    assert!(failed.request_complete);
    assert_eq!(failed.error.as_deref(), Some(reason.message()));
    assert_eq!(failed.state.revision, before.state.revision);
    assert_eq!(failed.state.project, before.state.project);
    assert_eq!(failed.state.can_undo, before.state.can_undo);
    assert_eq!(failed.state.can_redo, before.state.can_redo);
    assert_eq!(failed.state.unsaved, before.state.unsaved);
    release.send(()).unwrap();
    let undone = h.run(Request::Undo);
    assert_eq!(undone.state.revision, before.state.revision + 1);
    assert_eq!(
        undone.state.project.tempo_map(),
        before.state.project.tempo_map()
    );
    assert!(undone.state.project.loop_region().is_none());
    assert!(undone.state.can_redo);
}

#[test]
fn verified_tempo_point_preserves_the_authored_gain_step_at_its_beat() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    let (device, original) = open_verified_file_with_limiter(&mut h, &files);
    let old_timing = original.state.project.tempo_map();
    // 1/16 beat has integral frame coordinates at both tempos.
    let beat = BeatTime::from_raw(1 << 28);
    let old_step = old_timing.beat_to_frame(beat).unwrap();
    assert_eq!(old_step, 1500);
    // Host output trim's normalized range is -60 to +24 dB. Use a step
    // with sufficient separation to distinguish the old and retimed clocks.
    let points =
        [(BeatTime::ZERO, 60.0 / 84.0), (beat, 54.0 / 84.0)].map(|(beat, value)| AutomationPoint {
            beat,
            value: AutomationValue::new(value).unwrap(),
            outgoing: AutomationInterpolation::Step,
        });
    let before = h.run(Request::Edit(Command::ReplaceDeviceAutomationTarget {
        device,
        target: DeviceAutomationTarget::HostOutputGain,
        points: points.to_vec(),
    }));
    let previous = h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let mut out = [[0.0; 2]; 64];
    let deadline = Instant::now() + h.wait;
    loop {
        render(&h, &mut out);
        if out.last().unwrap()[0] > 0.20 {
            break;
        }
        assert!(Instant::now() < deadline, "waiting for compensated output");
        std::thread::yield_now();
    }
    // Render a second audible block so the published clock has a full
    // lookahead history before measuring the compensated delay.
    render(&h, &mut out);
    let old = status(&h).transport_observation().unwrap();
    let latency = old.arrangement_frame - status(&h).audible_position.load(Acquire);
    assert_eq!(latency, 72); // The default Limiter's 1.5 ms lookahead at 48 kHz.
    assert!(old.arrangement_frame + TEMPO_CUT_LEAD_FRAMES < old_step);
    let staged = stage_tempo_point(&mut h, 90_000_000);
    assert_eq!(staged.state.project, before.state.project);

    let due = old.performance_frame + TEMPO_CUT_LEAD_FRAMES;
    while status(&h).performance_frame.load(Acquire) < due - 12 {
        let frame = status(&h).performance_frame.load(Acquire);
        render(&h, &mut out[..((due - 12 - frame).min(64) as usize)]);
        assert_eq!(status(&h).applied_audio_source(), Some(previous));
        assert!(Instant::now() < deadline, "waiting for the tempo cut");
    }
    render(&h, &mut out[..16]);
    assert_pcm(&out[..16], 0.25);
    let switched = status(&h).transport_observation().unwrap();
    assert_eq!(switched.performance_frame, due + 4);
    let applied = h.next();
    assert!(applied.request_complete);
    assert!(applied.error.is_none(), "{:?}", applied.error);
    assert_eq!(applied.state.revision, before.state.revision + 1);
    assert_eq!(
        applied.state.project.device(device).unwrap().automation(),
        before.state.project.device(device).unwrap().automation()
    );
    let timing = applied.state.project.tempo_map();
    let mapped = timing
        .beat_to_frame(
            old_timing
                .frame_to_beat(old.arrangement_frame + TEMPO_CUT_LEAD_FRAMES)
                .unwrap(),
        )
        .unwrap();
    assert_eq!(switched.arrangement_frame, mapped + 4);
    let step = timing.beat_to_frame(beat).unwrap();
    assert_eq!(step, 2000);

    // Compare the compensated step to an uninterrupted pass on the same
    // new tempo map. The renderer owns the beat-to-sample rounding rule.
    let start = step - 8;
    let mut edited_pcm = [[0.0; 2]; 257];
    render_at_position(&h, start, &mut edited_pcm);

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let deadline = Instant::now() + h.wait;
    loop {
        render(&h, &mut out);
        if status(&h).position.load(Acquire) < start {
            break;
        }
        assert!(Instant::now() < deadline, "waiting for playback from zero");
    }
    let mut uninterrupted_pcm = [[0.0; 2]; 257];
    render_at_position(&h, start, &mut uninterrupted_pcm);
    let edge = usize::try_from(step + latency - start).unwrap();
    assert_pcm(&edited_pcm[..edge], 0.25);
    assert_pcm(&edited_pcm[edge..], 0.25 * 10.0_f32.powf(-6.0 / 20.0));
    assert_eq!(edited_pcm, uninterrupted_pcm);
}

fn open_verified_file_with_limiter(
    h: &mut Harness,
    files: &Files,
) -> (DeviceId, hex_session::service::Event) {
    h.wait = Duration::from_millis(500);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 16_384)));
    h.applied(h.context.unwrap());
    let track = imported.state.project.tracks()[0].id();
    let installed = h.run_verified_device_insert(Command::InsertDevice {
        track,
        before: None,
        state: hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap(),
    });
    h.applied(h.context.unwrap());
    let Some(Created::Device(device)) = installed.created else {
        panic!("installed limiter identity missing");
    };
    (device, installed)
}

fn stage_tempo_point(h: &mut Harness, micro_bpm: u32) -> hex_session::service::Event {
    let staged = h.run(Request::Edit(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(micro_bpm).unwrap(),
    }));
    assert!(
        !staged.request_complete,
        "tempo point committed before its cut"
    );
    let deadline = Instant::now() + h.wait;
    while !status(h).publication_in_flight() {
        if let Ok(event) = h.service.as_ref().unwrap().try_event() {
            panic!("tempo edit finished before publication: {:?}", event.error);
        }
        assert!(Instant::now() < deadline, "waiting for tempo submission");
        std::thread::yield_now();
    }
    staged
}

fn render_at_position(h: &Harness, target: u64, out: &mut [Frame]) {
    let mut discarded = [[0.0; 2]; 64];
    while status(h).position.load(Acquire) < target {
        let frame = status(h).position.load(Acquire);
        let count = (target - frame).min(discarded.len() as u64);
        render(h, &mut discarded[..count as usize]);
        assert_eq!(status(h).position.load(Acquire), frame + count);
    }
    assert_eq!(status(h).position.load(Acquire), target);
    render(h, out);
}

fn assert_pcm(frames: &[Frame], expected: f32) {
    assert!(
        frames
            .iter()
            .flatten()
            .all(|sample| (*sample - expected).abs() < 0.0001),
        "expected {expected} on each channel, got {frames:?}",
    );
}

fn status(h: &Harness) -> &hex_audio::prepared::Status {
    h.audio.as_ref().unwrap().status().unwrap()
}

fn render(h: &Harness, out: &mut [Frame]) {
    let mut backend = h.backend.lock().unwrap();
    let allocations = allocation_counter::measure(|| {
        backend.as_mut().unwrap().render(out).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
}
