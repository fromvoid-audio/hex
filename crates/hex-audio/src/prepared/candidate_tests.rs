use super::*;
use crate::native_callback_owner::{
    ErrorCallbackOwner, Guard, NativeCallbackOwner, NativeRetirement, reserve_native,
};
use hex_engine::{
    AudioArrangementInput, AudioCandidateOwner, AudioPreparationCustody, AudioStorageReservation,
    OwnedAudioInputs, OwnedAudioSource,
};
use hex_project::{Command, Created, Editor};
use std::time::Duration;

// Match the real native exclusive-output lease while these headless fixtures
// share the actual singleton lifecycle worker.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(crate) fn finish(retirement: &mut NativeRetirement) {
    let root = retirement.root();
    root.announce_retirement();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !retirement.advance().unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "lifecycle disposal did not finish"
        );
        std::thread::yield_now();
    }
    // These healthy headless fixtures manually drive terminal retirement without
    // a CPAL StreamOwner/Retire message. Wait for the real singleton executor's
    // idle observation before allowing another fixture's independent root. The
    // production path obtains this barrier by FIFO Retire adoption under Lease.
    while root.cleanup_scheduled() {
        assert!(
            std::time::Instant::now() < deadline,
            "executor retained old fixture job"
        );
        std::thread::yield_now();
    }
}

pub(crate) fn recover(
    control: &mut Control,
    ticket: &crate::AudioCandidateTicket,
) -> crate::AudioCandidateRecovery {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(ready) = control.recover_audio_candidate(ticket) {
            return ready;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lifecycle recovery did not finish: {}",
            control.native_candidate_root().test_status()
        );
        std::thread::yield_now();
    }
}
pub(crate) fn candidate() -> Option<AudioCandidateOwner> {
    candidate_with_session(false).map(|(owner, _)| owner)
}
pub(super) fn candidate_with_session(
    with_session: bool,
) -> Option<(AudioCandidateOwner, Option<hex_project::ClipId>)> {
    candidate_with_context(with_session, None)
}
pub(crate) fn candidate_with_context(
    with_session: bool,
    context: Option<hex_engine::AudioSourceContext>,
) -> Option<(AudioCandidateOwner, Option<hex_project::ClipId>)> {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return None;
    }
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "audio".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    for start in [0, 8] {
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
            })
            .unwrap();
    }
    let session_clip = if with_session {
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "audio scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "audio clip".into(),
                asset,
                source_start: 2,
                source_end: 6,
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        Some(clip)
    } else {
        None
    };
    let frames = if with_session {
        (0..8)
            .map(|index| [0.25 + 0.01 * index as f32; 2])
            .collect()
    } else {
        vec![[0.25; 2]; 8]
    };
    let source = OwnedAudioSource::try_resident(48_000, frames).ok().unwrap();
    let bank = OwnedAudioInputs::try_new(
        vec![(asset, source)],
        AudioStorageReservation {
            allocation_items: 64,
            handle_obligations: 256,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let input =
        AudioArrangementInput::new(editor.project(), 11, bank).with_audio_prepared_revision();
    let input = if let Some(context) = context {
        input.with_source_context(context)
    } else {
        input
    };
    let token = custody.begin_input_on_control(input).ok().unwrap();
    Some((
        match AudioCandidateOwner::try_new_on_control(custody, token) {
            Ok(owner) => owner,
            Err(refusal) => {
                let reason = format!("{:?}", refusal.reason);
                drop(refusal);
                panic!("actual candidate constructor refused: {reason}");
            }
        },
        session_clip,
    ))
}
pub(crate) fn anchored() -> (
    Control,
    Guard<NativeCallbackOwner>,
    Guard<ErrorCallbackOwner>,
    NativeRetirement,
) {
    let (mut control, callback) = channel(48_000).unwrap();
    let (data, errors, terminals) = reserve_native(callback, Arc::clone(&control.status));
    let retirement = NativeRetirement::new(terminals, control.native_candidate_root());
    control.enable_native_custody().unwrap();
    (control, data, errors, retirement)
}
fn legacy() -> Playback {
    Playback::new(hex_engine::Project::new(48_000, vec![]).unwrap())
}
pub(crate) fn pair() -> crate::CandidateDomain {
    let mut runtime =
        hex_stream::StreamRuntime::new(hex_stream::StreamConfig::with_pool_pages(4), vec![])
            .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    runtime.split().ok().unwrap()
}
#[test]
fn real_candidate_queue_rejects_before_render_preserves_applied_revision_and_recovers_original_bank()
 {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let identity = owner.identity();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control.publish(legacy()).unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        let mut output = [[1.0; 2]; 8];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 8]); // candidate's real 0.25 PCM was never installed
    });
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let recovery = recover(&mut control, &ticket);
    assert_eq!(recovery.disposed.identity(), identity);
    assert!(recovery.disposed.closed_owner_groups() > 0);
    assert!(recovery.inputs.owning_handle_count() > 0);
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    assert!(
        control
            .audio_candidate_custody()
            .runtime_and_shared_overhead_unknown
    );
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn naked_channel_and_full_queue_refuse_original_owner_then_control_first_drop_preserves_queued_custody()
 {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let identity = owner.identity();
    let (mut naked, callback) = channel(48_000).unwrap();
    let refused = naked
        .stage_audio_candidate(owner, Vec::with_capacity(3))
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(refused.domains.capacity(), 3);
    drop(naked);
    drop(callback);
    let (mut control, data, errors, mut retirement) = anchored();
    let root = control.native_candidate_root();
    let _abandoned_ticket = control
        .stage_audio_candidate(refused.owner, vec![])
        .ok()
        .unwrap();
    let second = candidate().unwrap();
    let second_id = second.identity();
    let refused = control
        .stage_audio_candidate(second, Vec::with_capacity(4))
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), second_id);
    assert_eq!(refused.domains.capacity(), 4);
    assert_eq!(root.report().occupied_slots, 1);
    drop(control);
    assert!(!retirement.advance().unwrap());
    std::thread::spawn(move || drop(data)).join().unwrap();
    assert!(!retirement.advance().unwrap()); // error closure still owns its guard
    assert_eq!(root.report().occupied_slots, 1);
    std::thread::spawn(move || drop(errors)).join().unwrap();
    finish(&mut retirement);
    assert_eq!(root.report().occupied_slots, 0);
}
#[test]
fn full_return_queue_holds_actual_candidate_and_shutdown_ack_is_not_disposal() {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    data.owner_mut().unwrap().run(|callback, _| {
        for revision in 1..=RETIRED as u64 {
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    playback: Box::new(legacy()),
                }))
                .ok()
                .unwrap();
        }
    });
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(data.owner_mut().unwrap().callback.held_retirement.is_some());
    control.request_shutdown();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(!control.status.shutdown_complete.load(Acquire));
    control.reclaim(); // only legacy returns; no candidate is available yet
    assert!(control.recover_audio_candidate(&ticket).is_none());
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(control.status.shutdown_complete.load(Acquire));
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1); // no disposal inference from ACK
    drop(recover(&mut control, &ticket));
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn actual_catalog_lease_busy_keeps_slot_and_original_pair_until_normal_cleanup() {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let domain = pair();
    let extra = domain.0.catalog_lease().unwrap();
    let ticket = control
        .stage_audio_candidate(owner, vec![domain])
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    for _ in 0..3 {
        assert!(control.recover_audio_candidate(&ticket).is_none());
    }
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    drop(extra);
    drop(recover(&mut control, &ticket));
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn actual_domain_pressure_and_mismatched_pairs_refuse_without_replacing_inputs() {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let first = pair();
    let lease = first.0.catalog_lease().unwrap();
    let ticket = control
        .stage_audio_candidate(owner, vec![first, pair()])
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    let next = candidate().unwrap();
    let identity = next.identity();
    let domains = vec![pair()];
    let pointer = domains.as_ptr();
    let refusal = control.stage_audio_candidate(next, domains).err().unwrap();
    assert_eq!(refusal.reason, "candidate domain custody is full");
    assert_eq!(refusal.owner.identity(), identity);
    assert_eq!(refusal.domains.as_ptr(), pointer);
    // Explicitly reclaim refused, still-original control-side domains.
    for (control, pages) in refusal.domains {
        control.shutdown(pages).ok().unwrap();
    }
    let a = pair();
    let b = pair();
    let mismatch = vec![(a.0, b.1), (b.0, a.1)];
    let pointer = mismatch.as_ptr();
    let mut refusal = control
        .stage_audio_candidate(refusal.owner, mismatch)
        .err()
        .unwrap();
    assert_eq!(refusal.reason, "candidate catalog custody differs");
    assert_eq!(refusal.domains.as_ptr(), pointer);
    let (b_control, a_pages) = refusal.domains.pop().unwrap();
    let (a_control, b_pages) = refusal.domains.pop().unwrap();
    a_control.shutdown(a_pages).ok().unwrap();
    b_control.shutdown(b_pages).ok().unwrap();
    drop(lease);
    for _ in 0..8 {
        control.reclaim();
    }
    drop(recover(&mut control, &ticket));
    drop(refusal.owner); // original unaccepted owner, control-side teardown
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn candidate_rejection_held_return_and_guard_release_have_zero_total_and_net_callback_allocations()
{
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    data.owner_mut().unwrap().run(|callback, _| {
        for revision in 0..RETIRED as u64 {
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    playback: Box::new(legacy()),
                }))
                .ok()
                .unwrap();
        }
    });
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
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
    assert!(data.owner_mut().unwrap().callback.held_retirement.is_some());
    control.reclaim(); // drain legacy outside measurement
    assert!(control.recover_audio_candidate(&ticket).is_none());
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
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
    let measured = allocation_counter::measure(|| {
        drop(data);
        drop(errors);
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
    drop(recover(&mut control, &ticket));
    drop(control);
    finish(&mut retirement);
}

pub(crate) fn wait_for_abandoned_cleanup(
    root: &crate::candidate_custody::CandidateRoot,
    deadline: std::time::Instant,
) -> std::thread::ThreadId {
    loop {
        // Vacancy and the nonblocking thread witness use separate locks. Capture
        // the witness here; a later try_lock can contend even after vacancy.
        if let (0, Some(thread)) = (root.report().occupied_slots, root.cleanup_thread()) {
            return thread;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "abandoned cleanup required UI polling"
        );
        std::thread::yield_now();
    }
}

#[test]
fn live_control_abandonment_finishes_busy_multiphase_cleanup_without_another_ui_poll() {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let root = control.native_candidate_root();
    let domain = pair();
    let extra = domain.0.catalog_lease().unwrap();
    let ticket = control
        .stage_audio_candidate(owner, vec![domain])
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    control.reclaim();
    drop(control); // actual return receiver stays anchored; job remains autonomous
    assert_eq!(root.report().occupied_slots, 1);
    assert!(!retirement.advance().unwrap()); // both backend guards still exist
    drop(extra);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let cleanup_thread = wait_for_abandoned_cleanup(&root, deadline);
    assert_ne!(cleanup_thread, std::thread::current().id());
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn candidate_rejection_consumes_priority_stop_under_full_command_pressure_in_the_same_block() {
    let _serial = serial();
    let Some(owner) = candidate() else {
        return;
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    let source = hex_engine::Audio::new(48_000, vec![[0.5; 2]; 16]).unwrap();
    let clip = hex_engine::Clip::new(source, 0, 128, true).unwrap();
    let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
    control
        .publish(Playback::new(
            hex_engine::Project::new(48_000, vec![track]).unwrap(),
        ))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 8];
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(output, [[0.5; 2]; 8]);
    assert!(control.status.playing.load(Acquire));
    let mut last = 0;
    for _ in 0..COMMANDS {
        last = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    let ticket = control.stage_audio_candidate(owner, vec![]).ok().unwrap();
    control.stop();
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    #[cfg(not(feature = "allocation-probe"))]
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(
        !control.status.playing.load(Acquire),
        "candidate rejection starved priority Stop"
    );
    assert_eq!(control.status.consumed_command.load(Acquire), last);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.render(&mut output).unwrap());
    assert_eq!(output, [[0.0; 2]; 8]);
    drop(recover(&mut control, &ticket));
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn constructor_domain_opt_in_requires_vacancy_and_preserves_legacy_refusal_owner() {
    let _serial = serial();
    let reservation = crate::AudioCandidateDomainReservation {
        max_retained_data_bytes: 4 * 1024 * 1024,
        max_construction_peak_data_bytes: 4 * 1024 * 1024,
    };
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_domains(reservation)
        .unwrap();
    let original_root = Arc::new(hex_engine::Project::new(48_000, vec![]).unwrap());
    let original = Playback::from_shared(Arc::clone(&original_root));
    let refusal = control.publish(original).err().unwrap();
    assert_eq!(Arc::strong_count(&original_root), 2);
    assert_eq!(control.revision, 0);
    assert!(!control.status.publication_pending.load(Acquire));
    assert!(
        control
            .configure_audio_candidate_domains(reservation)
            .is_err()
    );
    drop(refusal);
    assert_eq!(Arc::strong_count(&original_root), 1);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);

    // Default legacy behavior is unchanged, including its real applied ACK.
    let (mut control, mut data, errors, mut retirement) = anchored();
    assert_eq!(control.publish(legacy()).unwrap(), 1);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(
        control
            .configure_audio_candidate_domains(reservation)
            .is_err()
    );
    assert_eq!(control.revision, 1);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
