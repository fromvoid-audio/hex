//! One explicit hosted Off source and its exact installed-source retirement.
//! This control adapter holds only copied identity. Root owns every graph,
//! endpoint, editor/native lease and the original worker through ordinary return.
use super::*;

impl Plugins {
    pub(in crate::native_audio) fn power_off_binding(
        &self,
        context: AudioSourceContext,
        owner: ProcessorChainOwner,
        device: u64,
    ) -> Option<Binding> {
        self.sources
            .iter()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.project_generation == context.project_generation
                    && source.device == device
                    && source.owner == owner
                    && !source.editor_open
                    && !source.editor_transfer
                    && !source.edit_backlog
                    // The existing retirement lease takes over an admitted
                    // read after the exact cut, just as quiescent(true) does.
                    && (source.edit_recovery.is_none()
                        || source.editor_retirement.lease.is_some())
                    && source.open_editor_group.is_none()
                    && source.pending_touch.is_none()
            })
            .filter(|source| !self.editor_holds(source.binding))
            .filter(|_| self.parameter_gesture.is_none())
            .map(|source| source.binding)
    }

    /// Exact Off ends admission before the terminal. An exact Main Off pass
    /// stamp can stage Main Power On without granting worker custody.
    pub(in crate::native_audio) fn finish_power_off(
        &mut self,
        context: AudioSourceContext,
        accepted: bool,
        applied: Option<hex_audio::prepared::AppliedAudioSource>,
        off: Option<hex_audio::prepared::NativeProcessorIdentity>,
    ) -> Option<(hex_audio::prepared::AppliedAudioSource, u64)> {
        let (intent, candidate, stop_generation) = self
            .power_off
            .filter(|(_, candidate, _)| *candidate == context)?;
        self.power_off = None;
        let applied = applied?;
        if !accepted
            || !off
                .is_some_and(|off| off.owner() == intent.owner && off.device_id() == intent.device)
            || applied.context != candidate
            || applied.output_identity != intent.source.output_identity
            || applied.audio_revision == intent.source.audio_revision
        {
            return None;
        }
        let source = self.sources.iter_mut().find(|source| {
            source.binding == intent.binding
                && source.device == intent.device
                && source.owner == intent.owner
                && source.project_generation == candidate.project_generation
        })?;
        source.accepted = false;
        source.retiring = true;
        stop_generation
            .filter(|_| intent.owner == ProcessorChainOwner::Main)
            .map(|generation| (applied, generation))
    }

    pub(super) fn prepare_running_power_off(
        &mut self,
        mut output: Option<&mut WorkerPreparedOutput>,
        input: &ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<PreparedRevision, String> {
        let intent = input
            .running_power_off()
            .ok_or("running power intent changed")?;
        let output = output
            .as_mut()
            .ok_or("running power output is unavailable")?;
        let control = output
            .publication
            .control()
            .ok_or("running power output closed")?;
        let actual = control.status.recording_source();
        if actual.is_none_or(|actual| {
            actual.context != intent.source.context
                || actual.output_identity != intent.source.output_identity
                || actual.audio_revision != intent.source.audio_revision
        }) || !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
            || actual != control.status.native_effect_source()
            || self.power_off_binding(intent.source.context, intent.owner, intent.device)
                != Some(intent.binding)
        {
            return Err("running power source changed".into());
        }
        let stop_generation = if intent.owner == ProcessorChainOwner::Main {
            control
                .capture_fresh_runtime_fence()
                .filter(|fence| fence.expected_active_revision() == intent.source.audio_revision)
                .map(hex_audio::prepared::FreshRuntimeFence::consumed_stop_generation)
        } else {
            None
        };
        let target = match intent.owner {
            ProcessorChainOwner::Main => input
                .powered_off_main()
                .map(|(device, state)| (intent.owner, device, state)),
            ProcessorChainOwner::Return(_) => input.powered_off_return(),
            _ => None,
        };
        let (owner, device, state) = target
            .filter(|(owner, device, _)| (*owner, *device) == (intent.owner, intent.device))
            .ok_or("power off target changed")?;
        let eligible = control.with_native_effect_definition(&[intent.binding], |definition| {
            definition
                .matches_power_off_on_control(
                    owner,
                    intent.binding,
                    device,
                    state,
                    input.sample_rate(),
                )
                .map_err(|error| error.to_string())
        })?;
        if !eligible {
            return Err("running power off requires the original 512-frame source".into());
        }
        let prepared = Hosting::prepare_powered_off_on_control(
            hex_audio::plugin_effects::PoweredOffEffect {
                owner,
                device_id: device,
                sample_rate: input.sample_rate(),
                state,
            },
            prepare,
        )?;
        self.power_off = Some((intent, input.context(), stop_generation));
        self.edit = Some(input.context());
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(binding: Binding) -> Source {
        Source {
            binding,
            provider: None,
            owner: ProcessorChainOwner::Main,
            device: 7,
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }

    #[test]
    fn only_exact_applied_off_ends_the_original_bindings_admission() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let previous = AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let candidate = AudioSourceContext {
            editor_revision: 3,
            ..previous
        };
        let intent = hex_session::service::ResidentRunningPowerOff {
            owner: ProcessorChainOwner::Main,
            device: 7,
            binding,
            source: hex_session::service::ResidentGraphSubmission {
                context: previous,
                output_identity: 1,
                audio_revision: 8,
            },
        };
        let applied = hex_audio::prepared::AppliedAudioSource {
            context: candidate,
            output_identity: 1,
            audio_revision: 9,
        };
        for case in 0..7 {
            let mut plugins = Plugins::default();
            let owner = if case == 6 {
                ProcessorChainOwner::Return(1)
            } else {
                intent.owner
            };
            let mut primary = source(binding);
            primary.owner = owner;
            plugins.sources.push(primary);
            let other = Binding {
                instance: 5,
                ..binding
            };
            plugins.sources.push(source(other));
            plugins.power_off = Some((
                hex_session::service::ResidentRunningPowerOff { owner, ..intent },
                candidate,
                (case != 5).then_some(17),
            ));
            let exact_off = plugins.finish_power_off(
                candidate,
                case != 0,
                Some(match case {
                    1 => hex_audio::prepared::AppliedAudioSource {
                        audio_revision: 8,
                        ..applied
                    },
                    2 => hex_audio::prepared::AppliedAudioSource {
                        output_identity: 6,
                        ..applied
                    },
                    _ => applied,
                }),
                hex_audio::prepared::NativeProcessorIdentity::new(
                    if case == 3 {
                        ProcessorChainOwner::Return(1)
                    } else {
                        owner
                    },
                    7,
                )
                .ok(),
            );
            assert_eq!(exact_off, (case == 4).then_some((applied, 17)));
            assert_eq!(plugins.sources[0].retiring, case >= 4);
            assert_eq!(plugins.sources[0].accepted, case < 4);
            assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);
            assert!(plugins.power_off.is_none());
        }
        let mut plugins = Plugins::default();
        plugins.sources.push(source(binding));
        assert_eq!(
            plugins.power_off_binding(previous, ProcessorChainOwner::Main, 7),
            Some(binding)
        );
        plugins.sources[0].editor_open = true;
        assert_eq!(
            plugins.power_off_binding(previous, ProcessorChainOwner::Main, 7),
            None
        );
    }
}
