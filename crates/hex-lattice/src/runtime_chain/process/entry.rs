use super::*;

impl NativeRuntimeChain {
    pub(super) fn process_entries_tapped(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(
            ProcessorChainOwner,
            u64,
            DeviceTypeId,
            NodeSignalPhase,
            usize,
            &[StereoFrame],
        ),
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                NodeSignalPhase::Input,
                frame_offset,
                frames,
            );
            Self::process_entry(entry, frames, clock, output_clock)?;
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                NodeSignalPhase::Output,
                frame_offset,
                frames,
            );
        }
        Ok(())
    }

    pub(super) fn process_entries(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            Self::process_entry(entry, frames, clock, output_clock)?;
        }
        Ok(())
    }

    pub(super) fn process_entries_observed(
        entries: &mut [RuntimeEntry],
        frames: &mut [StereoFrame],
        frame_offset: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        observer: &mut impl FnMut(ProcessorChainOwner, u64, DeviceTypeId, usize, &[StereoFrame]),
    ) -> Result<(), DeviceContractError> {
        for entry in entries {
            Self::process_entry(entry, frames, clock, output_clock)?;
            observer(
                entry.owner,
                entry.device_id,
                entry.device_type,
                frame_offset,
                frames,
            );
        }
        Ok(())
    }

    fn process_entry(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Self::process_entry_with_foreign(
            entry,
            frames,
            EntryProcessContext {
                key: None,
                frame_offset: 0,
                clock,
                output_clock,
                foreign: &mut RefuseForeign,
                input: None,
                external: None,
            },
        )
    }

    pub(super) fn process_entry_with_foreign(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        context: EntryProcessContext<'_>,
    ) -> Result<(), DeviceContractError> {
        let EntryProcessContext {
            key,
            frame_offset,
            clock,
            output_clock,
            foreign,
            input,
            external,
        } = context;
        let aligned = entry.scratch_aligned_dry.is_some();
        let aligned_bypass =
            aligned && entry.bypassed && (!entry.supports_power_off || entry.removal_bypassing);
        if !entry.scratch_bypass_ramp.is_active() && !aligned_bypass {
            if !entry.powered_off(entry.scratch_bypass_ramp)
                && let Some(history) = &mut entry.scratch_aligned_dry
            {
                history.advance_discard(frames);
            }
            return Self::process_entry_with_foreign_inner(
                entry,
                frames,
                EntryProcessContext {
                    key,
                    frame_offset,
                    clock,
                    output_clock,
                    foreign,
                    input,
                    external,
                },
            );
        }
        let mut offset = 0;
        while (aligned_bypass || entry.scratch_bypass_ramp.is_active()) && offset < frames.len() {
            const CHUNK: usize = 64;
            let mut count = (frames.len() - offset).min(CHUNK);
            if !aligned || entry.supports_power_off && !aligned_bypass {
                count = count.min(entry.scratch_bypass_ramp.remaining());
            }
            let end = offset + count;
            let mut dry = [[0.0; 2]; CHUNK];
            dry[..count].copy_from_slice(&frames[offset..end]);
            if let Some(history) = &mut entry.scratch_aligned_dry {
                for frame in &mut dry[..count] {
                    *frame = history.advance(*frame);
                }
            }
            let shifted_clock = clock.map(|clock| OffsetClock { clock, offset });
            let shifted_output_clock = output_clock.map(|clock| OffsetClock { clock, offset });
            let shifted_frame_offset = frame_offset
                .checked_add(offset)
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            let bypassed = entry.bypassed;
            let aligned_metering = aligned && entry.metering;
            if aligned_metering {
                entry.meter.observe_input(&frames[offset..end]);
                entry.metering = false;
            }
            entry.bypassed = false;
            let result = Self::process_entry_with_foreign_inner(
                entry,
                &mut frames[offset..end],
                EntryProcessContext {
                    key: key.map(|key| &key[offset..end]),
                    frame_offset: shifted_frame_offset,
                    clock: shifted_clock
                        .as_ref()
                        .map(|c| c as &dyn NativeInstrumentClock),
                    output_clock: shifted_output_clock
                        .as_ref()
                        .map(|c| c as &dyn NativeInstrumentClock),
                    foreign,
                    input,
                    external,
                },
            );
            entry.bypassed = bypassed;
            entry.metering |= aligned_metering;
            result?;
            for (wet, dry) in frames[offset..end].iter_mut().zip(&dry[..count]) {
                let mix = entry.scratch_bypass_ramp.next(bypassed);
                if aligned && mix == 0.0 {
                    *wet = *dry;
                } else if !aligned || mix != 1.0 {
                    for channel in 0..2 {
                        wet[channel] = dry[channel] + (wet[channel] - dry[channel]) * mix;
                    }
                }
            }
            if aligned_metering {
                entry.meter.observe_output(&frames[offset..end]);
            }
            offset = end;
        }
        if offset == frames.len() {
            return Ok(());
        }
        if !entry.powered_off(entry.scratch_bypass_ramp)
            && let Some(history) = &mut entry.scratch_aligned_dry
        {
            history.advance_discard(&frames[offset..]);
        }
        let shifted_clock = clock.map(|clock| OffsetClock { clock, offset });
        let shifted_output_clock = output_clock.map(|clock| OffsetClock { clock, offset });
        Self::process_entry_with_foreign_inner(
            entry,
            &mut frames[offset..],
            EntryProcessContext {
                key: key.map(|key| &key[offset..]),
                frame_offset: frame_offset
                    .checked_add(offset)
                    .ok_or(DeviceContractError::InvalidDescriptor)?,
                clock: shifted_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                output_clock: shifted_output_clock
                    .as_ref()
                    .map(|c| c as &dyn NativeInstrumentClock),
                foreign,
                input,
                external,
            },
        )
    }

    pub(crate) fn process_entry_with_foreign_inner(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        context: EntryProcessContext<'_>,
    ) -> Result<(), DeviceContractError> {
        let EntryProcessContext {
            key,
            frame_offset,
            clock,
            output_clock,
            foreign,
            input,
            external,
        } = context;
        let external = external.filter(|window| {
            !entry.automation_frozen
                && window.identity.owner() == entry.owner
                && window.identity.device_id() == entry.device_id
        });
        let segment = if let Some(window) = external {
            let start = frame_offset
                .checked_sub(window.frame_offset)
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            let end = start
                .checked_add(frames.len())
                .ok_or(DeviceContractError::InvalidDescriptor)?;
            if end > window.values.frames() {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            Some(ExternalSegment {
                values: window.values,
                start,
                frames: frames.len(),
            })
        } else {
            None
        };
        let external = segment
            .as_ref()
            .map(|segment| segment as &dyn ExternalParameterAutomation);
        let is_foreign = matches!(entry.runtime, RuntimeOwnership::Foreign(_));
        if key.is_some()
            && ((!is_foreign && !entry.runtime.is_stateful()) || !entry.matrix.is_identity())
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        if entry.powered_off(entry.scratch_bypass_ramp) {
            if entry.metering {
                entry.meter.observe_input(frames);
            }
            if let Some(history) = &mut entry.scratch_aligned_dry {
                for frame in frames.iter_mut() {
                    *frame = history.advance(*frame);
                }
            }
            if entry.metering {
                entry.meter.observe_output(frames);
            }
            return Ok(());
        }
        let modulates_input = entry.modulates_host_input();
        if !entry.bypassed && !is_foreign && !modulates_input {
            Self::apply_host_gain(entry, frames, clock, true, input, external)?;
        }
        if entry.metering && !modulates_input {
            if is_foreign {
                entry
                    .meter
                    .observe_input_with_gain(frames, entry.input_gain);
            } else {
                entry.meter.observe_input(frames);
            }
        }
        let mut foreign_applies_output_gain = false;
        if let RuntimeOwnership::Foreign(runtime) = &mut entry.runtime {
            let configuration = entry
                .configuration
                .as_deref()
                .ok_or(DeviceContractError::ForeignProcessingUnavailable)?;
            let node = ForeignProcessorNode {
                owner: entry.owner,
                device_id: entry.device_id,
                device_type: entry.device_type,
                sample_rate: entry.sample_rate,
                configuration,
                prepared: runtime.prepared,
                automation: entry
                    .automation
                    .foreign()
                    .map(|program| {
                        program.borrow_for_effect_clock(if entry.automation_frozen {
                            None
                        } else {
                            clock.and_then(|clock| clock.authored_clock_epoch_at_offset(0))
                        })
                    })
                    .transpose()?,
                bypassed: entry.bypassed,
                input_gain: entry.input_gain,
                output_gain: if entry.automation_frozen {
                    entry.committed_host_gains[1]
                } else {
                    entry.output_gain
                },
            };
            foreign_applies_output_gain = foreign.returns_post_trim(node);
            // A failed stage supplies no new observation. Invalidate scratch
            // first so an earlier quiet state cannot mask renewed input.
            runtime.scratch_drained = false;
            // A detached tail retains its old program key while finishing any
            // partial PCM quantum. Its new input has no authored origin.
            let clock = if entry.automation_frozen && node.automation.is_some() {
                None
            } else {
                clock
            };
            if let Some(key) = key {
                foreign.process_segment_with_sidechain(
                    node,
                    frame_offset,
                    frames,
                    key,
                    &[],
                    clock,
                )?;
            } else if let Some(modulation) = &mut runtime.modulation {
                modulation.process_effect_segment(
                    node,
                    frame_offset,
                    frames,
                    clock,
                    foreign,
                    entry.automation_frozen,
                )?;
            } else {
                foreign.process_segment(node, frame_offset, frames, clock)?;
            }
            runtime.scratch_drained =
                matches!(runtime.prepared.output_tail(), OutputTail::Finite { .. })
                    && foreign.committed_tail_is_drained(node);
        } else if let Some(key) = key {
            if !entry.bypassed && !modulates_input {
                entry.scratch_matrix_ramp.apply_block(entry.matrix, frames);
            }
            Self::process_native_provider(
                entry,
                frames,
                Some(key),
                clock,
                output_clock,
                input,
                external,
            )?;
        } else if entry.bypassed {
            if entry.runtime.is_stateful() || entry.modulation.is_some() {
                if entry.matrix.is_identity() {
                    Self::process_native_provider(
                        entry,
                        frames,
                        None,
                        clock,
                        output_clock,
                        input,
                        external,
                    )?;
                } else {
                    for (frame_index, frame) in frames.iter_mut().enumerate() {
                        let audible = *frame;
                        let mut internal = entry.matrix.apply(audible);
                        // Preserve the existing per-frame matrix/bypass path.
                        // A shifted view keeps clock offset zero meaningful.
                        let shifted = clock.map(|clock| OffsetClock {
                            clock,
                            offset: frame_index,
                        });
                        let shifted = shifted
                            .as_ref()
                            .map(|clock| clock as &dyn NativeInstrumentClock);
                        if entry.modulation.is_some() {
                            // A nonidentity matrix can coexist only with an
                            // unassigned bank. Advance it while the processor
                            // sees the same private input it would without it.
                            Self::process_native_provider(
                                entry,
                                std::slice::from_mut(&mut internal),
                                None,
                                shifted,
                                None,
                                None,
                                None,
                            )?;
                        } else {
                            entry.runtime.process_bypassed_with_clock(
                                std::slice::from_mut(&mut internal),
                                shifted,
                            )?;
                        }
                        *frame = audible;
                    }
                }
            }
        } else {
            if !modulates_input {
                entry.scratch_matrix_ramp.apply_block(entry.matrix, frames);
            }
            Self::process_native_provider(
                entry,
                frames,
                None,
                clock,
                output_clock,
                input,
                external,
            )?;
        }
        if !entry.bypassed && !foreign_applies_output_gain && !entry.modulates_host_output() {
            Self::apply_host_gain(entry, frames, output_clock, false, input, external)?;
        } else if !entry.bypassed && foreign_applies_output_gain && !frames.is_empty() {
            entry.scratch_host_gains[1] = if entry.automation_frozen {
                entry.committed_host_gains[1]
            } else {
                entry.output_gain
            };
        }
        if entry.metering {
            entry.meter.observe_output(frames);
        }
        Ok(())
    }

    fn process_native_provider(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        key: Option<&[StereoFrame]>,
        clock: Option<&dyn NativeInstrumentClock>,
        output_clock: Option<&dyn NativeInstrumentClock>,
        input: Option<NativeTransientValue>,
        external: Option<&dyn ExternalParameterAutomation>,
    ) -> Result<(), DeviceContractError> {
        let modulates_input = entry.modulates_host_input();
        let values = (modulates_input
            || (!entry.automation.parameter_ids().is_empty() && !entry.automation_frozen))
            .then(|| {
                entry
                    .automation
                    .block_with_optional_external(clock, frames.len(), input, external)
            })
            .transpose()?;
        let input_stage = values.as_ref().filter(|_| modulates_input);
        let upstream = values
            .as_ref()
            .map(|value| value as &dyn NativeParameterAutomation);
        use host_output_gain_ramp::TransientGain;
        let host_values = entry
            .modulates_host_output()
            .then(|| {
                entry.automation.block_with_optional_external(
                    output_clock,
                    frames.len(),
                    input,
                    external,
                )
            })
            .transpose()?;
        let transient = entry.scratch_transient_output_gain;
        let host_target = hex_lattice_contract::DeviceAutomationTarget::HostOutputGain;
        if host_values.is_some()
            && !frames.is_empty()
            && transient == TransientGain::Follow
            && (entry.automation.has_host_target(host_target)
                || input.is_some_and(|input| input.target() == host_target))
        {
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
        }
        let runtime = &mut entry.runtime;
        if let Some(modulation) = entry
            .modulation
            .as_mut()
            .filter(|_| !entry.automation_frozen)
        {
            let tap_required = input_stage.is_some() && modulation.follows_input();
            let chunk_frames = if tap_required {
                modulation
                    .chunk_frames()
                    .min(host_input_gain_modulation::TAP_FRAMES)
            } else {
                modulation.chunk_frames()
            };
            let mut offset = 0usize;
            for chunk in frames.chunks_mut(chunk_frames) {
                let end = offset
                    .checked_add(chunk.len())
                    .ok_or(DeviceContractError::InvalidDescriptor)?;
                let shifted = clock.map(|clock| OffsetClock { clock, offset });
                let chunk_clock = shifted
                    .as_ref()
                    .map(|clock| clock as &dyn NativeInstrumentClock);
                let shifted_values =
                    upstream.map(|values| OffsetParameterValues { values, offset });
                let chunk_values = shifted_values
                    .as_ref()
                    .map(|values| values as &dyn NativeParameterAutomation);
                let chunk_key = key
                    .map(|key| {
                        key.get(offset..end)
                            .ok_or(DeviceContractError::InvalidDescriptor)
                    })
                    .transpose()?;
                let mut tap;
                let follower_input = if let Some(input) = input_stage.filter(|_| tap_required) {
                    tap = [[0.0; 2]; host_input_gain_modulation::TAP_FRAMES];
                    let tap = &mut tap[..chunk.len()];
                    host_input_gain_modulation::prepare_tap(
                        tap,
                        chunk,
                        entry.input_gain,
                        input,
                        offset,
                        entry.scratch_matrix_ramp,
                        entry.matrix,
                    );
                    Some(&*tap)
                } else {
                    None
                };
                modulation.render_audio_chunk(
                    chunk,
                    follower_input,
                    chunk_values,
                    chunk_clock,
                    |chunk, values| {
                        if let Some(input) = input_stage {
                            if let Some(gain) =
                                host_input_gain_modulation::apply(input, values, offset, chunk)?
                            {
                                entry.scratch_host_gains[0] = gain;
                            }
                            if entry.metering {
                                entry.meter.observe_input(chunk);
                            }
                            entry.scratch_matrix_ramp.apply_block(entry.matrix, chunk);
                        }
                        if let Some(key) = chunk_key {
                            runtime.process_auxiliary(
                                StereoAuxiliaryBlock::paired(chunk, key),
                                chunk_clock,
                                (!values.parameter_ids().is_empty()).then_some(values),
                                entry.bypassed,
                            )
                        } else if values.parameter_ids().is_empty() {
                            if entry.bypassed {
                                runtime.process_bypassed_with_clock(chunk, chunk_clock)
                            } else {
                                runtime.process_with_clock(chunk, chunk_clock)
                            }
                        } else {
                            runtime.process_automation(chunk, chunk_clock, values, entry.bypassed)
                        }?;
                        if let Some(host) = host_values.as_ref()
                            && let Some(gain) = host_output_gain_ramp::apply_modulated(
                                &mut entry.scratch_output_gain_ramp,
                                transient,
                                host,
                                values,
                                offset,
                                chunk,
                            )?
                        {
                            entry.scratch_host_gains[1] = gain;
                        }
                        Ok(())
                    },
                )?;
                offset = end;
            }
            if host_values.is_some() && !frames.is_empty() {
                host_output_gain_ramp::finish_release(entry);
            }
            Ok(())
        } else if let Some(key) = key {
            runtime.process_auxiliary(
                StereoAuxiliaryBlock::paired(frames, key),
                clock,
                upstream,
                entry.bypassed,
            )
        } else if let Some(values) = upstream {
            runtime.process_automation(frames, clock, values, entry.bypassed)
        } else if entry.bypassed {
            runtime.process_bypassed_with_clock(frames, clock)
        } else {
            runtime.process_with_clock(frames, clock)
        }
    }

    fn apply_host_gain(
        entry: &mut RuntimeEntry,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        input: bool,
        transient: Option<NativeTransientValue>,
        external: Option<&dyn ExternalParameterAutomation>,
    ) -> Result<(), DeviceContractError> {
        let index = if input { 0 } else { 1 };
        let base = if input {
            entry.input_gain
        } else {
            entry.output_gain
        };
        let target = if input {
            hex_lattice_contract::DeviceAutomationTarget::HostInputGain
        } else {
            hex_lattice_contract::DeviceAutomationTarget::HostOutputGain
        };
        use host_output_gain_ramp::TransientGain;
        if !input
            && !entry.automation_frozen
            && let TransientGain::Held(gain) = entry.scratch_transient_output_gain
        {
            if let Some(external) = external {
                entry
                    .automation
                    .block_with_external(clock, frames.len(), None, external)?;
            }
            if let Some(applied) = entry.scratch_output_gain_ramp.apply(gain.linear(), frames) {
                entry.scratch_host_gains[index] = applied;
            }
            return Ok(());
        }
        if entry.automation_frozen
            || (!entry.automation.has_host_target(target)
                && transient.is_none_or(|value| value.target() != target))
        {
            let gain = if entry.automation_frozen {
                entry.committed_host_gains[index]
            } else {
                base
            };
            // Static HostOutputGain uses the 5 ms OutputGainRamp across compatible
            // handoffs and transient trims. Static HostInputGain intentionally does
            // not share OutputGainRamp: it feeds nonlinear drive/threshold stages,
            // is not a transient preview target, and incompatible static input-gain
            // edits rely on the chain's de-click cut while automated/modulated input
            // gain runs per-sample via `host_input_gain_modulation` / `host_gain_at`.
            if !input && !entry.automation_frozen {
                if let Some(gain) = entry.scratch_output_gain_ramp.apply(gain, frames) {
                    entry.scratch_host_gains[index] = gain;
                    host_output_gain_ramp::finish_release(entry);
                }
            } else {
                if gain != 1.0 {
                    for frame in frames.iter_mut() {
                        frame[0] *= gain;
                        frame[1] *= gain;
                    }
                }
                if !frames.is_empty() {
                    entry.scratch_host_gains[index] = gain;
                }
            }
            return Ok(());
        }
        let values = entry.automation.block_with_optional_external(
            clock,
            frames.len(),
            transient,
            external,
        )?;
        let releasing = !input && entry.scratch_transient_output_gain == TransientGain::Releasing;
        if !input && !releasing && !frames.is_empty() {
            entry.scratch_output_gain_ramp = host_output_gain_ramp::OutputGainRamp::INACTIVE;
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut gain = values.host_gain_at(target, offset).unwrap_or(base);
            if releasing {
                gain = entry.scratch_output_gain_ramp.next(gain);
            }
            if gain != 1.0 {
                frame[0] *= gain;
                frame[1] *= gain;
            }
            entry.scratch_host_gains[index] = gain;
        }
        if releasing && !frames.is_empty() {
            host_output_gain_ramp::finish_release(entry);
        }
        Ok(())
    }
}
