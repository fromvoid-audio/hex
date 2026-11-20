//! One explicit Main Off source and its exact installed-source retirement.
//! This control adapter holds only copied identity. Root owns every graph,
//! endpoint, editor/native lease and the original worker through ordinary return.
use super::*;

impl Plugins {
    pub(in crate::native_audio) fn main_power_off_binding(
        &self,
        context: AudioSourceContext,
        device: u64,
    ) -> Option<Binding> {
        self.sources
            .iter()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.project_generation == context.project_generation
                    && source.device == device
                    && source.owner == ProcessorChainOwner::Main
                    && !source.editor_open
                    && !source.editor_transfer
                    && !source.edit_backlog
                    && source.edit_recovery.is_none()
                    && source.open_editor_group.is_none()
                    && source.pending_touch.is_none()
            })
            .filter(|source| !self.editor_holds(source.binding))
            .filter(|_| self.parameter_gesture.is_none())
            .map(|source| source.binding)
    }

    /// Exact Off ends admission before the terminal, but grants no reclamation.
    pub(in crate::native_audio) fn finish_power_off(
        &mut self,
        context: AudioSourceContext,
        accepted: bool,
        applied: Option<hex_audio::prepared::AppliedAudioSource>,
        off: Option<u64>,
    ) {
        let Some((intent, candidate)) = self
            .power_off
            .filter(|(_, candidate)| *candidate == context)
        else {
            return;
        };
        self.power_off = None;
        if !accepted
            || off != Some(intent.device)
            || applied.is_none_or(|actual| {
                actual.context != candidate
                    || actual.output_identity != intent.source.output_identity
                    || actual.audio_revision == intent.source.audio_revision
            })
        {
            return;
        }
        if let Some(source) = self.sources.iter_mut().find(|source| {
            source.binding == intent.binding
                && source.device == intent.device
                && source.owner == ProcessorChainOwner::Main
                && source.project_generation == candidate.project_generation
        }) {
            source.accepted = false;
            source.retiring = true;
        }
    }

    pub(super) fn prepare_main_power_off(
        &mut self,
        mut output: Option<&mut WorkerPreparedOutput>,
        input: &ResidentEffectPreparation<'_>,
        prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<PreparedRevision, hex_engine::Invalid>,
    ) -> Result<PreparedRevision, String> {
        let intent = input
            .running_main_power_off()
            .ok_or("running main power intent changed")?;
        let output = output
            .as_mut()
            .ok_or("running main output is unavailable")?;
        let control = output
            .publication
            .control()
            .ok_or("running main output closed")?;
        let actual = control.status.recording_source();
        if actual.is_none_or(|actual| {
            actual.context != intent.source.context
                || actual.output_identity != intent.source.output_identity
                || actual.audio_revision != intent.source.audio_revision
        }) || !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
            || self.main_power_off_binding(intent.source.context, intent.device)
                != Some(intent.binding)
        {
            return Err("running main power source changed".into());
        }
        let (device, state) = input
            .powered_off_main()
            .ok_or("main power target changed")?;
        let eligible = control.with_native_effect_definition(&[intent.binding], |definition| {
            definition
                .matches_main_power_off_on_control(
                    intent.binding,
                    device,
                    state,
                    input.sample_rate(),
                )
                .map_err(|error| error.to_string())
        })?;
        if !eligible {
            return Err("running main power requires the original 512-frame source".into());
        }
        let prepared = Hosting::prepare_powered_off_on_control(
            hex_audio::plugin_effects::PoweredOffEffect {
                owner: ProcessorChainOwner::Main,
                device_id: device,
                sample_rate: input.sample_rate(),
                state,
            },
            prepare,
        )?;
        self.power_off = Some((intent, input.context()));
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
        let intent = hex_session::service::ResidentMainPowerOff {
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
        for case in 0..5 {
            let mut plugins = Plugins::default();
            plugins.sources.push(source(binding));
            let other = Binding {
                instance: 5,
                ..binding
            };
            plugins.sources.push(source(other));
            plugins.power_off = Some((intent, candidate));
            plugins.finish_power_off(
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
                if case == 3 { None } else { Some(7) },
            );
            assert_eq!(plugins.sources[0].retiring, case == 4);
            assert_eq!(plugins.sources[0].accepted, case != 4);
            assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);
            assert!(plugins.power_off.is_none());
        }
        let mut plugins = Plugins::default();
        plugins.sources.push(source(binding));
        assert_eq!(plugins.main_power_off_binding(previous, 7), Some(binding));
        plugins.sources[0].editor_open = true;
        assert_eq!(plugins.main_power_off_binding(previous, 7), None);
    }
}
