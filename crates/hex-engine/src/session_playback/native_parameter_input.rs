//! One transient request for the current prepared native target.
use crate::{Invalid, NativeParameterApplication};
use hex_lattice::{NativeTransientInput, NativeTransientOperation};

/// One control-allocated backing for the armed runtime. Callback operations
/// overwrite only Copy fields; they never replace or retire this Box.
#[derive(Default)]
pub(super) struct State {
    pub(super) input: Option<NativeTransientInput>,
    pub(super) processing: Option<crate::native_parameter_input::Processing>,
    pub(super) application: Option<NativeParameterApplication>,
    pub(super) scalar: Option<Box<crate::scalar_automation::StateSet>>,
}
impl State {
    pub(super) fn is_empty(&self) -> bool {
        self.input.is_none()
            && self.processing.is_none()
            && self.application.is_none()
            && self.scalar.is_none()
    }
}

impl crate::SessionPlayback {
    /// Fixed DATA payload of the sole armed-state Box; allocator overhead is
    /// excluded, matching the existing boxed keyboard runtime accounting.
    pub(crate) const fn native_parameter_input_state_bytes() -> usize {
        std::mem::size_of::<State>()
    }

    pub fn native_parameter_input_target(&self) -> Option<crate::NativeParameterInputTarget> {
        self.revision.native_parameter_input_target()
    }
    /// Validate one inline request against the actual armed current runtime.
    /// This stages no receipt: callers must render successfully and then wait
    /// for the audio output owner's delivery confirmation.
    pub fn set_native_parameter_input(
        &mut self,
        operation: NativeTransientOperation,
    ) -> Result<(), Invalid> {
        let target = self
            .revision
            .native_parameter_input_target()
            .filter(|target| target.target() == operation.target())
            .filter(|_| !self.linear_arrangement_export)
            .ok_or(Invalid("native parameter input target is not armed"))?;
        let input = NativeTransientInput {
            identity: target.identity(),
            operation,
        };
        if target.device_class() == crate::NativeDeviceClass::Instrument {
            let program = self
                .revision
                .session()
                .armed_instrument_automation(target)
                .ok_or(Invalid("native instrument input is not prepared"))?;
            if program.input_target() != Some(operation.target())
                || program
                    .block_with_input(None, 0, operation.value())
                    .is_err()
            {
                return Err(Invalid("native instrument input is invalid or unsupported"));
            }
            let state = self
                .native_parameter_state
                .as_deref_mut()
                .ok_or(Invalid("native parameter input state is not prepared"))?;
            state.input = Some(input);
            state.application = None;
            return Ok(());
        }
        let chain = match input.identity.owner() {
            hex_lattice::ProcessorChainOwner::Source(source) => target
                .source_effect_receiver()
                .and_then(|receiver| self.source_effect_receiver_index(receiver))
                .and_then(|index| self.source_effects.chain_for(index, source)),
            hex_lattice::ProcessorChainOwner::Main => Some(&self.main_chain),
            hex_lattice::ProcessorChainOwner::Track(id) => self
                .revision
                .arrangement()
                .tracks()
                .iter()
                .position(|track| {
                    track
                        .canonical_track_id()
                        .is_some_and(|track| track.value() == id)
                })
                .and_then(|index| self.device_chains.get(index)),
            hex_lattice::ProcessorChainOwner::Return(id) => self
                .revision
                .arrangement()
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == id)
                .and_then(|index| self.return_chains.get(index)),
        }
        .ok_or(Invalid("native parameter input owner no longer exists"))?;
        if !chain
            .validate_transient_input(input)
            .map_err(|_| Invalid("native parameter input is invalid or unsupported"))?
        {
            return Err(Invalid("native parameter input target is not current"));
        }
        let state = self
            .native_parameter_state
            .as_deref_mut()
            .ok_or(Invalid("native parameter input state is not prepared"))?;
        state.input = Some(input);
        state.application = None;
        Ok(())
    }

    fn source_effect_receiver_index(&self, receiver: std::num::NonZeroU64) -> Option<usize> {
        self.revision
            .arrangement()
            .tracks()
            .iter()
            .position(|track| {
                track
                    .canonical_track_id()
                    .is_some_and(|id| id.value() == receiver.get())
                    && track.receives_source_audio()
            })
    }

    /// An interrupted source/transport discards the pending input and any
    /// admitted transient output trajectory before the next render.
    pub fn invalidate_native_parameter_input(&mut self) {
        use hex_lattice::ProcessorChainOwner;
        use hex_lattice_contract::DeviceAutomationTarget::HostOutputGain;
        if let Some(target) = self.native_parameter_input_target()
            && target.target() == HostOutputGain
        {
            let identity = target.identity();
            let chain = match identity.owner() {
                ProcessorChainOwner::Source(source) => target
                    .source_effect_receiver()
                    .and_then(|receiver| self.source_effect_receiver_index(receiver))
                    .and_then(|index| self.source_effects.chain_for_mut(index, source)),
                ProcessorChainOwner::Main => Some(&mut self.main_chain),
                ProcessorChainOwner::Track(id) => self
                    .revision
                    .arrangement()
                    .tracks()
                    .iter()
                    .position(|track| track.canonical_track_id().is_some_and(|t| t.value() == id))
                    .and_then(|index| self.device_chains.get_mut(index)),
                ProcessorChainOwner::Return(id) => self
                    .revision
                    .arrangement()
                    .return_buses()
                    .iter()
                    .position(|bus| bus.id.value() == id)
                    .and_then(|index| self.return_chains.get_mut(index)),
            };
            if let Some(chain) = chain {
                chain.invalidate_transient_output_gain(identity);
            }
        }
        self.finish_native_parameter_input();
    }

    /// Retire the request after delivery of Clear. Its already admitted
    /// release continues in the output stage until it reaches the fallback.
    pub fn finish_native_parameter_input(&mut self) {
        if let Some(state) = self.native_parameter_state.as_deref_mut() {
            state.input = None;
            state.processing = None;
            state.application = None;
        }
    }

    pub fn take_native_parameter_application(&mut self) -> Option<NativeParameterApplication> {
        self.native_parameter_state
            .as_deref_mut()?
            .application
            .take()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice::NativeTransientValue;
    use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};

    fn fixture(
        owner: u8,
        target: DeviceAutomationTarget,
        limiter_before: Option<bool>,
    ) -> (SessionPlayback, Editor) {
        fixture_with_state(
            owner,
            target,
            limiter_before,
            UTILITY_DEFINITION.default_state().unwrap(),
        )
    }
    pub(in crate::session_playback) fn fixture_with_state(
        owner: u8,
        target: DeviceAutomationTarget,
        limiter_before: Option<bool>,
        state: hex_lattice_contract::NativeDeviceState,
    ) -> (SessionPlayback, Editor) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "native input".into(),
                path: "memory/native-input.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let limiter = || {
            hex_devices::limiter::DEFINITION
                .patch_parameter(
                    &hex_devices::limiter::DEFINITION.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(2),
                )
                .unwrap()
        };
        if limiter_before == Some(true) {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: limiter(),
                })
                .unwrap();
        }
        let command = match owner {
            0 => Command::InsertDevice {
                track,
                before: None,
                state,
            },
            1 => Command::InsertMainDevice {
                before: None,
                state,
            },
            2 => {
                let Created::ReturnTrack(bus) = editor
                    .execute(Command::AddReturnTrack {
                        name: "input return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return");
                };
                editor
                    .execute(Command::SetReturnSend {
                        track,
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
                Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state,
                }
            }
            _ => panic!("fixture owner"),
        };
        let Created::Device(device) = editor.execute(command).unwrap() else {
            panic!("device");
        };
        if limiter_before == Some(false) {
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: limiter(),
                })
                .unwrap();
        }
        let revision = PreparedRevision::prepare_for_native_parameter_input(
            editor.project(),
            &BTreeMap::from([(
                asset,
                Audio::new(48_000, vec![[0.125, -0.25]; 4096]).unwrap(),
            )]),
            Arc::new(SineTable::new()),
            None,
            device,
            target,
        )
        .unwrap();
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        (runtime, editor)
    }

    #[test]
    fn shared_native_input_float_bool_choice_clear_and_all_owners_render_actual_pcm() {
        for owner in 0..3 {
            for (id, value, expected) in [
                (parameter::GAIN, ParameterValue::Float(2.0), [0.25, -0.5]),
                (parameter::MUTE, ParameterValue::Bool(true), [0.0, 0.0]),
                (
                    parameter::CHANNEL,
                    ParameterValue::Choice(3),
                    [-0.25, 0.125],
                ),
            ] {
                let target = DeviceAutomationTarget::Parameter(id);
                let (mut runtime, editor) = fixture(owner, target, None);
                let state_address =
                    runtime.native_parameter_state.as_deref().unwrap() as *const State;
                let actual = runtime.accounted_runtime_storage_bytes().unwrap();
                assert!(
                    actual
                        <= SessionPlayback::interactive_runtime_allowance(&runtime.revision)
                            .unwrap()
                );
                let operation =
                    NativeTransientOperation::Set(NativeTransientValue::Parameter { id, value });
                runtime.set_native_parameter_input(operation).unwrap();
                let mut output = [[9.0; 2]; 8];
                runtime.render(&mut output).unwrap();
                let dry = if owner == 2 { [0.125, -0.25] } else { [0.0; 2] };
                assert_eq!(output, [[expected[0] + dry[0], expected[1] + dry[1]]; 8]);
                let applied = runtime.take_native_parameter_application().unwrap();
                assert_eq!(
                    (
                        applied.arrangement_frame,
                        applied.performance_frame,
                        applied.loop_pass,
                        applied.remaining_latency_frames
                    ),
                    (0, 0, 0, 0)
                );
                assert_eq!(applied.operation, operation);
                assert!(runtime.take_native_parameter_application().is_none());
                runtime
                    .set_native_parameter_input(NativeTransientOperation::Clear(target))
                    .unwrap();
                runtime.render(&mut output).unwrap();
                assert_eq!(output, [[0.125 + dry[0], -0.25 + dry[1]]; 8]);
                let cleared = runtime.take_native_parameter_application().unwrap();
                assert_eq!(
                    (cleared.arrangement_frame, cleared.performance_frame),
                    (8, 8)
                );
                assert_eq!(cleared.operation, NativeTransientOperation::Clear(target));
                runtime.invalidate_native_parameter_input();
                assert!(
                    runtime
                        .native_parameter_state
                        .as_deref()
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    runtime.native_parameter_state.as_deref().unwrap() as *const State,
                    state_address
                );
                assert_eq!(runtime.accounted_runtime_storage_bytes().unwrap(), actual);
                assert!(
                    editor.project().tracks()[0]
                        .devices()
                        .iter()
                        .chain(editor.project().main_devices())
                        .all(|device| device.automation().is_empty())
                );
            }
        }
    }

    #[test]
    fn saturator_provider_and_both_host_stages_match_independently_prepared_canonical_values() {
        let definition = &hex_devices::saturator::SATURATOR_DEFINITION;
        let drive = hex_devices::saturator::parameter::DRIVE;
        let gain = hex_lattice_contract::HostGain::from_millidb(12_001).unwrap();
        let base = definition.default_state().unwrap();
        let mut host_outputs = Vec::new();
        for (target, value, state) in [
            (
                DeviceAutomationTarget::Parameter(drive),
                NativeTransientValue::Parameter {
                    id: drive,
                    value: ParameterValue::Float(12.0),
                },
                definition
                    .patch_parameter(&base, drive, ParameterValue::Float(12.0))
                    .unwrap(),
            ),
            (
                DeviceAutomationTarget::HostInputGain,
                NativeTransientValue::HostInputGain(gain),
                base.clone()
                    .with_input_gain_millidb(gain.millidb())
                    .unwrap(),
            ),
            (
                DeviceAutomationTarget::HostOutputGain,
                NativeTransientValue::HostOutputGain(gain),
                base.clone()
                    .with_output_gain_millidb(gain.millidb())
                    .unwrap(),
            ),
        ] {
            let (mut transient, _) = fixture_with_state(0, target, None, base.clone());
            let (mut canonical, _) = fixture_with_state(0, target, None, state);
            transient
                .set_native_parameter_input(NativeTransientOperation::Set(value))
                .unwrap();
            let mut actual = [[0.0; 2]; 256];
            let mut expected = actual;
            transient.render(&mut actual).unwrap();
            canonical.render(&mut expected).unwrap();
            let application = transient.take_native_parameter_application().unwrap();
            if target == DeviceAutomationTarget::HostOutputGain {
                assert_ne!(
                    actual[16], expected[16],
                    "output-stage edit ramps before settling"
                );
                let settled = usize::try_from(application.performance_frame).unwrap() + 239;
                assert_eq!(actual[settled..], expected[settled..]);
            } else {
                assert_eq!(actual, expected);
            }
            assert_eq!(application.operation, NativeTransientOperation::Set(value));
            if target.is_host() {
                host_outputs.push(actual);
            }
        }
        assert_ne!(
            host_outputs[0], host_outputs[1],
            "nonlinear input and output gain stages must remain distinct"
        );
    }

    #[test]
    fn native_host_trim_input_and_output_use_shared_receipts_without_provider_lanes() {
        let gain = hex_lattice_contract::HostGain::from_millidb(-6_001).unwrap();
        for owner in 0..3 {
            for (target, value) in [
                (
                    DeviceAutomationTarget::HostInputGain,
                    NativeTransientValue::HostInputGain(gain),
                ),
                (
                    DeviceAutomationTarget::HostOutputGain,
                    NativeTransientValue::HostOutputGain(gain),
                ),
            ] {
                let (mut runtime, _) = fixture(owner, target, None);
                runtime
                    .set_native_parameter_input(NativeTransientOperation::Set(value))
                    .unwrap();
                let mut output = [[9.0; 2]; 4];
                runtime.render(&mut output).unwrap();
                let dry = if owner == 2 { 1.0 } else { 0.0 };
                let smooth = target == DeviceAutomationTarget::HostOutputGain;
                let factor = |from: f32, to: f32, elapsed: usize| {
                    (f64::from(from) + (f64::from(to) - f64::from(from)) * elapsed as f64 / 240.0)
                        as f32
                };
                for (offset, frame) in output.into_iter().enumerate() {
                    let expected = if smooth {
                        factor(1.0, gain.linear(), offset + 1)
                    } else {
                        gain.linear()
                    };
                    assert!((frame[0] - 0.125 * (expected + dry)).abs() < 1e-7);
                    assert!((frame[1] + 0.25 * (expected + dry)).abs() < 1e-7);
                }
                assert_eq!(
                    runtime
                        .take_native_parameter_application()
                        .unwrap()
                        .operation,
                    NativeTransientOperation::Set(value)
                );
                runtime
                    .set_native_parameter_input(NativeTransientOperation::Clear(target))
                    .unwrap();
                runtime.render(&mut output).unwrap();
                for (offset, frame) in output.into_iter().enumerate() {
                    let expected = if smooth {
                        factor(factor(1.0, gain.linear(), 4), 1.0, offset + 1)
                    } else {
                        1.0
                    };
                    assert!((frame[0] - 0.125 * (expected + dry)).abs() < 1e-7);
                    assert!((frame[1] + 0.25 * (expected + dry)).abs() < 1e-7);
                }
                assert_eq!(
                    runtime
                        .take_native_parameter_application()
                        .unwrap()
                        .operation,
                    NativeTransientOperation::Clear(target)
                );
            }
        }
    }

    #[test]
    fn native_input_irregular_blocks_keep_loop_content_and_processing_origins_distinct() {
        let id = parameter::GAIN;
        let target = DeviceAutomationTarget::Parameter(id);
        let (prepared, _) = fixture(0, target, None);
        let mut runtime = SessionPlayback::new_with_policy_and_loop(
            Arc::clone(&prepared.revision),
            false,
            None,
            Some(hex_project::LoopRegion::new(0, 64, true).unwrap()),
        )
        .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let operation = NativeTransientOperation::Set(NativeTransientValue::Parameter {
            id,
            value: ParameterValue::Float(2.0),
        });
        let mut performance = 0;
        for frames in [7, 19, 3, 43, 1] {
            runtime.set_native_parameter_input(operation).unwrap();
            let mut output = vec![[9.0; 2]; frames];
            runtime.render(&mut output).unwrap();
            for (offset, frame) in output.iter().enumerate() {
                let position = (performance + offset as u64) % 64;
                let fade = if position >= 49 {
                    (63 - position) as f32 / 15.0
                } else {
                    1.0
                };
                assert_eq!(*frame, [0.25 * fade, -0.5 * fade]);
            }
            let applied = runtime.take_native_parameter_application().unwrap();
            assert_eq!(
                (
                    applied.performance_frame,
                    applied.arrangement_frame,
                    applied.loop_pass
                ),
                (performance, performance % 64, performance / 64)
            );
            performance += frames as u64;
        }
    }

    #[test]
    fn missing_output_origin_defers_clear_without_releasing_the_applied_trim() {
        use crate::native_parameter_input::{Processing, process_range_with_external};
        use hex_lattice::{NativeInstrumentClock, RefuseForeign};
        use hex_lattice_contract::HostGain;
        struct Origin;
        impl NativeInstrumentClock for Origin {
            fn beat_at_offset(&self, _: usize) -> Option<u64> {
                None
            }
            fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
                u64::try_from(offset).ok()
            }
            fn loop_pass_at_offset(&self, _: usize) -> Option<u64> {
                Some(0)
            }
        }
        let target = DeviceAutomationTarget::HostOutputGain;
        let (mut playback, _) = fixture(1, target, None);
        let identity = playback.native_parameter_input_target().unwrap().identity();
        let chain = &mut playback.main_chain;
        let gain = HostGain::from_db(-6.0).unwrap();
        let render = |chain: &mut hex_lattice::NativeRuntimeChain,
                      operation,
                      clock,
                      pcm: &mut [[f32; 2]]| {
            let mut processing = Processing::new(
                NativeTransientInput {
                    identity,
                    operation,
                },
                0,
                0,
            );
            chain.begin_block();
            process_range_with_external(
                Some(&mut processing),
                None,
                chain,
                0..1,
                pcm,
                0,
                Some(&Origin),
                clock,
                0,
                0,
                &mut RefuseForeign,
                &mut None,
            )
            .unwrap();
            chain.commit_block();
            processing.applied
        };
        let set = NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(gain));
        assert!(render(chain, set, Some(&Origin), &mut [[0.25, -0.5]; 240]).is_some());
        let clear = NativeTransientOperation::Clear(target);
        let held = [0.25 * gain.linear(), -0.5 * gain.linear()];
        let mut pcm = [[0.25, -0.5]; 7];
        assert!(render(chain, clear, None, &mut pcm).is_none());
        assert_eq!(pcm, [held; 7]);
        pcm.fill([0.25, -0.5]);
        assert_eq!(
            render(chain, clear, Some(&Origin), &mut pcm)
                .unwrap()
                .operation,
            clear
        );
        assert!(held[0] < pcm[0][0] && pcm[0][0] < pcm[6][0] && pcm[6][0] < 0.25);
    }

    #[test]
    fn native_input_waits_for_real_upstream_origin_and_reports_actual_processing_counter() {
        let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
        let (mut runtime, _) = fixture(1, target, Some(true));
        runtime
            .set_native_parameter_input(NativeTransientOperation::Set(
                NativeTransientValue::Parameter {
                    id: parameter::GAIN,
                    value: ParameterValue::Float(2.0),
                },
            ))
            .unwrap();
        let mut warmup = [[9.0; 2]; 288];
        runtime.render(&mut warmup).unwrap();
        assert_eq!(warmup, [[0.0; 2]; 288]);
        assert!(runtime.take_native_parameter_application().is_none());
        let mut frame = [[9.0; 2]];
        runtime.render(&mut frame).unwrap();
        assert_eq!(frame, [[0.25, -0.5]]);
        let applied = runtime.take_native_parameter_application().unwrap();
        assert_eq!(
            (
                applied.arrangement_frame,
                applied.performance_frame,
                applied.remaining_latency_frames
            ),
            (0, 288, 0)
        );
    }

    #[test]
    fn native_input_before_main_limiter_reports_remaining_final_output_delay() {
        for owner in [0, 1] {
            let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
            let (mut runtime, _) = fixture(owner, target, Some(false));
            runtime
                .set_native_parameter_input(NativeTransientOperation::Set(
                    NativeTransientValue::Parameter {
                        id: parameter::GAIN,
                        value: ParameterValue::Float(2.0),
                    },
                ))
                .unwrap();
            let mut first = [[9.0; 2]];
            runtime.render(&mut first).unwrap();
            assert_eq!(first, [[0.0; 2]]);
            let applied = runtime.take_native_parameter_application().unwrap();
            assert_eq!(
                (
                    applied.arrangement_frame,
                    applied.performance_frame,
                    applied.remaining_latency_frames
                ),
                (0, 0, 288)
            );
        }
    }
}
