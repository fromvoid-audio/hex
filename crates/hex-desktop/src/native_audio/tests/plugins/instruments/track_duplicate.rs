//! Track duplication starts an independent hosted instrument with the current preset.
use super::*;
use crate::native_audio::tests::resident_sections::run_callback;
use hex_project::TrackSelectionIds;
use hex_session::service::TrackClipboardAction;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn duplicate_track_captures_live_vst3_preset_for_a_new_instance() {
    const CHILD: &str = "HEX_DESKTOP_TEST_TRACK_DUPLICATE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .env("HEX_ORIGINAL_INSTRUMENT_LIVE_PRESET", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track");
    };
    let (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(1).unwrap(),
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("midi clip");
    };
    let (applied, _) = h.applied(applied);
    let note = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &note.state,
            PluginEffectDestination::Instrument(track),
        )))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let (applied, _) = h.applied(applied);
    let original = inserted.state.project.track(track).unwrap();
    let original_device = original.instrument().unwrap();
    let preset = |device: &hex_project::Device| {
        let component = hex_plugin_state::decode(device.state())
            .unwrap()
            .component
            .unwrap();
        f32::from_le_bytes(component[8..12].try_into().unwrap())
    };
    assert_eq!(preset(original_device), 1.0);
    let audio = h.audio.as_mut().unwrap();
    audio.control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..6 {
        render_completed_block(&h);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Pause)
        .unwrap();
    render_completed_block(&h);

    let duplicate = run_callback(
        &mut h,
        Request::TrackClipboard {
            project_generation: 0,
            revision: inserted.state.revision,
            action: TrackClipboardAction::Duplicate {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                without_clips: true,
            },
        },
    );
    assert!(duplicate.error.is_none(), "{:?}", duplicate.error);
    let Some(Created::Tracks { first: copy, last }) = duplicate.created else {
        panic!("track copy");
    };
    assert_eq!(copy, last);
    h.applied(applied);
    let project = &duplicate.state.project;
    let copied = project.track(copy).unwrap();
    let copied_device = copied.instrument().unwrap();
    assert_ne!(copied.source_id(), original.source_id());
    assert_ne!(copied_device.id(), original_device.id());
    assert_eq!(h.plugin_records.load(Acquire), 2);
    assert_eq!(project.track(track).unwrap(), original);
    assert_eq!(
        preset(copied_device),
        0.5,
        "copy must start with the live preset"
    );
    let bypassed = run_callback(
        &mut h,
        Request::Edit(Command::SetDeviceBypass {
            device: copied_device.id(),
            bypassed: true,
        }),
    );
    assert!(bypassed.error.is_none(), "{:?}", bypassed.error);
    let project = &bypassed.state.project;
    assert_eq!(project.track(track).unwrap(), original);
    assert!(
        project
            .track(copy)
            .unwrap()
            .instrument()
            .unwrap()
            .bypassed()
    );
}
