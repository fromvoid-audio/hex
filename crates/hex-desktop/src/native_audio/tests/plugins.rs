//! Actual Session/desktop/native Root workflow; no device or app execution.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_audio::plugin_effects::{EffectPresentation, Hosting};
use hex_project::{Command, Created};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};

fn host() -> Hosting {
    Hosting::inspect_fixture_on_control(&PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap(),
    ))
    .unwrap()
}
fn selection() -> PluginEffectSelection {
    let receipt = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let module = receipt
        .parent()
        .unwrap()
        .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3");
    let cid = [
        0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    let executable = "Contents/MacOS/fixture".to_string();
    let (code, info) = Hosting::inspect_fixture_binary(&module, &executable, cid).unwrap();
    let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect();
    PluginEffectSelection {
        module,
        executable,
        cid,
        executable_sha256: hex(code),
        info_sha256: hex(info),
    }
}
fn request(
    state: &hex_session::service::ViewState,
    destination: PluginEffectDestination,
) -> PluginEffectInsertion {
    PluginEffectInsertion {
        context: AudioSourceContext {
            project_generation: state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            editor_revision: state.revision,
        },
        selection: selection(),
        destination,
        before: None,
        control: Arc::new(PluginEffectControl::default()),
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < end,
            "actual native coordinator progress timed out"
        );
        std::thread::yield_now();
    }
}
fn terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event() {
            if !event.plugin_effect_complete.is_empty() {
                return event;
            }
        }
        assert!(
            Instant::now() < end,
            "matching native insertion terminal timed out"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_waits_for_actual_endpoint_then_commits_track_return_main() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for location in 0..3 {
        // A headless sink has no physical-device contribution. This original
        // zero-latency fixture has a known2Q copied bridge and no downstream
        // processor; file input acquisition contributes0. No hardware claim.
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let files = Files::new();
        let mut state = h.run(Request::Import(files.wav(48_000, 2048))).state;
        (applied, _) = h.applied(applied);
        let track = state.project.tracks()[0].id();
        let destination = if location == 1 {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "effect".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(bus)) = added.created else {
                panic!("return")
            };
            state = h
                .run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                }))
                .state;
            (applied, _) = h.applied(applied);
            PluginEffectDestination::Return(bus)
        } else if location == 0 {
            PluginEffectDestination::Track(track)
        } else {
            PluginEffectDestination::Main
        };
        let input = request(&state, destination);
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.request_complete && pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, state.project);
        // Hold the real callback. Observe an actual queued endpoint, then prove
        // it did not become canonical while its installation is still withheld.
        wait(|| h.plugin_queued.load(Acquire));
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(!accepted.request_complete);
        assert_eq!(accepted.plugin_effect_complete.len(), 1);
        assert!(accepted.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &accepted.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(accepted.state.revision, state.revision + 1);
        assert!(
            matches!(accepted.created, Some(Created::Device(id)) if id.value() == state.project.next_identity())
        );
        h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let mut output = Vec::new();
        for block in 0..8 {
            output.extend(h.render_plugin_block());
            wait(|| h.plugin_completed.load(Acquire) >= block + 1);
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        let expected = if location == 1 {
            [0.3125; 2]
        } else {
            [0.125; 2]
        };
        assert!(
            output[512..].iter().all(|frame| *frame == expected),
            "native PCM differs at location{location}: {:?}",
            &output[512..528]
        );
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_refuses_unknown_presentation_without_canonical_or_history_edit() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(host(), None);
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let input = request(&initial.state, PluginEffectDestination::Main);
    h.service
        .submit(Request::InsertPluginEffect(input.clone()))
        .unwrap();
    let mut refused = h.next();
    if refused.plugin_effect_complete.is_empty() {
        refused = terminal(&mut h);
    }
    assert!(refused.error.as_ref().unwrap().contains("presentation"));
    assert_eq!(refused.state.project, initial.state.project);
    assert!(!refused.state.can_undo);
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &input.control
    ));
    let end = Instant::now() + Duration::from_secs(10);
    while h.plugin_records.load(Acquire) != 0 {
        h.render();
        assert!(
            Instant::now() < end,
            "original refused source did not reach actual report recovery"
        );
        std::thread::yield_now();
    }
}
