use super::*;
use hex_engine::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_engine::{AudioStorageLedgerReservation, PreparedRevision, SineTable};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 256,
        root_rows: 128,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    }
}
fn compile(editor: &Editor, sources: &ResidentDeviceAssets) -> Playback {
    compile_with_tuning(editor, sources, None).0
}
fn compile_with_tuning(
    editor: &Editor,
    sources: &ResidentDeviceAssets,
    previous: Option<(&Project, &PreparedRevision)>,
) -> (Playback, Arc<PreparedRevision>) {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(sources, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        100_000,
        &cancel,
    )
    .unwrap();
    let revision = match previous {
        Some((project, old)) => revision
            .with_tuning_continuation_from(editor.project(), project, old)
            .unwrap(),
        None => revision,
    };
    let revision = Arc::new(revision);
    let playback =
        Playback::try_from_prepared_with_device_resources(Arc::clone(&revision), &resources)
            .unwrap();
    (playback, revision)
}
fn source(
    editor: &mut Editor,
    sources: &mut ResidentDeviceAssets,
    level: f32,
) -> hex_project::AssetId {
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: format!("source-{level}.wav").into(),
            sample_rate: 8000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    sources
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(8000, vec![[level; 2]; 1024])
                    .ok()
                    .unwrap(),
            ),
            0,
            limits(),
            1024,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    asset
}
fn block(callback: &mut Callback) -> [Frame; 32] {
    let mut output = [[0.0; 2]; 32];
    let mut render = || {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counted = allocation_counter::measure(&mut render);
        assert_eq!(
            (
                counted.count_total,
                counted.bytes_total,
                counted.count_current,
                counted.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    render();
    output
}

#[cfg(feature = "allocation-probe")]
#[test]
fn resident_mute_prepared_owner_is_checked_in_the_actual_callback() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            source_sample_rate: 8_000,
            source_frames: 1_024,
            playback_frames: 1_024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let audio = hex_engine::Audio::new(8_000, vec![[0.5, -0.25]; 1_024]).unwrap();
    let media = BTreeMap::from([(asset, audio)]);
    let table = Arc::new(SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    let original = prepare(&editor);
    let same_pcm_other_owner = prepare(&editor);
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    let muted = prepare(&editor);
    for expected in [&original, &same_pcm_other_owner] {
        let (mut control, mut callback) = channel(8_000).unwrap();
        control
            .publish_with_policy_and_context_generation(
                Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap(),
                PublicationPolicy::InheritRuntime,
                1,
            )
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        block(&mut callback);
        assert_eq!(block(&mut callback), [[0.5, -0.25]; 32]);
        control.reclaim();

        let mut successor = Playback::try_from_prepared_with_session(Arc::clone(&muted)).unwrap();
        assert!(successor.prepare_resident_mute_on_control(expected, track));
        let installed = control
            .publish_with_policy_and_context_generation(
                successor,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .unwrap();
        let first = block(&mut callback);
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        let second = block(&mut callback);
        if Arc::ptr_eq(expected, &original) {
            for (index, actual) in first.into_iter().chain(second).enumerate() {
                let gain = 1.0 - (index + 1).min(40) as f32 / 40.0;
                assert!((actual[0] - 0.5 * gain).abs() < 1e-6);
                assert!((actual[1] + 0.25 * gain).abs() < 1e-6);
            }
        } else {
            assert_eq!(first, [[0.0; 2]; 32]);
            assert_eq!(second, [[0.0; 2]; 32]);
        }
        control.reclaim();
        control.request_shutdown();
        block(&mut callback);
        control.reclaim();
    }
}

#[test]
fn shared_sampler_scalar_retarget_keeps_live_and_authored_history_without_callback_allocation() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let asset = source(&mut editor, &mut sources, 0.25);
    let Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    editor
        .execute(Command::InsertInstrument {
            track: receiver,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, asset, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let device = editor
        .project()
        .track(receiver)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let source_id = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source_id),
        })
        .unwrap();
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let prepare = |editor: &Editor, live: bool| {
        let mut playback = compile(editor, &sources);
        if live {
            playback
                .prepare_computer_keyboard(editor.project(), contributor, Arc::clone(&input))
                .unwrap();
        }
        playback
    };
    let (mut control, mut callback) = channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .publish_with_policy_and_context_generation(
            prepare(&editor, true),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(block(&mut callback), [[0.75; 2]; 32]);
    control.reclaim();
    for attack in [false, true] {
        let original = editor.project().device(device).unwrap().state();
        let mut state = hex_devices::sampler::state::SamplerState::from_native(original).unwrap();
        state.gain = 0.5;
        if attack {
            state.envelope.attack_seconds = 0.05;
        }
        let state = state.replace_in(original).unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        control
            .publish_with_policy_and_context_generation(
                prepare(&editor, true),
                PublicationPolicy::InheritRuntime,
                7,
            )
            .unwrap();
        let output = block(&mut callback);
        for (i, frame) in output.iter().enumerate() {
            let expected = if attack {
                0.375 * (64 + i) as f32 / 400.0
            } else {
                0.375
            };
            for sample in frame {
                assert!(
                    (sample - expected).abs() < 1e-6,
                    "attack={attack} i={i}: {frame:?}"
                );
            }
        }
        control.reclaim();
    }
    input.set_key(0, None).unwrap();
    let released = block(&mut callback);
    for (i, frame) in released.iter().enumerate() {
        let expected = 0.125 * (2.0 * (96 + i) as f32 / 400.0 + 0.24 * (1.0 - i as f32 / 400.0));
        for sample in frame {
            assert!((sample - expected).abs() < 1e-6, "key off {i}: {frame:?}");
        }
    }
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: None,
        })
        .unwrap();
    control
        .publish_with_policy_and_context_generation(
            prepare(&editor, false),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    let detached = block(&mut callback);
    for (i, frame) in detached.iter().enumerate() {
        let expected = 0.125
            * ((128 + i) as f32 / 400.0
                + 0.32 * (1.0 - i as f32 / 400.0)
                + 0.24 * (1.0 - (32 + i) as f32 / 400.0));
        for sample in frame {
            assert!((sample - expected).abs() < 1e-6, "detach {i}: {frame:?}");
        }
    }
    control.reclaim();
    for _ in 0..16 {
        block(&mut callback);
        control.reclaim();
    }
    assert_eq!(block(&mut callback), [[0.125; 2]; 32]);
    control.request_shutdown();
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    control.reclaim();
}

#[test]
fn shared_sampler_tuning_preview_cancel_keeps_actual_live_cursor_without_callback_allocations() {
    const PCM: [Frame; 4] = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]];
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "tuning-phase.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    sources
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(8000, PCM.to_vec())
                    .ok()
                    .unwrap(),
            ),
            0,
            limits(),
            4,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    editor
        .execute(Command::InsertInstrument {
            track: receiver,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    4, asset, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let (mut initial, neutral_revision) = compile_with_tuning(&editor, &sources, None);
    let neutral = editor.project().clone();
    initial
        .prepare_computer_keyboard(editor.project(), contributor, Arc::clone(&input))
        .unwrap();
    let (mut control, mut callback) = channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .publish_with_policy_and_context_generation(initial, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 7];
    let render = || {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let count = allocation_counter::measure(render);
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut render = render;
        render();
    }
    for (i, frame) in prefix.iter().enumerate() {
        assert_eq!(*frame, PCM[i % 4].map(|sample| sample * 3.0));
    }
    control.reclaim();
    let tune = |editor: &mut Editor, hz| {
        let mut scale = hex_tuning::ProjectTuning::default().scale();
        scale.set_reference(69, hz, 0).unwrap();
        editor
            .execute(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, hex_tuning::Audition::default())
                    .unwrap(),
            ))
            .unwrap();
    };
    tune(&mut editor, 660.0);
    let skipped_project = editor.project().clone();
    let (_, skipped_revision) =
        compile_with_tuning(&editor, &sources, Some((&neutral, &neutral_revision)));
    tune(&mut editor, 880.0);
    let tuned_project = editor.project().clone();
    let (mut tuned, tuned_revision) = compile_with_tuning(
        &editor,
        &sources,
        Some((&skipped_project, &skipped_revision)),
    );
    tuned
        .prepare_computer_keyboard(editor.project(), contributor, Arc::clone(&input))
        .unwrap();
    let revision = control
        .publish_with_policy_and_context_generation(tuned, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    // An octave up, the band-limited sampler (#4089) keeps only the 4-frame
    // loop's mean; every other partial is at or above the output Nyquist.
    // block() probes this pitched-up render for callback allocations. The
    // cursor is checked by the unity segments on either side.
    let mean: [f32; 2] = std::array::from_fn(|channel| {
        PCM.iter().map(|frame| frame[channel]).sum::<f32>() / 4.0 * 3.0
    });
    for frame in block(&mut callback) {
        for channel in 0..2 {
            assert!((frame[channel] - mean[channel]).abs() < 3e-3, "{frame:?}");
        }
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert!(!input.is_cancelled());
    control.reclaim();
    editor
        .execute(Command::SetTuning(neutral.tuning()))
        .unwrap();
    let (mut cancelled, _) =
        compile_with_tuning(&editor, &sources, Some((&tuned_project, &tuned_revision)));
    cancelled
        .prepare_computer_keyboard(editor.project(), contributor, Arc::clone(&input))
        .unwrap();
    control
        .publish_with_policy_and_context_generation(cancelled, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    for (i, frame) in block(&mut callback).iter().enumerate() {
        assert_eq!(*frame, PCM[(71 + i) % 4].map(|sample| sample * 3.0));
    }
    assert!(!input.is_cancelled());
    control.reclaim();
    input.set_key(0, None).unwrap();
    for (i, frame) in block(&mut callback).iter().enumerate() {
        for channel in 0..2 {
            let expected = PCM[(103 + i) % 4][channel] * (3.0 - i as f32 / 400.0);
            assert!((frame[channel] - expected).abs() < 1e-6);
        }
    }
    control.command(PlaybackCommand::Pause).unwrap();
    let paused = control.status.position.load(Acquire);
    for _ in 0..14 {
        block(&mut callback);
        control.reclaim();
    }
    // The queued Pause ends after its 21-frame fade at 8 kHz (#4151).
    assert_eq!(control.status.position.load(Acquire), paused + 21);
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    input.set_key(0, Some((60, 127, 2))).unwrap();
    for (i, frame) in block(&mut callback).iter().enumerate() {
        assert_eq!(*frame, PCM[i % 4]);
    }
    control.request_shutdown();
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    control.reclaim();
}

#[test]
fn ordinary_sampler_publication_keeps_migrated_a_and_b_obligations_through_resource_free_c() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    #[cfg(feature = "allocation-probe")]
    let (_observer, dropped) = hex_engine::native_device_resources::control_drop_probe::observe(
        std::thread::current().id(),
    );
    let a = source(&mut editor, &mut sources, 0.25);
    let b = source(&mut editor, &mut sources, -0.125);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, a, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let a_graph = compile(&editor, &sources);
    let (mut control, mut callback) = channel(8000).unwrap();
    let a_graph = match control.publish(a_graph) {
        Err(error) => error.playback,
        Ok(_) => panic!("unconfigured source accepted"),
    };
    assert_eq!(
        control.outstanding_device_resource_leases(),
        0,
        "unconfigured refusal cannot issue an obligation"
    );
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    let a_revision = control
        .publish_with_policy_and_context_generation(a_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    // Queue pressure returns the exact candidate without issuing an untracked lease.
    let pending = compile(&editor, &sources);
    let pending_identity = Arc::clone(pending.prepared_revision().unwrap());
    let refused = control.publish(pending).unwrap_err();
    assert!(Arc::ptr_eq(
        refused.playback.prepared_revision().unwrap(),
        &pending_identity
    ));
    assert_eq!(control.outstanding_device_resource_leases(), 1);
    drop(refused);
    drop(pending_identity);
    control.command(PlaybackCommand::Play).unwrap();
    assert!(
        block(&mut callback)
            .iter()
            .flatten()
            .any(|value| *value != 0.0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), a_revision);
    assert_eq!(control.outstanding_device_resource_leases(), 1);
    assert!(
        control
            .configure_device_resource_publication_on_control(limits())
            .is_err()
    );
    // Compatible edits use the ordinary same-output handoff. Repeated publication
    // must close the returned candidate's unused lease rather than accumulating it.
    let mut current_gain = 1.0_f32;
    for (muted, gain) in [(true, 1.0), (false, 1.0), (false, 0.5), (false, 1.0)] {
        editor.execute(Command::Mute { track, muted }).unwrap();
        editor.execute(Command::Gain { track, gain }).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                compile(&editor, &sources),
                PublicationPolicy::InheritRuntime,
                7,
            )
            .unwrap();
        let output = block(&mut callback);
        for (index, frame) in output.iter().enumerate() {
            if current_gain != gain {
                current_gain += (gain - current_gain) / (40 - index) as f32;
            }
            let expected = if muted { 0.0 } else { 0.25 * current_gain };
            assert!(frame.iter().all(|sample| (*sample - expected).abs() < 1e-6));
        }
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(control.reclaim(), 1);
        assert_eq!(control.outstanding_device_resource_leases(), 1);
    }
    // Cancellation and a stale command fence return source-owning candidates
    // without installing them. Their obligations close only at control reclamation.
    let active_revision = control.status.applied_revision.load(Acquire);
    for cancel in [true, false] {
        let fence = control.capture_fresh_runtime_fence().unwrap();
        if !cancel {
            control.command(PlaybackCommand::Play).unwrap();
        }
        let revision = control
            .publish_with_policy_and_context_generation(
                compile(&editor, &sources),
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                7,
            )
            .unwrap();
        assert_eq!(control.outstanding_device_resource_leases(), 2);
        if cancel {
            assert_eq!(
                control.cancel_fresh_runtime_publication(revision),
                FreshRuntimeCancellationResult::Cancelled
            );
        }
        block(&mut callback);
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            active_revision
        );
        assert_eq!(control.status.rejected_revision().unwrap().0, revision);
        assert_eq!(control.reclaim(), 1);
        assert_eq!(control.outstanding_device_resource_leases(), 1);
    }
    let b_state = hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
        1024, b, 0.05,
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: b_state.clone(),
        })
        .unwrap();
    // A same-device resource change cannot clone an active retained source,
    // so the live handoff restarts audio at the cursor (Pulse #3784): B's
    // runtime is installed and A's graph and lease retire on control.
    let rejected_before = control.status.rejected_revision();
    let restarted = control
        .publish_with_policy_and_context_generation(
            compile(&editor, &sources),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), restarted);
    assert_eq!(control.status.rejected_revision(), rejected_before);
    assert_eq!(control.reclaim(), 1);
    assert_eq!(control.outstanding_device_resource_leases(), 1);
    // A's runtime retired with the restart; its PCM stays owned by the
    // control-side source bank until that bank drops below.
    #[cfg(feature = "allocation-probe")]
    assert!(dropped.try_recv().is_err());
    // Explicit replacement has a new device/source identity and may retain
    // B's finite release alongside the new device's untriggered source.
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: b_state,
        })
        .unwrap();
    assert_ne!(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
    let b_graph = compile(&editor, &sources);
    let b_revision = control
        .publish_with_policy_and_context_generation(b_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), b_revision);
    assert_eq!(control.reclaim(), 1);
    assert_eq!(
        control.outstanding_device_resource_leases(),
        2,
        "the replaced device's lease moved into the new graph's tail"
    );
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let c_graph = compile(&editor, &sources);
    drop(sources);
    // No runtime retains A any more: dropping the bank drops it on control.
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(dropped.try_recv().unwrap(), std::thread::current().id());
        assert!(dropped.try_recv().is_err());
    }
    let c_revision = control
        .publish_with_policy_and_context_generation(c_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    let c_output = block(&mut callback);
    assert!(
        c_output.iter().flatten().all(|value| *value < 0.0),
        "source-free C renders B's retained release tail"
    );
    assert_eq!(control.status.applied_revision.load(Acquire), c_revision);
    let installed = callback
        .active
        .as_ref()
        .and_then(ActivePublication::legacy)
        .unwrap();
    assert_eq!(
        (installed.control_context_generation, installed.revision),
        (7, c_revision)
    );
    assert_eq!(control.reclaim(), 1);
    assert_eq!(
        control.outstanding_device_resource_leases(),
        1,
        "B physically lives in C; the untriggered runtime returned on control"
    );
    #[cfg(feature = "allocation-probe")]
    assert!(
        dropped.try_recv().is_err(),
        "B cannot drop while C owns its tail"
    );
    control.command(PlaybackCommand::Stop).unwrap();
    block(&mut callback);
    let empty = Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare(
            &Project::new(8000).unwrap(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    control.publish(empty).unwrap();
    block(&mut callback);
    control.reclaim();
    assert_eq!(control.outstanding_device_resource_leases(), 0);
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(dropped.try_recv().unwrap(), std::thread::current().id());
        assert!(dropped.try_recv().is_err());
    }
    control.request_shutdown();
    block(&mut callback);
    control.reclaim();
}

#[test]
fn impossible_resource_row_budget_refuses_before_allocating() {
    let mut reservation = limits();
    reservation.handle_rows = usize::MAX;
    reservation.max_retained_bytes = 1;
    reservation.max_construction_peak_bytes = 1;
    let counted = allocation_counter::measure(|| {
        assert!(
            hex_engine::native_device_resources::DeviceResourcePublication::reserve_on_control(
                reservation,
                usize::MAX,
                usize::MAX
            )
            .is_err()
        );
    });
    assert_eq!(
        (
            counted.count_total,
            counted.bytes_total,
            counted.count_current,
            counted.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn ordinary_sampler_keyboard_rebind_honors_mute_and_leaves_other_audio_playing() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let a = source(&mut editor, &mut sources, 0.25);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, a, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let Created::Track(audio_track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(audio) = editor
        .execute(Command::AddAsset {
            path: "clip.wav".into(),
            sample_rate: 8000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset: audio,
            start: 0,
            duration: 1024,
            source_offset: 0,
            source_length: 1024,
            looping: false,
        })
        .unwrap();
    let media = BTreeMap::from([(
        audio,
        hex_engine::Audio::new(8000, vec![[0.125; 2]; 1024]).unwrap(),
    )]);
    let keys = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    keys.set_key(0, Some((60, 127, 1))).unwrap();
    let graph =
        |project: &Project,
         attach: bool,
         input: &Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>| {
            let cancel = AtomicBool::new(false);
            let resources =
                PreparedDeviceResources::prepare_on_control(&sources, project, 1_000_000, &cancel)
                    .unwrap();
            let revision = Arc::new(
                PreparedRevision::prepare_with_device_resources(
                    project,
                    &media,
                    Arc::new(SineTable::new()),
                    None,
                    &hex_engine::audio_transform::AudioTransformPreparation::default(),
                    &resources,
                    100_000,
                    &cancel,
                )
                .unwrap(),
            );
            let mut playback =
                Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap();
            if attach {
                playback
                    .prepare_computer_keyboard(project, track, Arc::clone(input))
                    .unwrap();
            }
            playback
        };
    let (mut control, mut callback) = channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph(editor.project(), true, &keys),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert!(
        block(&mut callback)
            .iter()
            .flatten()
            .any(|sample| *sample > 0.125)
    );
    for muted in [true, false] {
        editor.execute(Command::Mute { track, muted }).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph(editor.project(), true, &keys),
                PublicationPolicy::InheritRuntime,
                7,
            )
            .unwrap();
        let pcm = block(&mut callback);
        assert!(!keys.is_cancelled());
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        let expected = if muted { 0.125 } else { 0.375 };
        assert!(
            pcm.iter()
                .flatten()
                .all(|sample| (*sample - expected).abs() < 1e-6),
            "mute={muted}: {pcm:?}"
        );
        control.reclaim();
    }
    // Independent uninterrupted key-off oracle: removal releases the old
    // source through its finite envelope instead of cutting off its PCM.
    let oracle_keys = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    oracle_keys.set_key(0, Some((60, 127, 1))).unwrap();
    let mut oracle = graph(editor.project(), true, &oracle_keys);
    oracle.apply(PlaybackCommand::Play);
    oracle.render(&mut [[0.0; 2]; 96]).unwrap();
    oracle_keys.set_key(0, None).unwrap();
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph(editor.project(), false, &keys),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    for release_block in 0..14 {
        let actual = block(&mut callback);
        let mut expected = [[0.0; 2]; 32];
        oracle.render(&mut expected).unwrap();
        for (actual, expected) in actual.iter().zip(expected) {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-6,
                    "release block={release_block}: {actual:?} != {expected:?}"
                );
            }
        }
        if release_block == 0 {
            assert!(actual.iter().flatten().any(|sample| *sample > 0.125));
        }
        if release_block == 13 {
            assert!(
                actual
                    .iter()
                    .flatten()
                    .all(|sample| (*sample - 0.125).abs() < 1e-6),
                "unrelated audio continues after the sampler release is quiet"
            );
        }
    }
    assert!(keys.is_cancelled());
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
    assert_eq!(control.outstanding_device_resource_leases(), 0);
}

#[test]
fn ordinary_mutable_enable_or_restored_note_suppresses_precut_and_dispatches_exact_or_future_onsets()
 {
    for restored_note in [false, true] {
        for cut in [64, 128] {
            let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
            let mut sources = ResidentDeviceAssets::default();
            let asset = source(&mut editor, &mut sources, 0.25);
            let Created::Track(track) = editor
                .execute(Command::AddMidiTrack {
                    name: "sampler".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor.execute(Command::InsertInstrument { track, state:
            hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(1024, asset, 0.0).unwrap(),
        }).unwrap();
            let device = editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            let Created::Clip(clip) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap()
            else {
                panic!("clip")
            };
            let future = editor.project().tempo_map().frame_to_beat(128).unwrap();
            assert_eq!(
                editor.project().tempo_map().beat_to_frame(future).unwrap(),
                128
            );
            for (start, duration) in [
                (BeatTime::ZERO, BeatTime::ONE),
                (future, BeatTime::from_raw(BeatTime::ONE.raw() / 4)),
            ] {
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start,
                        duration,
                        pitch: 60,
                        channel: 0,
                        velocity: 127,
                    })
                    .unwrap();
            }
            if restored_note {
                let note = editor.project().track(track).unwrap().midi_clips()[0].notes()[0].id();
                editor.execute(Command::RemoveMidiNote(note)).unwrap();
            } else {
                editor
                    .execute(Command::SetDeviceBypass {
                        device,
                        bypassed: true,
                    })
                    .unwrap();
            }
            let disabled = compile(&editor, &sources);
            let (mut control, mut callback) = channel(8000).unwrap();
            control
                .configure_device_resource_publication_on_control(limits())
                .unwrap();
            let old_revision = control
                .publish_with_policy_and_context_generation(
                    disabled,
                    PublicationPolicy::InheritRuntime,
                    9,
                )
                .unwrap();
            control.command(PlaybackCommand::Play).unwrap();
            for _ in 0..cut / 32 {
                assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
            }
            assert_eq!(control.status.position.load(Acquire), cut);
            assert_eq!(control.status.applied_revision.load(Acquire), old_revision);
            if restored_note {
                assert!(editor.undo());
            } else {
                editor
                    .execute(Command::SetDeviceBypass {
                        device,
                        bypassed: false,
                    })
                    .unwrap();
            }
            let enabled_revision = control
                .publish_with_policy_and_context_generation(
                    compile(&editor, &sources),
                    PublicationPolicy::InheritRuntime,
                    9,
                )
                .unwrap();
            // block() measures begin/install/handoff/render together, including signed frees.
            for start in (cut..192).step_by(32) {
                let output = block(&mut callback);
                assert_eq!(output, [[if start < 128 { 0.0 } else { 0.25 }; 2]; 32]);
                assert_eq!(
                    control.status.applied_revision.load(Acquire),
                    enabled_revision
                );
            }
            assert!(control.status.rejected_revision().is_none());
            control.request_shutdown();
            for _ in 0..3 {
                block(&mut callback);
                control.reclaim();
            }
            assert_eq!(control.outstanding_device_resource_leases(), 0);
        }
    }
}

#[test]
fn exact_selection_bank_uses_ordinary_current_sampler_and_releases_without_callback_allocation() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let asset = source(&mut editor, &mut sources, 0.25);
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "selected sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, asset, 0.001,
                )
                .unwrap(),
        })
        .unwrap();
    let before = editor.project().snapshot();
    let (mut control, mut callback) = channel(8000).unwrap();
    let input = Arc::new(
        control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_note_audition(&[(60, 37), (64, 93)], 125)
            .unwrap(),
    );
    let mut actual = compile(&editor, &sources);
    actual
        .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
        .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for (pitch, velocity) in [(60, 37), (64, 93)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 32),
                pitch,
                channel: 0,
                velocity,
            })
            .unwrap();
    }
    let mut oracle = compile(&editor, &sources);
    oracle.apply(PlaybackCommand::Play);
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control
        .publish_with_policy_and_context_generation(actual, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    // Admission alone does not trigger. The eventual ACK publishes the complete bank.
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    input.trigger_note_audition().unwrap();
    let mut peak = 0.0_f32;
    let mut output = [[0.0; 2]; 25];
    for _ in 0..40 {
        let mut expected = [[0.0; 2]; 25];
        oracle.render(&mut expected).unwrap();
        let measured = allocation_counter::measure(|| {
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
        for (actual, expected) in output.iter().flatten().zip(expected.iter().flatten()) {
            peak = peak.max(actual.abs());
            assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
        }
    }
    assert!(peak > 0.001);
    assert!(input.note_audition_finished());
    assert_eq!(output, [[0.0; 2]; 25]);
    assert!(input.trigger_note_audition().is_err());
    // A newly prepared graph with the same completed immutable bank cannot replay it.
    let mut fresh = compile(&editor, &sources);
    fresh
        .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
        .unwrap();
    control
        .publish_with_policy_and_context_generation(fresh, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    assert!(!input.is_cancelled());
    control.stop();
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    assert!(input.is_cancelled());
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn clip_activation_releases_real_sampler_gates_and_retriggers_window_without_callback_allocation() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let asset = source(&mut editor, &mut sources, 0.25);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, asset, 0.002,
                )
                .unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let (mut control, mut callback) = channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control.publish(compile(&editor, &sources)).unwrap();
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let prefix = block(&mut callback);
    assert!(prefix.iter().all(|frame| frame[0] > 0.0));
    editor
        .execute(Command::SetTimeRangeActive {
            range: hex_project::TimeRange::new(vec![track], 64, 128).unwrap(),
            active: false,
        })
        .unwrap();
    let admitted = control.publish(compile(&editor, &sources)).unwrap();
    let held = block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), admitted);
    assert_eq!(held, prefix, "unmuted prefix keeps the held sample");
    let release = block(&mut callback);
    assert!(release[0][0] > 0.0);
    assert_eq!(release[31], [0.0; 2]);
    assert_eq!(
        block(&mut callback),
        [[0.0; 2]; 32],
        "muted middle after normal release"
    );
    assert_eq!(
        block(&mut callback),
        prefix,
        "right window chases remaining gate at frame128"
    );
    let right = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .last()
        .unwrap()
        .id();
    editor
        .execute(Command::SetClipActive {
            clip: right,
            active: false,
        })
        .unwrap();
    let muted = control.publish(compile(&editor, &sources)).unwrap();
    let release = block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), muted);
    assert!(release[0][0] > 0.0);
    assert_eq!(release[31], [0.0; 2]);
    assert_eq!(block(&mut callback), [[0.0; 2]; 32]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn one_voice_sampler_retrigger_steals_its_release_without_callback_allocation() {
    // A one-voice looping sampler with a 16-frame release retriggers at
    // frame 4000 (one quarter at 120 bpm, 8 kHz). Prepare admits it by held
    // density; the callback steals the release into the declick pool.
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let asset = source(&mut editor, &mut sources, 0.25);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let original =
        hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
            1024, asset, 0.002,
        )
        .unwrap();
    let mut state = hex_devices::sampler::state::SamplerState::from_native(&original).unwrap();
    state.voices = 1;
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state.replace_in(&original).unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for (start, pitch) in [(BeatTime::ZERO, 60), (BeatTime::ONE, 72)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    let (mut control, mut callback) = channel(8000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control.publish(compile(&editor, &sources)).unwrap();
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let held = block(&mut callback);
    assert!(held.iter().all(|frame| frame[0] > 0.0));
    for _ in 1..125 {
        block(&mut callback);
    }
    let steal = block(&mut callback);
    assert!(
        steal.iter().all(|frame| frame[0] > 0.0),
        "no gap at the steal"
    );
    assert!(
        steal[0][0] > held[0][0],
        "the declicked release sounds beside the new voice"
    );
    assert_eq!(
        steal[31], held[31],
        "the stolen release has retired beside the one new held voice"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn sampler_gain_modulation_renders_through_ordinary_publication_without_allocations() {
    use hex_engine::lfo_modulator::{Rate, Settings, Waveform};
    use hex_project::{
        DeviceAutomationTarget, DeviceModulation, DeviceModulationRoute, ModulationAmount,
        ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
        ModulationSourceId, Modulator,
    };

    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    let asset = source(&mut editor, &mut sources, 0.25);
    let state = hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
        1024, asset, 0.002,
    )
    .unwrap();
    let device_type = state.type_id();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sampler".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        })
        .unwrap()
    else {
        panic!("sampler track");
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("arrangement clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let next = editor.project().next_identity();
    let source = ModulationSourceId::new(next).unwrap();
    let slot = Modulator {
        id: source,
        color: Default::default(),
        enabled: true,
        state: Settings {
            rate: Rate::Hertz(2_000.0),
            phase: 0.0,
            waveform: Waveform::Square,
        }
        .encode()
        .unwrap(),
    };
    let edit = |depth| Command::ReplaceDeviceModulation {
        device,
        modulation: DeviceModulation::new(
            vec![slot.clone()],
            vec![DeviceModulationRoute {
                id: ModulationRouteId::new(next + 1).unwrap(),
                source,
                output: ModulationOutputId::MAIN,
                destination: ModulationDestination::Device {
                    device_type,
                    target: DeviceAutomationTarget::Parameter(
                        hex_lattice_contract::ParameterId::from_static(1),
                    ),
                    vst3_class: None,
                },
                amount: ModulationAmount::new(depth).unwrap(),
                range: ModulationRange::default(),
                enabled: true,
            }],
        )
        .unwrap(),
    };
    editor.execute(edit(0.125)).unwrap();
    let (mut control, mut callback) = channel(8_000).unwrap();
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    control.publish(compile(&editor, &sources)).unwrap();
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert_eq!(
        block(&mut callback),
        std::array::from_fn(|index| { [if index % 4 < 2 { 0.375 } else { 0.125 }; 2] })
    );
    editor.execute(edit(0.25)).unwrap();
    let admitted = control.publish(compile(&editor, &sources)).unwrap();
    assert_eq!(
        block(&mut callback),
        std::array::from_fn(|index| { [if index % 4 < 2 { 0.5 } else { 0.0 }; 2] })
    );
    assert_eq!(control.status.applied_revision.load(Acquire), admitted);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
