//! Native scalar edit/fence/custody regressions; rendered controls live in controls.rs.
use super::*;
use hex_engine::SamplerParameter::{
    EndFrame, Gain, LoopEnabled, LoopEndFrame, LoopStartFrame, StartFrame, Transpose,
};

pub(super) fn current(
    limit: u64,
) -> (
    Files,
    Harness,
    hex_project::TrackId,
    hex_project::DeviceId,
    Arc<hex_session::service::ViewState>,
) {
    let files = Files::new();
    let mut h = Harness::resident_sampler(limit);
    let (track, asset) = authored(&mut h, &files, true, false, false);
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
    (files, h, track, device, assigned.state)
}
pub(super) fn edit(
    state: &hex_session::service::ViewState,
    track: hex_project::TrackId,
    device: hex_project::DeviceId,
) -> hex_session::service::SamplerParameterEdit {
    hex_session::service::SamplerParameterEdit {
        request_id: 1,
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        owner: hex_session::service::SamplerParameterOwner::Track(track),
        device,
        parameter: hex_engine::SamplerParameter::Gain,
        value: 0.5,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
pub(super) fn unchanged(
    before: &hex_session::service::ViewState,
    after: &hex_session::service::Event,
) {
    assert_eq!(after.state.revision, before.revision);
    assert_eq!(after.state.project.snapshot(), before.project.snapshot());
    assert_eq!(
        (after.state.can_undo, after.state.can_redo),
        (before.can_undo, before.can_redo)
    );
    assert_eq!(after.state.document_path, before.document_path);
    assert!(
        after
            .state
            .source_overviews
            .shares_storage_with(&before.source_overviews)
    );
}
pub(super) fn original_pcm(h: &mut Harness) {
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    assert_eq!(pcm[0], [0.125, -0.25]);
    assert_eq!(pcm[2], [0.5, 0.0]);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        identity
    );
}
#[test]
fn sampler_gain_stale_invalid_cancelled_and_tight_budget_refusals_preserve_actual_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(8);
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    for parameter in [
        Gain,
        Transpose,
        StartFrame,
        EndFrame,
        LoopEnabled,
        LoopStartFrame,
        LoopEndFrame,
        hex_engine::SamplerParameter::AttackMs,
        hex_engine::SamplerParameter::DecayMs,
        hex_engine::SamplerParameter::SustainPercent,
        hex_engine::SamplerParameter::ReleaseMs,
    ] {
        for case in 0..6 {
            let mut gain = edit(&before, track, device);
            gain.parameter = parameter;
            match case {
                0 => gain.value = f64::NAN,
                1 => gain.value = parameter.bounds().1 + 0.01,
                2 => gain.value = parameter.bounds().0 - 0.1,
                3 => gain.revision += 1,
                4 => gain.project_generation += 1,
                _ => gain
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Release),
            }
            h.service
                .as_ref()
                .unwrap()
                .submit(Request::SamplerEdit(
                    hex_session::service::SamplerEdit::Parameter(gain),
                ))
                .unwrap();
            let refused = h.next();
            assert!(refused.error.is_some());
            unchanged(&before, &refused);
            assert_eq!(refused.sampler_edit_complete, Some(1));
        }
    }
    original_pcm(&mut h);
    let initial = h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(edit(&before, track, device)),
    ));
    assert!(initial.sampler_edit_complete.is_none());
    unchanged(&before, &initial);
    h.render();
    let refused = h.next();
    assert!(
        refused.error.as_ref().unwrap().contains("frame limit"),
        "{:?}",
        refused.error
    );
    assert!(
        !refused.request_complete,
        "worker continuation is not another UI acknowledgement"
    );
    unchanged(&before, &refused);
    assert_eq!(refused.sampler_edit_complete, Some(1));
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        identity
    );
    assert_eq!(h.observation.lock().unwrap().native_hold.frames, 4);
    original_pcm(&mut h);
}
#[test]
fn sampler_gain_fence_observes_earlier_play_and_refuses_later_play_without_replay() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(edit(&before, track, device)),
    ));
    assert_eq!(h.render()[0], [0.125, -0.25]);
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("sampler edit audio changed")
    );
    unchanged(&before, &refused);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    let gain = edit(&before, track, device);
    let cancel = Arc::clone(&gain.cancel);
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(gain),
    ));
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_err()
    );
    cancel.store(true, std::sync::atomic::Ordering::Release);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let cancelled = h.next();
    assert!(cancelled.error.is_some());
    unchanged(&before, &cancelled);
    for _ in 0..3 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
    original_pcm(&mut h);
}

#[test]
fn sampler_gain_postcommit_open_failure_keeps_authored_gain_save_export_and_explicit_retry() {
    postcommit_open_failure(FailedEdit::Gain);
}
#[test]
fn sampler_history_postcommit_open_failure_keeps_authored_history_save_export_and_explicit_retry() {
    postcommit_open_failure(FailedEdit::History);
}
#[test]
fn sampler_replacement_postcommit_open_failure_keeps_new_source_save_export_and_explicit_retry() {
    postcommit_open_failure(FailedEdit::Replacement);
}
#[test]
fn sampler_replacement_history_postcommit_open_failure_keeps_restored_source_save_export_and_retry()
{
    postcommit_open_failure(FailedEdit::ReplacementHistory);
}
#[test]
fn sampler_midi_postcommit_open_failure_keeps_composition_save_export_and_explicit_retry() {
    postcommit_open_failure(FailedEdit::Midi);
}
#[derive(Clone, Copy)]
enum FailedEdit {
    Midi,
    Gain,
    History,
    Replacement,
    ReplacementHistory,
}
fn postcommit_open_failure(kind: FailedEdit) {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, before) = current(
        if matches!(
            kind,
            FailedEdit::Replacement | FailedEdit::ReplacementHistory
        ) {
            24
        } else {
            12
        },
    );
    let (before, request, expected_gain) = if matches!(kind, FailedEdit::Midi) {
        let request = Request::SamplerEdit(hex_session::service::SamplerEdit::Midi(
            hex_session::service::SamplerMidiEdit {
                request_id: 44,
                project_generation: before
                    .canonical_audio_source
                    .as_ref()
                    .unwrap()
                    .context()
                    .project_generation,
                revision: before.revision,
                command: Box::new(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
                    length: BeatTime::ONE,
                }),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        ));
        (before, request, 1.0)
    } else if matches!(kind, FailedEdit::Replacement) {
        let request = Request::SamplerEdit(hex_session::service::SamplerEdit::Replacement(
            super::replacement::intent(&before, track, super::replacement::incoming(&files)),
        ));
        (before, request, 1.0)
    } else if matches!(kind, FailedEdit::ReplacementHistory) {
        let changed = super::replacement::accept(
            &mut h,
            super::replacement::intent(&before, track, super::replacement::incoming(&files)),
        );
        let request = Request::SamplerEdit(hex_session::service::SamplerEdit::History(
            super::history::history(&changed, false),
        ));
        (changed, request, 1.0)
    } else if matches!(kind, FailedEdit::History) {
        let changed = super::history::complete(
            &mut h,
            Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(edit(
                &before, track, device,
            ))),
        );
        let request = Request::SamplerEdit(hex_session::service::SamplerEdit::History(
            super::history::history(&changed, false),
        ));
        (changed, request, 1.0)
    } else {
        let request = Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(edit(
            &before, track, device,
        )));
        (before, request, 0.5)
    };
    let expected_pcm = if matches!(kind, FailedEdit::Replacement) {
        [0.25_f32, 0.125]
    } else {
        [0.125 * expected_gain as f32, -0.25 * expected_gain as f32]
    };
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.observation.lock().unwrap().fail_gain_open = true;
    h.run(request);
    h.render();
    let accepted = h.next();
    assert!(accepted.error.is_none());
    assert_eq!(accepted.state.revision, before.revision + 1);
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::Gain,
            accepted.state.project.device(device).unwrap().state()
        )
        .unwrap(),
        expected_gain
    );
    if matches!(kind, FailedEdit::Midi) {
        assert_eq!(
            accepted
                .state
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
    }
    let context = h.context.unwrap();
    let end = Instant::now() + h.wait;
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::RelinquishClient { .. }) => {
                h.audio = None;
                h.backend.lock().unwrap().take();
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => {
                assert!(error.to_string().contains("fixture hardware open failed"));
                h.audio = None;
                h.backend.lock().unwrap().take();
                break;
            }
            Ok(EditorAudioEvent::Opened { .. }) => {
                panic!("failed hardware open must not report an opened/applied output")
            }
            _ => {}
        }
        assert!(
            Instant::now() < end,
            "postcommit hardware failure was not reported"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let saved = files.0.join("gain-after-open-failure.hex");
    h.run(Request::SaveNew(saved.clone()));
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::Gain,
            hex_storage::load(&saved)
                .unwrap()
                .device(device)
                .unwrap()
                .state()
        )
        .unwrap(),
        expected_gain
    );
    let path = files.0.join("gain-after-open-failure.wav");
    h.run(Request::ExportRange {
        path: path.clone(),
        start: 0,
        frames: Some(8),
        signal: hex_session::service::ExportSignal::Main,
        format: hex_export::ExportFormat::Pcm16,
        overwrite: false,
        control: hex_export_control(),
    });
    loop {
        if let Ok(event) = h.service.as_ref().unwrap().try_export_event() {
            assert!(event.result.is_ok(), "{:?}", event.result);
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let bytes = fs::read(path).unwrap();
    // PCM16 export applies ±1 LSB TPDF dither.
    for (offset, expected) in [(44, expected_pcm[0]), (46, expected_pcm[1])] {
        let actual = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let expected = (expected * 32768.0) as i16;
        assert!(
            (i32::from(actual) - i32::from(expected)).abs() <= 1,
            "{actual} not within one lsb of {expected}"
        );
    }
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::RestartAudio {
            project_generation: context.project_generation,
            request_id: 77,
        })
        .unwrap();
    let retry = h.next();
    assert!(retry.error.is_none(), "{:?}", retry.error);
    loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::Opened { audio, .. }) => h.audio = Some(audio),
            Ok(EditorAudioEvent::RelinquishClient { .. }) => {
                h.audio = None;
                h.backend.lock().unwrap().take();
            }
            Ok(EditorAudioEvent::Unavailable { error, .. }) => panic!("retry unavailable: {error}"),
            _ => {}
        }
        let mut slot = h.backend.lock().unwrap();
        if let Some(backend) = slot.as_mut() {
            if backend.shutdown_requested() {
                slot.take();
            } else {
                let measured =
                    allocation_counter::measure(|| backend.render(&mut [[0.0; 2]; 64]).unwrap());
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
        }
        drop(slot);
        if h.audio
            .as_ref()
            .and_then(|a| a.status())
            .and_then(|s| s.applied_audio_source())
            .is_some_and(|s| s.context == context && s.output_identity != old)
        {
            break;
        }
        assert!(
            Instant::now() < end,
            "explicit retry did not reach actual CURRENT"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_eq!(h.render()[0], expected_pcm);
}

#[test]
fn sampler_gain_lost_callback_timeout_preserves_earlier_play_and_allows_new_teardown() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let old_output = h.audio.as_ref().unwrap().control.output_identity();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(edit(&before, track, device)),
    ));
    h.observation.lock().unwrap().expire_gain_fence = true;
    let timeout = h.next();
    assert!(timeout.error.as_ref().unwrap().contains("timed out"));
    unchanged(&before, &timeout);
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_err()
    );
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .control
            .reserve_sampler_keyboard_epoch()
            .is_err()
    );
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(edit(&before, track, device)),
        ))
        .unwrap();
    let busy = h.next();
    assert!(busy.error.as_ref().unwrap().contains("busy"));
    unchanged(&before, &busy);
    // The callback returns only now: the Play admitted before the timed-out
    // fence is still honored, not reclassified as a raced new command.
    assert_eq!(h.render()[0], [0.125, -0.25]);
    let end = Instant::now() + h.wait;
    loop {
        if h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    h.audio.as_ref().unwrap().control.stop();
    for _ in 0..3 {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
    // A second callback loss may be abandoned through actual New/output
    // teardown; the old scalar fence is never treated as a source receipt.
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(edit(&before, track, device)),
    ));
    h.observation.lock().unwrap().expire_gain_fence = true;
    let timeout = h.next();
    assert!(timeout.error.as_ref().unwrap().contains("timed out"));
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced);
    assert!(new.state.project.tracks().is_empty());
    h.open();
    // The empty New project uses the ordinary path, which deliberately has no
    // constructor AppliedAudioSource. A genuinely opened different output
    // proves the old exclusive lease was retired, not a fabricated sampler ACK.
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        old_output
    );
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert_eq!(h.observation.lock().unwrap().native_hold.frames, 0);
}

#[test]
fn sampler_gain_cancel_after_real_sealing_and_fenced_late_keyboard_preserve_old_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let output = h.audio.as_ref().unwrap().control.output_identity();
    let gain = edit(&before, track, device);
    h.observation.lock().unwrap().cancel_edit_after_seal = Some(Arc::clone(&gain.cancel));
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(gain),
    ));
    h.render();
    let cancelled = h.next();
    assert!(cancelled.error.as_ref().unwrap().contains("cancelled"));
    unchanged(&before, &cancelled);
    assert!(
        h.observation.lock().unwrap().gain_prepared.is_some(),
        "must reach actual sealed cell before cancellation"
    );
    assert!(
        h.observation.lock().unwrap().gain_cells.is_empty(),
        "cancelled cell must never enter output drain/adoption"
    );
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    original_pcm(&mut h);
    // Reserve before the fence but deliver activation after it began. This
    // exact old output/target must still refuse the late keyboard activation.
    let (identity, epoch) = h
        .audio
        .as_ref()
        .unwrap()
        .control
        .reserve_sampler_keyboard_epoch()
        .unwrap();
    let input = Arc::new(
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_native_sampler_activation(
            identity, epoch,
        )
        .unwrap(),
    );
    input.set_key(0, Some((60, 127, 1))).unwrap();
    let gain = edit(&before, track, device);
    let cancel = Arc::clone(&gain.cancel);
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(gain),
    ));
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::PrepareComputerKeyboard {
            project_generation: before
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            revision: before.revision,
            request_id: 99,
            track,
            input: Arc::clone(&input),
        })
        .unwrap();
    let late = h.next();
    assert!(late.error.is_some());
    assert_eq!(late.computer_keyboard_complete, Some(99));
    assert!(input.is_cancelled());
    cancel.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let cancelled = h.next();
    assert!(cancelled.error.is_some());
    unchanged(&before, &cancelled);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    original_pcm(&mut h);
}

#[test]
fn sampler_gain_fence_cancels_actual_old_epoch_before_deferring_a_new_bound_epoch() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let activate = |h: &mut Harness, request_id: u64| {
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
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let event = h.run(Request::PrepareComputerKeyboard {
            project_generation: before
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            revision: before.revision,
            request_id,
            track,
            input: Arc::clone(&input),
        });
        assert_eq!(event.computer_keyboard_complete, Some(request_id));
        h.audio
            .as_ref()
            .unwrap()
            .control
            .sampler_keyboard_key(output, epoch, 0, input.key_word(0).unwrap())
            .unwrap();
        (output, epoch, input)
    };
    let render_one = |h: &Harness| {
        let mut output = [[0.0; 2]; 1];
        let mut slot = h.backend.lock().unwrap();
        let backend = slot.as_mut().unwrap();
        let measured = allocation_counter::measure(|| backend.render(&mut output).unwrap());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        output
    };
    let (output, a, input_a) = activate(&mut h, 101);
    assert_eq!(
        render_one(&h),
        [[0.125, -0.25]],
        "A must actually sound and remain alive after one frame"
    );
    input_a.cancel();
    h.audio
        .as_ref()
        .unwrap()
        .control
        .cancel_sampler_keyboard_epoch(output, a);
    let (_, b, _input_b) = activate(&mut h, 102);
    assert!(b > a);
    // No callback has observed B, or A's terminal cancellation, yet.
    let gain = edit(&before, track, device);
    let cancel = Arc::clone(&gain.cancel);
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::Parameter(gain),
    ));
    let (entered, arrival) = mpsc::sync_channel(1);
    let (release, resume) = mpsc::sync_channel(0);
    h.observation.lock().unwrap().pause_poll = Some((entered, resume));
    arrival.recv_timeout(h.wait).unwrap();
    for _ in 0..3 {
        assert_eq!(
            render_one(&h),
            [[0.0; 2]],
            "cancelled actual A must stop even though newer bound B is deferred behind the gain fence"
        );
    }
    cancel.store(true, std::sync::atomic::Ordering::Release);
    h.audio
        .as_ref()
        .unwrap()
        .control
        .cancel_sampler_keyboard_epoch(output, b);
    release.send(()).unwrap();
    let cancelled = h.next();
    assert!(cancelled.error.is_some());
    unchanged(&before, &cancelled);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

#[test]
fn sampler_region_actual_source_extent_refuses_before_canonical_commit_or_output_drain() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, before) = current(12);
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    for parameter in [EndFrame, LoopEndFrame] {
        let mut request = edit(&before, track, device);
        request.parameter = parameter;
        request.value = 5.0; // Codec/global limit allows it; actual retained WAV has4 frames.
        assert!(
            hex_engine::patch_sampler_parameter_on_control(
                parameter,
                before.project.device(device).unwrap().state(),
                5.0
            )
            .is_ok()
        );
        let pending = h.run(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(request),
        ));
        assert!(pending.error.is_none());
        assert_eq!(pending.sampler_edit_complete, None);
        unchanged(&before, &pending);
        h.render();
        let refused = h.next();
        assert!(refused.error.is_some());
        assert_eq!(refused.sampler_edit_complete, Some(1));
        unchanged(&before, &refused);
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            identity
        );
        original_pcm(&mut h);
    }
}

#[test]
fn sampler_dormant_trim_enable_conflict_and_active_loop_bounds_preserve_history_and_current_pcm() {
    fn publish(
        h: &mut Harness,
        before: &hex_session::service::ViewState,
        track: hex_project::TrackId,
        device: hex_project::DeviceId,
        parameter: hex_engine::SamplerParameter,
        value: f64,
    ) -> Arc<hex_session::service::ViewState> {
        let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let mut request = edit(before, track, device);
        request.parameter = parameter;
        request.value = value;
        let pending = h.run(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(request),
        ));
        assert!(pending.error.is_none(), "{:?}", pending.error);
        h.render();
        let committed = h.next();
        assert!(committed.error.is_none(), "{:?}", committed.error);
        h.open_sampler(old);
        h.applied(h.context.unwrap());
        committed.state
    }
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, initial) = current(12);
    let trimmed = publish(&mut h, &initial, track, device, StartFrame, 2.0);
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            LoopStartFrame,
            trimmed.project.device(device).unwrap().state()
        )
        .unwrap(),
        0.0
    );
    let old = h.audio.as_ref().unwrap().control.output_identity();
    let mut enable = edit(&trimmed, track, device);
    enable.parameter = LoopEnabled;
    enable.value = 1.0;
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(enable),
        ))
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    unchanged(&trimmed, &refused);
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), old);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    assert_eq!(&pcm[..3], &[[-0.125, 0.25], [-0.0625, 0.125], [0.0; 2]]);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    let repaired = publish(&mut h, &trimmed, track, device, LoopStartFrame, 2.0);
    let active = publish(&mut h, &repaired, track, device, LoopEnabled, 1.0);
    let old = h.audio.as_ref().unwrap().control.output_identity();
    let mut outside = edit(&active, track, device);
    outside.parameter = LoopStartFrame;
    outside.value = 1.0;
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(outside),
        ))
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    unchanged(&active, &refused);
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), old);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    let period = [[-0.125, 0.25], [-0.0625, 0.125], [0.0; 2], [-0.0625, 0.125]];
    for (index, frame) in pcm.iter().enumerate() {
        assert_eq!(*frame, period[index % 4]);
    }
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}

#[test]
fn sampler_envelope_roundtrip_no_change_never_fences_drains_or_changes_history() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, mut before) = current(12);
    for (parameter, value) in [
        (hex_engine::SamplerParameter::AttackMs, 63.7),
        (hex_engine::SamplerParameter::SustainPercent, 0.9),
    ] {
        let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        let mut request = edit(&before, track, device);
        request.parameter = parameter;
        request.value = value;
        h.run(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(
                hex_session::service::SamplerParameterEdit {
                    cancel: Arc::clone(&request.cancel),
                    ..request
                },
            ),
        ));
        h.render();
        let committed = h.next();
        assert!(committed.error.is_none(), "{:?}", committed.error);
        h.open_sampler(old);
        h.applied(h.context.unwrap());
        before = committed.state;
        assert_ne!(
            hex_engine::sampler_parameter_value_on_control(
                parameter,
                before.project.device(device).unwrap().state()
            )
            .unwrap(),
            value
        );
        request.revision = before.revision;
        let identity = h.audio.as_ref().unwrap().control.output_identity();
        let cells = h.observation.lock().unwrap().gain_cells.len();
        // No callback is run: a real fence would remain pending here.
        let same = h.run(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(request),
        ));
        assert_eq!(same.sampler_edit_complete, Some(1));
        assert_eq!(same.status.as_deref(), Some("no change"));
        unchanged(&before, &same);
        assert_eq!(
            h.audio.as_ref().unwrap().control.output_identity(),
            identity
        );
        assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
        // Non-Stop producer work must remain accepted immediately: no fence.
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render();
        h.audio.as_ref().unwrap().control.stop();
        assert_eq!(h.render(), [[0.0; 2]; 64]);
    }
}

#[test]
fn unassigned_sampler_gain_commits_and_undoes_through_private_fence() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, original) = current(12);
    let source = original.project.track(track).unwrap().source_id().unwrap();
    wav(&files, true);
    let saved = files.0.join("bound.hex");
    h.run(Request::SaveNew(saved.clone()));
    let mut project = hex_project::Editor::new(hex_storage::load(&saved).unwrap(), 16).unwrap();
    project
        .execute(Command::BindTrackSource {
            track,
            source: None,
        })
        .unwrap();
    // Keep a different bound sampler current while the original source remains unassigned.
    let state = project.project().device(device).unwrap().state().clone();
    project
        .execute(Command::InsertInstrument { track, state })
        .unwrap();
    let unbound = files.0.join("unbound.hex");
    hex_storage::save_new(&unbound, project.project()).unwrap();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    h.open();
    let opened = h.run(Request::Open(unbound));
    h.open();
    h.applied(h.context.unwrap());
    let before = opened.state;
    assert_ne!(
        before.project.track(track).unwrap().source_id(),
        Some(source)
    );
    assert_eq!(before.project.source_contributors(source).count(), 0);
    let mut request = edit(&before, track, device);
    request.owner = hex_session::service::SamplerParameterOwner::Source(source);
    request
        .cancel
        .store(true, std::sync::atomic::Ordering::Release);
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(
            hex_session::service::SamplerEdit::Parameter(request),
        ))
        .unwrap();
    let refused = h.next();
    assert!(refused.error.is_some());
    unchanged(&before, &refused);
    let mut request = edit(&before, track, device);
    request.owner = hex_session::service::SamplerParameterOwner::Source(source);
    let changed = super::history::complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(request)),
    );
    assert_ne!(
        changed.project.track(track).unwrap().source_id(),
        Some(source)
    );
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
    assert_eq!(
        hex_engine::sampler_parameter_value_on_control(
            Gain,
            changed.project.device(device).unwrap().state(),
        )
        .unwrap(),
        0.5
    );
    let undone = super::history::complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(
            super::history::history(&changed, false),
        )),
    );
    assert_eq!(undone.project, before.project);
    let redone = super::history::complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(
            super::history::history(&undone, true),
        )),
    );
    assert_eq!(redone.project, changed.project);
}
