//! Physical SDK gate and exact predecessor through a Resident removal offer.
use super::*;
use hex_session::service::GraphEditOperation;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn running_resident_vst3_removal_offers_the_exact_cut() {
    const CHILD: &str = "HEX_TEST_HOSTED_INSTRUMENT_REMOVAL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    h.next();
    let (mut applied, _) = h.applied(0);
    h.run(Request::Import(files.wav(48_000, 131_072)));
    (applied, _) = h.applied(applied);
    let Created::Track(track) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "held instrument".into(),
        }))
        .created
        .unwrap()
    else {
        panic!("instrument track");
    };
    (applied, _) = h.applied(applied);
    let length = BeatTime::from_quarters(8).unwrap();
    let Created::Clip(clip) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        }))
        .created
        .unwrap()
    else {
        panic!("instrument clip");
    };
    (applied, _) = h.applied(applied);
    let authored = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: length,
        pitch: 60,
        channel: 0,
        velocity: 63,
    }));
    (applied, _) = h.applied(applied);
    h.run(Request::InsertPluginEffect(request(
        &authored.state,
        PluginEffectDestination::Instrument(track),
    )));
    let before = terminal(&mut h);
    assert!(before.plugin_effect_complete[0].accepted);
    h.applied(applied);
    h.enable_confirmed_device_inserts();
    let control = &h.audio.as_ref().unwrap().control;
    control.set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..7 {
        render_completed_block(&h);
    }
    let pcm = render_completed_block(&h);
    let incumbent = 0.25 + 63.0 / 127.0;
    assert!(
        pcm.iter()
            .flatten()
            .all(|value| (*value - incumbent).abs() < 1e-5)
    );
    let source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let predecessor = source(&h);
    h.service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .unwrap();
    let mut offer = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while offer.is_none() {
        h.pump();
        render_completed_block(&h);
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, before.state.project);
            assert_eq!(event.state.revision, before.state.revision);
            assert_eq!(event.state.can_undo, before.state.can_undo);
            assert_eq!(event.state.can_redo, before.state.can_redo);
            assert_eq!(source(&h), predecessor);
            if let Some(error) = event.error {
                while let Some(row) = h.audio.as_mut().unwrap().control.take_audio_diagnostic() {
                    eprintln!("hosted removal diagnostic {row:?}");
                }
                panic!("hosted removal refused before consent: {error}");
            }
            assert!(!event.request_complete, "removal completed before its cut");
            offer = event.device_insert_confirmation;
        }
        assert!(Instant::now() < deadline, "missing hosted removal consent");
    }
    let offer = offer.unwrap();
    assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
    assert_eq!(
        offer.submission().output_identity,
        predecessor.output_identity
    );
    assert!(offer.cancel());
}
