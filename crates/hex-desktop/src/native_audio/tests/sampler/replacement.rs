//! Original-source replacement through the actual stopped fence and staged bank.
use super::*;
use hex_engine::SamplerParameter;
use hex_session::service::{SamplerEdit, SamplerReplacementEdit, ViewState};
pub(super) fn incoming(files: &Files) -> PathBuf {
    let path = files.0.join("replacement.wav");
    let frames = [
        [8192_i16, 4096],
        [-16384, 8192],
        [4096, -8192],
        [0, 16384],
        [-8192, -4096],
        [16384, 0],
    ];
    let data = (frames.len() * 4) as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&12000_u32.to_le_bytes());
    bytes.extend_from_slice(&48000_u32.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for frame in frames {
        for sample in frame {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    fs::write(&path, bytes).unwrap();
    path
}
fn configured_current(
    mode: u8,
) -> (
    Files,
    Harness,
    hex_project::TrackId,
    hex_project::DeviceId,
    Arc<ViewState>,
) {
    let (files, mut h, track, device, _) = gain::current(24);
    wav(&files, true);
    let original_path = files.0.join("before-configured.hex");
    h.run(Request::SaveNew(original_path.clone()));
    let original = hex_storage::load(&original_path).unwrap();
    let asset = original.assets().next().unwrap().id();
    let reference =
        hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(4, asset)
            .unwrap();
    let mut state = if mode == 1 {
        reference
    } else if mode == 2 {
        hex_engine::native_device_resources::current_sampler_fixture::slice_trigger_state(4, asset)
            .unwrap()
    } else {
        hex_engine::native_device_resources::sampler_import_template_on_control(4)
            .unwrap()
            .0
            .with_host_metadata_from(&reference)
            .unwrap()
    };
    if mode == 0 {
        for (parameter, value) in [
            (SamplerParameter::Gain, 0.5),
            (SamplerParameter::Transpose, -12.0),
            (SamplerParameter::StartFrame, 1.0),
            (SamplerParameter::LoopStartFrame, 1.0),
            (SamplerParameter::LoopEnabled, 1.0),
        ] {
            state =
                hex_engine::patch_sampler_parameter_on_control(parameter, &state, value).unwrap();
        }
        state = state
            .with_input_gain_millidb(6000)
            .unwrap()
            .with_output_gain_millidb(-6000)
            .unwrap();
    }
    let mut editor = hex_project::Editor::new(original, 16).unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let configured = files.0.join("configured.hex");
    hex_storage::save_new(&configured, editor.project()).unwrap();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    h.open();
    let opened = h.run(Request::Open(configured));
    h.open();
    h.applied(h.context.unwrap());
    (files, h, track, device, opened.state)
}
pub(super) fn intent(
    state: &ViewState,
    track: hex_project::TrackId,
    path: PathBuf,
) -> SamplerReplacementEdit {
    let device = state.project.track(track).unwrap().instrument().unwrap();
    SamplerReplacementEdit {
        request_id: 73,
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        owner: hex_project::InstrumentAssetOwner::Track(track),
        device: device.id(),
        expected_state: device.state().clone(),
        path,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
pub(super) fn accept(h: &mut Harness, edit: SamplerReplacementEdit) -> Arc<ViewState> {
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let before = h.run(Request::SamplerEdit(SamplerEdit::Replacement(edit)));
    assert!(before.sampler_edit_complete.is_none());
    h.render();
    let accepted = h.next();
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(accepted.sampler_edit_complete, Some(73));
    assert!(!accepted.request_complete);
    assert_eq!(accepted.state.revision, before.state.revision + 1);
    assert!(
        !accepted
            .state
            .source_overviews
            .shares_storage_with(&before.state.source_overviews)
    );
    assert_eq!(
        before.state.source_overviews.len() + 1,
        accepted.state.source_overviews.len()
    );
    h.open_sampler(old);
    h.applied(h.context.unwrap());
    let observed = h.observation.lock().unwrap();
    let (sealed, drained) = observed.gain_cells.last().unwrap();
    assert_eq!(sealed, drained);
    accepted.state
}
fn rendered(h: &mut Harness) -> [[f32; 2]; 64] {
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    pcm
}
fn keyboard(
    h: &mut Harness,
    state: &ViewState,
    track: hex_project::TrackId,
) -> Arc<hex_engine::computer_keyboard::ComputerKeyboardInput> {
    let (output, epoch) = h
        .audio
        .as_ref()
        .unwrap()
        .control
        .reserve_sampler_keyboard_epoch()
        .unwrap();
    let input = Arc::new(
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_native_sampler_activation(
            output, epoch,
        )
        .unwrap(),
    );
    let event = h.run(Request::PrepareComputerKeyboard {
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        request_id: 91,
        track,
        input: Arc::clone(&input),
    });
    assert_eq!(event.computer_keyboard_complete, Some(91));
    input
}
fn key(h: &mut Harness, input: &hex_engine::computer_keyboard::ComputerKeyboardInput, held: bool) {
    input
        .set_key(0, if held { Some((60, 127, 1)) } else { None })
        .unwrap();
    let (output, epoch) = input.native_sampler_activation().unwrap();
    h.audio
        .as_ref()
        .unwrap()
        .control
        .sampler_keyboard_key(output, epoch, 0, input.key_word(0).unwrap())
        .unwrap();
}
fn assert_literal(actual: &[[f32; 2]; 64]) {
    let expected = [
        [0.125, 0.0625],
        [-0.015625, 0.0859375],
        [-0.15625, 0.109375],
        [-0.2109375, 0.09375],
    ];
    for (actual, expected) in actual.iter().zip(expected) {
        for channel in 0..2 {
            assert!(
                // There is no incoming audio to trim. Generated sampler PCM
                // receives the preserved -6 dB output trim (10^(-6/20)).
                (actual[channel] - expected[channel] * 0.5011872_f32).abs() < 0.0000001,
                "{actual:?} != {expected:?}"
            );
        }
    }
}
#[test]
fn sampler_replacement_commits_one_new_original_preserves_controls_resets_bounds_and_reopens_both()
{
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, before) = configured_current(0);
    let held = keyboard(&mut h, &before, track);
    key(&mut h, &held, true);
    assert!(
        h.render().iter().any(|frame| *frame != [0.0; 2]),
        "old loop must actually sound while key is held"
    );
    let old_activation = held.native_sampler_activation().unwrap();
    let path = incoming(&files);
    let changed = accept(&mut h, intent(&before, track, path.clone()));
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .sampler_keyboard_epoch_revoked(old_activation.0, old_activation.1)
    );
    assert_eq!(
        h.render(),
        [[0.0; 2]; 64],
        "held old key must not carry into replacement"
    );
    let fresh = keyboard(&mut h, &changed, track);
    key(&mut h, &fresh, true);
    assert_literal(&h.render());
    key(&mut h, &fresh, false);
    h.audio
        .as_ref()
        .unwrap()
        .control
        .cancel_sampler_keyboard_epoch(
            fresh.native_sampler_activation().unwrap().0,
            fresh.native_sampler_activation().unwrap().1,
        );
    h.render();
    let target = changed.project.track(track).unwrap().instrument().unwrap();
    assert_eq!(target.id(), device);
    assert_eq!(
        changed.project.track(track).unwrap().instrument_position(),
        before.project.track(track).unwrap().instrument_position()
    );
    assert_eq!(
        changed.project.assets().count(),
        before.project.assets().count() + 1
    );
    let state = target.state();
    for (p, value) in [
        (SamplerParameter::Gain, 0.5),
        (SamplerParameter::Transpose, -12.0),
        (SamplerParameter::StartFrame, 0.0),
        (SamplerParameter::EndFrame, 6.0),
        (SamplerParameter::LoopEnabled, 0.0),
        (SamplerParameter::LoopStartFrame, 0.0),
        (SamplerParameter::LoopEndFrame, 6.0),
    ] {
        assert_eq!(
            hex_engine::sampler_parameter_value_on_control(p, state).unwrap(),
            value
        );
    }
    let asset = changed.project.assets().find(|a| a.path() == path).unwrap();
    assert_eq!(
        (
            asset.source_sample_rate(),
            asset.source_frames(),
            asset.playback_frames()
        ),
        (12000, 6, 8)
    );
    assert_eq!(
        state.asset_references()[0].asset.value(),
        asset.id().value()
    );
    assert_eq!(
        (state.input_gain_millidb(), state.output_gain_millidb()),
        (6000, -6000)
    );
    assert_literal(&rendered(&mut h));
    // Configuration and replacement share the actual global history order.
    let mut gain = gain::edit(&changed, track, device);
    gain.value = 0.25;
    let quieter = history::complete(&mut h, Request::SamplerEdit(SamplerEdit::Parameter(gain)));
    let restored = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&quieter, false))),
    );
    assert_eq!(restored.project, changed.project);
    // Both files disappear after admission: source history uses retained originals.
    let old_path = before.project.assets().next().unwrap().path().to_path_buf();
    fs::remove_file(&old_path).unwrap();
    fs::remove_file(&path).unwrap();
    let undone = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&restored, false))),
    );
    assert_eq!(
        undone.project.device(device).unwrap().state(),
        before.project.device(device).unwrap().state()
    );
    let old_pcm = rendered(&mut h);
    for (actual, expected) in old_pcm.iter().zip([
        [0.25, 0.0],
        [0.171875, 0.03125],
        [0.09375, 0.0625],
        [0.015625, 0.09375],
    ]) {
        for channel in 0..2 {
            assert!((actual[channel] - expected[channel] * 0.5011872_f32).abs() < 0.0000001);
        }
    }
    assert_eq!(undone.project.assets().count(), 1);
    assert_eq!(
        undone.source_overviews.len(),
        2,
        "unused source backing remains charged"
    );
    assert!(undone.source_overviews.get(asset.id()).is_some());
    let undone_path = files.0.join("undone.hex");
    h.run(Request::SaveNew(undone_path.clone()));
    let undone_saved = hex_storage::load(&undone_path).unwrap();
    assert_eq!(
        undone_saved.assets().count(),
        1,
        "retained bank rows are not invented canonical assets"
    );
    assert_eq!(
        undone_saved.device(device).unwrap().state(),
        before.project.device(device).unwrap().state()
    );
    let restored = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&undone, true))),
    );
    assert_eq!(
        restored.project.device(device).unwrap().state(),
        changed.project.device(device).unwrap().state()
    );
    assert_eq!(restored.project.assets().count(), 2);
    assert_literal(&rendered(&mut h));
    incoming(&files);
    // A repeated import retains the previous unused four-frame original. Its
    // additional bank+native overlap exceeds24, rather than silently collecting it.
    let cells = h.observation.lock().unwrap().gain_cells.len();
    h.run(Request::SamplerEdit(SamplerEdit::Replacement(intent(
        &restored,
        track,
        path.clone(),
    ))));
    h.render();
    let refused = h.next();
    assert!(refused.error.as_ref().unwrap().contains("frame"));
    gain::unchanged(&restored, &refused);
    assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
    assert_literal(&rendered(&mut h));
    // The original fixture deliberately unlinked old media. Restore identical
    // bytes so both retained canonical originals can be verified on real Open.
    wav(&files, true);
    let saved = files.0.join("replaced.hex");
    h.run(Request::SaveNew(saved.clone()));
    let stored = hex_storage::load(&saved).unwrap();
    assert_eq!(stored.assets().count(), 2);
    assert!(stored.assets().all(|a| a.original_source().is_some()));
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced);
    h.open();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(saved));
    assert_eq!(opened.state.project, Arc::new(stored));
    // Generation replacement already detaches the old client in Harness::next.
    h.open();
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(old_output)
    );
    h.applied(h.context.unwrap());
    assert_literal(&rendered(&mut h));
}
#[test]
fn sampler_replacement_stale_cancel_corrupt_postseal_and_tight_frames_preserve_old_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for tight in [false, true] {
        let (files, mut h, track, _, before) = gain::current(if tight { 12 } else { 24 });
        let path = incoming(&files);
        if !tight {
            for case in 0..4 {
                let mut edit = intent(&before, track, path.clone());
                match case {
                    0 => edit.revision += 1,
                    1 => edit.project_generation += 1,
                    2 => edit
                        .cancel
                        .store(true, std::sync::atomic::Ordering::Release),
                    _ => {
                        edit.expected_state =
                            edit.expected_state.with_output_gain_millidb(1000).unwrap()
                    }
                }
                h.service
                    .as_ref()
                    .unwrap()
                    .submit(Request::SamplerEdit(SamplerEdit::Replacement(edit)))
                    .unwrap();
                let refused = h.next();
                assert!(refused.error.is_some());
                gain::unchanged(&before, &refused);
            }
            let corrupt = files.0.join("corrupt.wav");
            fs::write(&corrupt, b"not wave").unwrap();
            for path in [corrupt, files.0.join("missing.wav")] {
                h.run(Request::SamplerEdit(SamplerEdit::Replacement(intent(
                    &before, track, path,
                ))));
                h.render();
                let refused = h.next();
                assert!(refused.error.is_some());
                gain::unchanged(&before, &refused);
            }
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap();
            h.run(Request::SamplerEdit(SamplerEdit::Replacement(intent(
                &before,
                track,
                path.clone(),
            ))));
            assert_eq!(h.render()[0], [0.125, -0.25]);
            let refused = h.next();
            assert!(
                refused
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("sampler edit audio changed")
            );
            gain::unchanged(&before, &refused);
            h.audio.as_ref().unwrap().control.stop();
            h.render();
            let pending = intent(&before, track, path.clone());
            let cancel = Arc::clone(&pending.cancel);
            h.run(Request::SamplerEdit(SamplerEdit::Replacement(pending)));
            let mut next = intent(&before, track, path.clone());
            next.request_id = 74;
            h.service
                .as_ref()
                .unwrap()
                .submit(Request::SamplerEdit(SamplerEdit::Replacement(next)))
                .unwrap();
            let refused = h.next();
            assert!(
                refused
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("already preparing")
            );
            assert_eq!(refused.sampler_edit_complete, Some(74));
            gain::unchanged(&before, &refused);
            cancel.store(true, std::sync::atomic::Ordering::Release);
            h.render();
            let refused = h.next();
            assert!(refused.error.is_some());
            gain::unchanged(&before, &refused);
            let edit = intent(&before, track, path.clone());
            h.observation.lock().unwrap().cancel_edit_after_seal = Some(Arc::clone(&edit.cancel));
            h.run(Request::SamplerEdit(SamplerEdit::Replacement(edit)));
            h.render();
            let refused = h.next();
            assert!(refused.error.as_ref().unwrap().contains("cancelled"));
            gain::unchanged(&before, &refused);
            assert!(h.observation.lock().unwrap().gain_prepared.is_some());
            assert!(h.observation.lock().unwrap().gain_cells.is_empty());
        } else {
            h.run(Request::SamplerEdit(SamplerEdit::Replacement(intent(
                &before, track, path,
            ))));
            h.render();
            let refused = h.next();
            assert!(refused.error.is_some());
            gain::unchanged(&before, &refused);
            assert!(h.observation.lock().unwrap().gain_prepared.is_none());
        }
        gain::original_pcm(&mut h);
    }
}

#[test]
fn sampler_replacement_unsupported_mode_refuses_before_media_decode_or_audio_fence() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in [1, 2] {
        let (files, mut h, track, _, before) = configured_current(mode);
        let original = rendered(&mut h);
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SamplerEdit(SamplerEdit::Replacement(intent(
                &before,
                track,
                files.0.join("never-created.wav"),
            ))))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.as_ref().unwrap().contains("classic mode"));
        assert_eq!(refused.sampler_edit_complete, Some(73));
        gain::unchanged(&before, &refused);
        assert!(h.observation.lock().unwrap().gain_prepared.is_none());
        assert_eq!(rendered(&mut h), original);
    }
}

#[test]
fn sampler_replacement_history_keeps_uncatalogued_originals_charged_and_refusal_keeps_redo() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, initial) = gain::current(20);
    let six_path = incoming(&files);
    let six = accept(&mut h, intent(&initial, track, six_path.clone()));
    let two_path = files.0.join("two.wav");
    let mut bytes = fs::read(&six_path).unwrap();
    bytes.truncate(44 + 8);
    bytes[4..8].copy_from_slice(&44_u32.to_le_bytes());
    bytes[40..44].copy_from_slice(&8_u32.to_le_bytes());
    fs::write(&two_path, bytes).unwrap();
    let two = accept(&mut h, intent(&six, track, two_path.clone()));
    assert_eq!(two.project.assets().count(), 3);
    fs::remove_file(six_path).unwrap();
    fs::remove_file(two_path).unwrap();
    let six_again = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&two, false))),
    );
    assert_eq!(
        six_again.project.device(device).unwrap().state(),
        six.project.device(device).unwrap().state()
    );
    assert_eq!(six_again.project.assets().count(), 2);
    assert_eq!(six_again.source_overviews.len(), 3);
    let expected = rendered(&mut h);
    assert_eq!(
        &expected[..4],
        &[
            [0.25, 0.125],
            [-0.3125, 0.21875],
            [-0.1875, 0.0],
            [0.09375, -0.0625]
        ]
    );
    let cells = h.observation.lock().unwrap().gain_cells.len();
    // Twelve original frames remain owned, including the uncatalogued two.
    // Current six plus candidate four would require22, beyond the real limit20.
    for _ in 0..2 {
        h.run(Request::SamplerEdit(SamplerEdit::History(
            history::history(&six_again, false),
        )));
        h.render();
        let refused = h.next();
        assert!(
            refused.error.as_ref().unwrap().contains("frame"),
            "{:?}",
            refused.error
        );
        gain::unchanged(&six_again, &refused);
        assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
        assert_eq!(rendered(&mut h), expected);
    }
    let redone = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&six_again, true))),
    );
    assert_eq!(
        redone.project.device(device).unwrap().state(),
        two.project.device(device).unwrap().state()
    );
    assert_eq!(redone.project.assets().count(), 3);
    assert_eq!(redone.source_overviews.len(), 3);
}

#[test]
fn sampler_replacement_history_stale_cancel_playing_and_postseal_refuse_without_consuming() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, _, initial) = gain::current(24);
    let changed = accept(&mut h, intent(&initial, track, incoming(&files)));
    let expected = rendered(&mut h);
    for case in 0..5 {
        let mut edit = history::history(&changed, false);
        match case {
            0 => edit.revision += 1,
            1 => edit
                .cancel
                .store(true, std::sync::atomic::Ordering::Release),
            2 => h
                .audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .unwrap(),
            3 => {
                h.observation.lock().unwrap().cancel_edit_after_seal =
                    Some(Arc::clone(&edit.cancel))
            }
            _ => edit.project_generation += 1,
        }
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SamplerEdit(SamplerEdit::History(edit)))
            .unwrap();
        let event = h.next();
        let refused = if event.sampler_edit_complete.is_some() {
            event
        } else {
            h.render();
            h.next()
        };
        assert!(refused.error.is_some(), "case{case}");
        gain::unchanged(&changed, &refused);
        assert_eq!(rendered(&mut h), expected);
    }
    let restored = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&changed, false))),
    );
    assert_eq!(restored.project.assets().count(), 1);
    gain::original_pcm(&mut h);
}

#[test]
fn unassigned_sampler_replacement_commits_and_undoes_through_private_fence() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, original) = gain::current(24);
    let source = original.project.track(track).unwrap().source_id().unwrap();
    wav(&files, true);
    let saved = files.0.join("source-bound.hex");
    h.run(Request::SaveNew(saved.clone()));
    let mut project = hex_project::Editor::new(hex_storage::load(&saved).unwrap(), 16).unwrap();
    project
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    let state = project.project().device(device).unwrap().state().clone();
    project
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let empty = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.sampler"),
    )
    .unwrap();
    let hex_project::Created::NativeSource(empty_source) = project
        .execute(Command::CreateNativeSource {
            name: "empty".into(),
            devices: vec![empty],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("empty source")
    };
    let empty_device = project
        .project()
        .native_source(empty_source)
        .unwrap()
        .instrument()
        .id();
    let path = files.0.join("source-unassigned.hex");
    hex_storage::save_new(&path, project.project()).unwrap();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    h.open();
    let opened = h.run(Request::Open(path));
    h.open();
    h.applied(h.context.unwrap());
    let before = opened.state;
    let make_edit = || {
        let mut edit = intent(&before, track, incoming(&files));
        edit.owner = hex_project::InstrumentAssetOwner::Source(source);
        edit.device = device;
        edit.expected_state = before.project.device(device).unwrap().state().clone();
        edit
    };
    let cancelled = make_edit();
    cancelled
        .cancel
        .store(true, std::sync::atomic::Ordering::Release);
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(SamplerEdit::Replacement(cancelled)))
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    gain::unchanged(&before, &refused);
    let changed = accept(&mut h, make_edit());
    assert_eq!(changed.project.source_contributors(source).count(), 0);
    assert_eq!(
        changed
            .project
            .native_source(source)
            .unwrap()
            .instrument()
            .id(),
        device
    );
    assert_ne!(
        changed.project.device(device).unwrap().state(),
        before.project.device(device).unwrap().state()
    );
    let undone = super::history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(super::history::history(
            &changed, false,
        ))),
    );
    assert_eq!(
        undone.project.device(device).unwrap().state(),
        before.project.device(device).unwrap().state()
    );
    let redone = super::history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(super::history::history(&undone, true))),
    );
    assert_eq!(
        redone.project.device(device).unwrap().state(),
        changed.project.device(device).unwrap().state()
    );
    let mut first = intent(&redone, track, incoming(&files));
    first.owner = hex_project::InstrumentAssetOwner::Source(empty_source);
    first.device = empty_device;
    first.expected_state = redone.project.device(empty_device).unwrap().state().clone();
    let loaded = accept(&mut h, first);
    assert_eq!(loaded.project.source_contributors(empty_source).count(), 0);
    assert!(!hex_engine::is_empty_sampler_state_on_control(
        loaded.project.device(empty_device).unwrap().state()
    ));
    let empty_again = super::history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(super::history::history(
            &loaded, false,
        ))),
    );
    assert!(hex_engine::is_empty_sampler_state_on_control(
        empty_again.project.device(empty_device).unwrap().state()
    ));
    let loaded_again = super::history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(super::history::history(
            &empty_again,
            true,
        ))),
    );
    assert_eq!(
        loaded_again.project.device(empty_device).unwrap().state(),
        loaded.project.device(empty_device).unwrap().state()
    );
}
