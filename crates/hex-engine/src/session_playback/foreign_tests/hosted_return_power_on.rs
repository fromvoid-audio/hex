//! A same-pass Return Power On fades and drains only its own alignment ring
//! while unaffected routes stay sample-exact on the existing Main clock.
use super::*;
use crate::{
    Audio,
    session_playback::NativeRemovalProgress,
    signal_graph::{FreshHistory, SignalRuntime},
};
use hex_devices::limiter::{DEFINITION as LIMITER, parameter::LOOKAHEAD};
use hex_lattice::{PreparedProcessorFactory, ProcessorChainOwner};
use hex_lattice_contract::{ForeignProcessorBinding, ParameterValue};

fn vst3_state() -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![19]).unwrap()
}

struct PoweredOffDefinition;
impl NativeDeviceMetadata for PoweredOffDefinition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("hex.vst3")
    }
    fn display_name(&self) -> Option<&'static str> {
        None
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(vst3_state())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}
impl NativeDeviceDefinition for PoweredOffDefinition {
    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_runtime_factory(
        &self,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::PoweredOff))
    }
    fn prepare_instance_runtime_factory(
        &self,
        _: ProcessorInstanceIdentity,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::PoweredOff))
    }
}

struct GainEffect {
    binding: ForeignProcessorBinding,
    gain: f32,
}
impl ForeignProcessors for GainEffect {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        _: usize,
        frames: &mut [Frame],
        _: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(node.prepared.binding(), Some(self.binding));
        for frame in frames {
            frame[0] *= self.gain;
            frame[1] *= self.gain;
        }
        Ok(())
    }

    fn reject_callback(&mut self) {
        panic!("unexpected callback refusal");
    }
}

#[test]
fn fitting_return_power_on_fades_drains_and_keeps_the_native_bed_sample_exact() {
    const RATE: u32 = 48_000;
    const FRAMES: usize = 8192;
    const FADE: usize = 240;
    const MAIN_LATENCY: usize = 576; // two 6 ms native Limiters (288 + 288)
    const BRIDGE: usize = 512;
    const WET_GAIN: f32 = 1.5;

    let mut project = fixture(&[&[], &[]], FRAMES);
    let [send_track, bed_track] = *project.tracks.as_slice() else {
        panic!("two direct audio rows");
    };
    let dry_l = |frame: usize| {
        if frame % 127 < 63 { 0.25 } else { 0.125 }
    };
    let bed_r = |frame: usize| 0.0625 + (frame % 31) as f32 / 1024.0;
    let send_asset = project.editor.project().track(send_track).unwrap().clips()[0].asset();
    let bed_asset = project.editor.project().track(bed_track).unwrap().clips()[0].asset();
    project.media.insert(
        send_asset,
        Audio::new(RATE, (0..FRAMES).map(|f| [dry_l(f), 0.0]).collect()).unwrap(),
    );
    project.media.insert(
        bed_asset,
        Audio::new(RATE, (0..FRAMES).map(|f| [0.0, bed_r(f)]).collect()).unwrap(),
    );

    let limiter_state = LIMITER
        .patch_parameter(
            &LIMITER.default_state().unwrap(),
            LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    for _ in 0..2 {
        project
            .editor
            .execute(Command::InsertDevice {
                track: bed_track,
                before: None,
                state: limiter_state.clone(),
            })
            .unwrap();
    }

    let Created::ReturnTrack(bus) = project
        .editor
        .execute(Command::AddReturnTrack { name: "fx".into() })
        .unwrap()
    else {
        panic!("return bus");
    };
    project
        .editor
        .execute(Command::SetReturnSend {
            track: send_track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let Created::Device(device) = project
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: vst3_state(),
        })
        .unwrap()
    else {
        panic!("hosted return effect");
    };
    project
        .editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();

    let owner = ProcessorChainOwner::Return(bus.value());
    let identity = ProcessorInstanceIdentity::new(owner, device.value()).unwrap();

    let off_defs: [&dyn NativeDeviceDefinition; 2] = [&PoweredOffDefinition, &LIMITER];
    let off_registry = NativeDeviceRegistry::new(&off_defs).unwrap();
    let off_revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            project.editor.project(),
            &project.media,
            Arc::new(SineTable::new()),
            None,
            &off_registry,
        )
        .unwrap(),
    );
    assert_eq!(
        off_revision.arrangement().main_latency_frames(),
        MAIN_LATENCY as u32
    );

    project
        .editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();
    let on_effect = instruments::paired_effect(identity, 7);
    let on_defs: [&dyn NativeDeviceDefinition; 2] = [&on_effect, &LIMITER];
    let on_registry = NativeDeviceRegistry::new(&on_defs).unwrap();
    let on_revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            project.editor.project(),
            &project.media,
            Arc::new(SineTable::new()),
            None,
            &on_registry,
        )
        .unwrap(),
    );
    let on_binding = on_revision.arrangement().return_buses()[0].device_chain[0]
        .foreign_processor()
        .unwrap()
        .binding()
        .unwrap();
    assert_eq!(
        on_revision.arrangement().main_latency_frames(),
        MAIN_LATENCY as u32
    );
    assert_eq!(
        on_revision
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .processor_latency(device.value(), None)
            .unwrap()
            .stage_frames,
        BRIDGE as u32
    );

    // SignalRuntime level proof: requires ChangedReturn, rejects plain copy_from.
    let off_signal = SignalRuntime::new(Arc::clone(
        off_revision.arrangement().signal_graph.as_ref().unwrap(),
    ))
    .unwrap();
    let on_signal = SignalRuntime::new(Arc::clone(
        on_revision.arrangement().signal_graph.as_ref().unwrap(),
    ))
    .unwrap();
    assert!(!on_signal.can_copy_from(&off_signal, None));
    assert!(on_signal.can_copy_from(
        &off_signal,
        Some(FreshHistory::ChangedReturn {
            bus,
            device: device.value(),
            cut_predecessor_pcm: false,
        }),
    ));

    // Contrast: cancelling mid-fade restores Off output cleanly; completing fade + drain installs.
    for cancel in [true, false] {
        let mut old = SessionPlayback::new(Arc::clone(&off_revision)).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        let mut warmup = vec![[0.0; 2]; 1024];
        old.render(&mut warmup).unwrap();
        for (offset, frame) in warmup[MAIN_LATENCY..].iter().enumerate() {
            let at = MAIN_LATENCY + offset;
            assert!((frame[0] - 1.5 * dry_l(at - MAIN_LATENCY)).abs() < 1e-6);
            assert!((frame[1] - bed_r(at - MAIN_LATENCY)).abs() < 1e-5);
        }

        let mut candidate = SessionPlayback::new(Arc::clone(&on_revision)).unwrap();
        candidate.require_continuous_insert = true;
        assert_eq!(
            candidate.advance_native_removal_from(&mut old).unwrap(),
            NativeRemovalProgress::Fading
        );
        // Refuses early handoff before fade + drain finishes.
        assert_eq!(
            candidate.inherit_state(&mut old),
            Err(SessionPlaybackError::DeviceTransitionBusy)
        );

        let mut cursor = 1024_usize;
        if cancel {
            let mut mid = [[0.0; 2]; 128];
            old.render(&mut mid).unwrap();
            for (offset, frame) in mid.iter().enumerate() {
                assert!((frame[1] - bed_r(cursor + offset - MAIN_LATENCY)).abs() < 1e-5);
            }
            cursor += 128;
            candidate.cancel_native_removal_from(&mut old);
            // After cancel ramps back up + drains 576 frames, Off Return is back at full 1.5x.
            let mut recovered = vec![[0.0; 2]; FADE + MAIN_LATENCY + 256];
            old.render(&mut recovered).unwrap();
            for (offset, frame) in recovered.iter().enumerate() {
                assert!((frame[1] - bed_r(cursor + offset - MAIN_LATENCY)).abs() < 1e-5);
            }
            for (offset, frame) in recovered[FADE + MAIN_LATENCY..].iter().enumerate() {
                let at = cursor + FADE + MAIN_LATENCY + offset;
                assert!((frame[0] - 1.5 * dry_l(at - MAIN_LATENCY)).abs() < 1e-6);
            }
            continue;
        }

        // Advance until fade (240) + Return alignment ring drain (576) completes.
        let mut ready = false;
        for _ in 0..8 {
            let mut block = [[0.0; 2]; 128];
            old.render(&mut block).unwrap();
            for (offset, frame) in block.iter().enumerate() {
                assert!(
                    (frame[1] - bed_r(cursor + offset - MAIN_LATENCY)).abs() < 1e-5,
                    "bed drifted during fade/drain at frame {}",
                    cursor + offset
                );
            }
            cursor += 128;
            if candidate.advance_native_removal_from(&mut old).unwrap()
                == NativeRemovalProgress::Ready
            {
                ready = true;
                break;
            }
        }
        assert!(ready, "fitting Return Power On did not reach Ready");

        candidate
            .inherit_state(&mut old)
            .expect("fitting Return Power On installs without interruption consent");

        let mut foreign = GainEffect {
            binding: on_binding,
            gain: WET_GAIN,
        };
        let mut post = vec![[0.0; 2]; 512];
        candidate
            .render_with_foreign_processors(&mut post, None, None, None, &mut foreign)
            .unwrap();
        let return_delay = MAIN_LATENCY - BRIDGE; // 64 frames
        for (offset, frame) in post.iter().enumerate() {
            let at = cursor + offset;
            // Independent bed stays sample-exact across the entire cut and post-cut.
            assert!(
                (frame[1] - bed_r(at - MAIN_LATENCY)).abs() < 1e-5,
                "bed drifted after cut at frame {at}: {} vs {}",
                frame[1],
                bed_r(at - MAIN_LATENCY)
            );
            // Zero leaked Off PCM: before the new 64-frame Return output delay fills,
            // left channel is exact dry; once filled, wet arrives from fresh GainEffect.
            let expected_l = if offset < return_delay {
                dry_l(at - MAIN_LATENCY)
            } else {
                dry_l(at - MAIN_LATENCY) + 0.5 * WET_GAIN * dry_l(at - return_delay)
            };
            assert!(
                (frame[0] - expected_l).abs() < 1e-5,
                "left channel mismatch at offset {offset} (frame {at}): {} vs {expected_l}",
                frame[0]
            );
        }
    }
}

#[test]
fn incompatible_zero_latency_return_power_on_still_requires_consent() {
    const FRAMES: usize = 2048;

    let mut project = fixture(&[&[]], FRAMES);
    let send_track = project.tracks[0];
    let Created::ReturnTrack(bus) = project
        .editor
        .execute(Command::AddReturnTrack { name: "fx".into() })
        .unwrap()
    else {
        panic!("return bus");
    };
    project
        .editor
        .execute(Command::SetReturnSend {
            track: send_track,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let Created::Device(device) = project
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: vst3_state(),
        })
        .unwrap()
    else {
        panic!("hosted return effect");
    };
    project
        .editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let owner = ProcessorChainOwner::Return(bus.value());
    let identity = ProcessorInstanceIdentity::new(owner, device.value()).unwrap();
    let off_defs: [&dyn NativeDeviceDefinition; 1] = [&PoweredOffDefinition];
    let off_registry = NativeDeviceRegistry::new(&off_defs).unwrap();
    let off_revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            project.editor.project(),
            &project.media,
            Arc::new(SineTable::new()),
            None,
            &off_registry,
        )
        .unwrap(),
    );
    project
        .editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: false,
        })
        .unwrap();
    let on_effect = instruments::paired_effect(identity, 9);
    let on_defs: [&dyn NativeDeviceDefinition; 1] = [&on_effect];
    let on_registry = NativeDeviceRegistry::new(&on_defs).unwrap();
    let on_revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            project.editor.project(),
            &project.media,
            Arc::new(SineTable::new()),
            None,
            &on_registry,
        )
        .unwrap(),
    );

    let mut old = SessionPlayback::new(off_revision).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 256]).unwrap();
    let mut candidate = SessionPlayback::new(on_revision).unwrap();
    candidate.require_continuous_insert = true;
    assert_eq!(
        candidate.advance_native_removal_from(&mut old).unwrap(),
        NativeRemovalProgress::NotApplicable
    );
    assert_eq!(
        candidate.inherit_state(&mut old),
        Err(SessionPlaybackError::DeviceInsertNeedsConfirmation)
    );
}
