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
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(sources, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancel,
        )
        .unwrap(),
    );
    Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap()
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
        let expected = if muted { 0.0 } else { 0.25 * gain };
        assert!(
            output
                .iter()
                .flatten()
                .all(|sample| (*sample - expected).abs() < 1e-6)
        );
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
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, b, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
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
        "returning A cannot close the lease moved into B's tail"
    );
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let c_graph = compile(&editor, &sources);
    drop(sources);
    #[cfg(feature = "allocation-probe")]
    assert!(dropped.try_recv().is_err());
    let c_revision = control
        .publish_with_policy_and_context_generation(c_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    let c_output = block(&mut callback);
    assert!(
        c_output.iter().flatten().all(|value| *value > 0.0),
        "source-free C renders positive A's old tail, not negative B"
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
        "A physically lives in C; B's untriggered runtime returned on control"
    );
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(dropped.try_recv().unwrap(), std::thread::current().id());
        assert!(
            dropped.try_recv().is_err(),
            "A cannot drop while C owns its tail"
        );
    }
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
    let graph = |project: &Project, attach: bool| {
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
                .prepare_computer_keyboard(project, track, Arc::clone(&keys))
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
            graph(editor.project(), true),
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
                graph(editor.project(), true),
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
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph(editor.project(), false),
            PublicationPolicy::InheritRuntime,
            7,
        )
        .unwrap();
    assert!(
        block(&mut callback)
            .iter()
            .flatten()
            .all(|sample| (*sample - 0.125).abs() < 1e-6)
    );
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
