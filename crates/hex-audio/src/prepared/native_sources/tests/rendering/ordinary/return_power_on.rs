//! Provider-side return from a real workerless Off cut to fresh hosted DSP.
use super::*;

const QUANTUM: usize = 256;
const BRIDGE: usize = 2 * QUANTUM;
const SECOND_WET: usize = BRIDGE + QUANTUM;
const PARTITION: usize = 200;
const FRAMES: usize = 131_072;

#[test]
#[ignore = "explicit original SDK receipts and restorable variant 37 required"]
fn running_return_power_on_preserves_the_pass_through_the_fresh_worker_prefix() {
    let _serial = serial();
    let mut f = fixture_with_quantum("processing", 1, QUANTUM as u16);
    let root = f.control.native_candidate_root();
    let original = f.binding;
    let bus = f.editor.project().return_tracks()[0].id();
    let device = f.editor.project().return_tracks()[0].devices()[0].id();
    let state = f.editor.project().device(device).unwrap().state().clone();
    let component = hex_plugin_state::decode(&state).unwrap().component.unwrap();
    assert_eq!(&component[..4], b"HXPS");
    assert_eq!(u32::from_le_bytes(component[4..8].try_into().unwrap()), 0);
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Return(bus.value()), device.value())
            .unwrap();
    let track = f.editor.project().tracks()[0].id();
    let original_clip = f.editor.project().tracks()[0].clips()[0].id();
    f.editor
        .execute(Command::RemoveClip(original_clip))
        .unwrap();
    let Created::Asset(asset) = f
        .editor
        .execute(Command::AddAsset {
            path: "nonconstant-return-on.wav".into(),
            sample_rate: 48_000,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("nonconstant resident bed");
    };
    f.editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: FRAMES as u64,
            source_offset: 0,
            source_length: FRAMES as u64,
            looping: false,
        })
        .unwrap();
    f.media.insert(
        asset,
        hex_engine::Audio::new(48_000, (0..FRAMES).map(input).collect()).unwrap(),
    );
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let on = compile(&f, true);
    assert_eq!(on.main_output_latency_frames(), BRIDGE as u32);
    publish(&mut f.control, on, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let mut completed = 0;
    for _ in 0..6 {
        physical_block::<QUANTUM>(&mut f);
        completed += 1;
        replacement::wait_for_blocks(&root, original, completed);
    }
    let predecessor = f.control.status.recording_source().unwrap();
    f.editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off = compile_off(&f, identity, &state);
    assert_eq!(off.main_output_latency_frames(), 0);
    let receipt = submit(&mut f, off, predecessor);
    for _ in 0..4 {
        physical_block::<QUANTUM>(&mut f);
        if receipt.state() != PublicationReceiptState::Pending {
            break;
        }
        completed += 1;
        replacement::wait_for_blocks(&root, original, completed);
    }
    assert_eq!(receipt.state(), PublicationReceiptState::Installed);
    let off_source = f.control.status.recording_source().unwrap();
    assert_eq!(off_source.audio_revision, receipt.revision());
    assert!(f.control.status.native_effect_source().is_none());
    return_original_worker(&mut f, original);
    assert_off_block(&mut f);

    let fresh = source_for_profile(
        &mut f.control,
        identity,
        "processing",
        QUANTUM as u16,
        Some(state),
    );
    assert_ne!(fresh, original);
    f.control.publish_native_source_endpoint(fresh).unwrap();
    f.binding = fresh;
    assert_off_block(&mut f);
    assert_eq!(
        f.control.native_source_endpoint_installed(fresh),
        Some(true)
    );
    f.editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();

    // The previous On source is the wrong predecessor even with a fresh
    // fence from this output. Refusal leaves the rolling Off PCM intact.
    let request = hex_engine::playback::HostedReturnPowerOn {
        identity,
        binding: fresh,
    };
    let mut candidate = compile(&f, true);
    candidate.require_hosted_return_power_on(request).unwrap();
    let fence = f.control.capture_fresh_runtime_fence().unwrap();
    match f.control.publish_with_admission_and_source_on_control(
        candidate,
        PublicationPolicy::DeviceInsert { predecessor, fence },
        successor_context(off_source),
        |_| Ok::<_, ()>(()),
    ) {
        Err(AdmittedPublishError::Publication(error)) => assert_eq!(
            error.reason,
            "device insert requires the exact installed project source"
        ),
        _ => panic!("stale pre-Off source acquired the next graph"),
    }
    assert_off_block(&mut f);
    assert_eq!(f.control.status.recording_source(), Some(off_source));
    assert_eq!(
        crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
            &root, fresh,
        ),
        Some(0)
    );

    let mut candidate = compile(&f, true);
    candidate.require_hosted_return_power_on(request).unwrap();
    let cancelled = submit(&mut f, candidate, off_source);
    assert_eq!(
        cancelled.cancel(),
        FreshRuntimeCancellationResult::Cancelled
    );
    assert_off_block(&mut f);
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
    assert_eq!(f.control.status.recording_source(), Some(off_source));
    assert_worker_input(&mut f, &root, fresh, 0);
    f.control.reclaim();

    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.25,
        })
        .unwrap();
    let mut candidate = compile(&f, true);
    candidate.require_hosted_return_power_on(request).unwrap();
    let changed_send = submit(&mut f, candidate, off_source);
    assert_off_block(&mut f);
    assert_eq!(
        changed_send.state(),
        PublicationReceiptState::Refused(
            hex_engine::playback::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
        )
    );
    assert_eq!(f.control.status.recording_source(), Some(off_source));
    assert_worker_input(&mut f, &root, fresh, 0);
    f.control.reclaim();
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();

    let mut candidate = compile(&f, true);
    candidate.require_hosted_return_power_on(request).unwrap();
    let on_receipt = submit(&mut f, candidate, off_source);
    let (start, pcm) = render_with_clock::<PARTITION>(&mut f);
    assert_eq!(
        pcm,
        std::array::from_fn(|offset| dry_and_return(start + offset - BRIDGE, 1.0))
    );
    if on_receipt.state() != PublicationReceiptState::Installed {
        let actual = on_receipt.state();
        assert_eq!(f.control.status.recording_source(), Some(off_source));
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success())
        );
        panic!("fresh Return provider refused the original pass: {actual:?}");
    }
    let on_source = f.control.status.recording_source().unwrap();
    assert_eq!(on_source.audio_revision, on_receipt.revision());
    assert_eq!(on_source.output_identity, off_source.output_identity);
    assert_worker_input(&mut f, &root, fresh, PARTITION);
    // The third hardware-sized partition straddles frame 512. Check every
    // sample before and after it, including both initial signed wet replies.
    for partition in 1..5 {
        let (position, pcm) = render_with_clock::<PARTITION>(&mut f);
        assert_eq!(position, start + partition * PARTITION);
        assert_fresh_pcm(start, position, pcm);
        assert_worker_input(&mut f, &root, fresh, (partition + 1) * PARTITION);
    }
    let (position, pcm) = render_with_clock::<{ 4 * QUANTUM - 5 * PARTITION }>(&mut f);
    assert_eq!(position, start + 5 * PARTITION);
    assert_fresh_pcm(start, position, pcm);
    assert_worker_input(&mut f, &root, fresh, 4 * QUANTUM);
    assert_eq!(f.control.status.native_effect_source(), Some(on_source));
    let returned = dispose(f);
    let native::PreparationDisposal::Reaped(retired) = returned.disposal else {
        panic!("return the fresh worker")
    };
    assert!(retired.status.success());
    let terminal = retired.terminal.unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (4, 4 * QUANTUM as u64));
}

fn assert_worker_input(
    f: &mut OrdinaryFixture,
    root: &crate::candidate_custody::CandidateRoot,
    fresh: Binding,
    frames: usize,
) {
    f.data.owner_mut().unwrap().run(|callback, _| {
        assert_eq!(
            callback.native_sources.test_committed_frames(fresh),
            Some(frames as u64)
        );
    });
    let blocks = (frames / QUANTUM) as u64;
    replacement::wait_for_blocks(root, fresh, blocks);
    assert_eq!(
        crate::candidate_custody::native_source_test_fixture::original_worker_published_blocks(
            root, fresh
        ),
        Some(blocks)
    );
}

fn assert_fresh_pcm<const FRAMES: usize>(start: usize, position: usize, pcm: [Frame; FRAMES]) {
    assert_eq!(
        pcm,
        std::array::from_fn(|offset| {
            let frame = position + offset;
            let gain = match frame - start {
                0..BRIDGE => 1.0,
                BRIDGE..SECOND_WET => 0.5,
                _ => 0.625,
            };
            dry_and_return(frame - BRIDGE, gain)
        })
    );
}

fn compile_off(
    f: &OrdinaryFixture,
    identity: ProcessorInstanceIdentity,
    state: &hex_lattice_contract::NativeDeviceState,
) -> Playback {
    let revision = crate::plugin_effects::Hosting::prepare_powered_off_on_control(
        crate::plugin_effects::PoweredOffEffect {
            owner: identity.owner(),
            device_id: identity.device_id(),
            sample_rate: 48_000,
            state,
        },
        &mut |effects| {
            effects.prepare_registered_revision(
                f.editor.project(),
                &f.media,
                Arc::new(hex_engine::SineTable::new()),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &f.resources,
                1_000_000,
                &AtomicBool::new(false),
                None,
            )
        },
    )
    .unwrap();
    Playback::try_from_prepared_with_device_resources(Arc::new(revision), &f.resources).unwrap()
}

fn successor_context(source: AppliedAudioSource) -> hex_engine::AudioSourceContext {
    hex_engine::AudioSourceContext {
        editor_revision: source.context.editor_revision + 1,
        ..source.context
    }
}

fn submit(
    f: &mut OrdinaryFixture,
    playback: Playback,
    predecessor: AppliedAudioSource,
) -> CancellablePublicationReceipt {
    let fence = f.control.capture_fresh_runtime_fence().unwrap();
    let revision = publish_in_context(
        &mut f.control,
        playback,
        PublicationPolicy::DeviceInsert { predecessor, fence },
        successor_context(predecessor),
    );
    f.control.fresh_runtime_receipt(revision).unwrap()
}

fn return_original_worker(f: &mut OrdinaryFixture, binding: Binding) {
    let deadline = Instant::now() + Duration::from_millis(400);
    while !f.control.retire_native_source_preparation(binding) {
        assert!(Instant::now() < deadline, "retire the original Return");
        std::thread::yield_now();
    }
    let returned = loop {
        f.control.reclaim();
        if let Some(returned) = f.control.recover_native_source_preparation(binding) {
            break returned;
        }
        assert_off_block(f);
        assert!(
            Instant::now() < deadline,
            "return the original Return while rolling"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success())
    );
}

fn render_with_clock<const FRAMES: usize>(f: &mut OrdinaryFixture) -> (usize, [Frame; FRAMES]) {
    let timeline = f.control.status.position.load(Acquire);
    let performance = f.control.status.performance_frame.load(Acquire);
    let pcm = physical_block::<FRAMES>(f);
    assert_eq!(
        f.control.status.position.load(Acquire),
        timeline + FRAMES as u64
    );
    assert_eq!(
        f.control.status.performance_frame.load(Acquire),
        performance + FRAMES as u64
    );
    f.data.owner_mut().unwrap().run(|callback, _| {
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| active.playback.main_output_latency_frames()),
            Some(BRIDGE as u32)
        );
    });
    (timeline as usize, pcm)
}

fn assert_off_block(f: &mut OrdinaryFixture) {
    let (start, pcm) = render_with_clock::<QUANTUM>(f);
    assert_eq!(
        pcm,
        std::array::from_fn(|offset| dry_and_return(start + offset - BRIDGE, 1.0))
    );
}

fn input(frame: usize) -> Frame {
    if frame % 17 == 0 {
        // This pair collapses to the same mixed Off sample at a 0.5 send.
        return [f32::from_bits(0x3e2a_aaad), f32::from_bits(0x3e2a_aaae)];
    }
    [
        (frame % 109) as f32 / 1024.0 + 0.03125,
        -((frame % 71) as f32) / 512.0 - 0.0625,
    ]
}

fn dry_and_return(frame: usize, effect_gain: f32) -> Frame {
    input(frame).map(|sample| sample + ((sample * 0.5) * effect_gain))
}
