//! The single native receipt reader also publishes disposable last-touch facts.
//! One pending fact per admitted device plus one output delivery slot bounds UI
//! backpressure. Native gesture receipts retain their existing ordered reader;
//! only the UI projection coalesces. No callback work or canonical edit is added.
use super::*;
use hex_audio::plugin_worker::{ParameterEdit, ParameterEditKind, RecoveryEdits};
use hex_audio::prepared::AppliedAudioSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Touch {
    source: AppliedAudioSource,
    parameter: u32,
    sequence: u64,
}

fn latest_touch(source: AppliedAudioSource, edits: &[ParameterEdit]) -> Option<Touch> {
    edits.iter().rev().find_map(|edit| {
        matches!(
            edit.kind,
            ParameterEditKind::Begin | ParameterEditKind::Perform
        )
        .then_some(Touch {
            source,
            parameter: edit.id,
            sequence: edit.sequence,
        })
    })
}

impl Plugins {
    /// Read at most one original worker each 10ms, round-robin. Ordinary resident
    /// graphs retain stopped-edit flushing and latency recovery. Other retained
    /// sources observe without opting into that recovery contract. Drain a
    /// previously requested mode before switching, so one reader owns the mailbox.
    pub fn poll_parameter_edits(
        &mut self,
        output: &mut WorkerPreparedOutput,
        context: AudioSourceContext,
        recovery_allowed: bool,
    ) -> Option<AppliedAudioSource> {
        let now = std::time::Instant::now();
        if self.restore.is_some()
            || self.edit.is_some()
            || self.pending.is_some()
            || self.reopen.is_some()
            || self.sources.is_empty()
            || self.latency_poll_at.is_some_and(|at| now < at)
        {
            return None;
        }
        let control = output.publication.control()?;
        let active = control
            .status
            .applied_audio_source()
            .or_else(|| control.status.native_effect_source())
            .filter(|source| source.context == context)?;
        let recovery = recovery_allowed && control.status.applied_audio_source().is_none();
        self.latency_poll_at = now.checked_add(std::time::Duration::from_millis(10));
        let len = self.sources.len();
        let recording = self.recording_binding().and_then(|binding| {
            self.sources
                .iter()
                .position(|source| source.binding == binding && source.accepted && !source.retiring)
        });
        for _ in 0..len {
            let index = recording.unwrap_or(self.latency_poll % len);
            if recording.is_none() {
                self.latency_poll = (index + 1) % len;
            }
            let source = &mut self.sources[index];
            if !source.accepted
                || source.retiring
                || source.project_generation != context.project_generation
            {
                if recording.is_some() {
                    return None;
                }
                continue;
            }
            let requested_recovery = source
                .edit_recovery
                .unwrap_or(recovery && recording.is_none());
            source.observes_latency |= requested_recovery;
            let result = if requested_recovery {
                control.poll_native_source_recovery_edits_on_control(source.binding)
            } else {
                control
                    .poll_native_source_parameter_edits_on_control(source.binding)
                    .map(|batch| {
                        batch.map(|edits| RecoveryEdits {
                            edits,
                            pending_latency_restart: None,
                        })
                    })
            };
            let batch = match result {
                Ok(batch) => {
                    source.edit_recovery = batch.is_none().then_some(requested_recovery);
                    batch
                }
                Err(_) => return None, // Busy preserves the admitted request and prior touch.
            };
            if let Some(batch) = batch {
                source.edit_backlog = batch.edits.pending != 0;
                if let Some(touch) = latest_touch(active, batch.edits.as_slice()) {
                    source.pending_touch = Some(touch);
                }
                if let Some(log) = &self.diagnostics
                    && let [first, ..] = batch.edits.as_slice()
                    && let Some(last) = batch.edits.as_slice().last()
                {
                    let _ = log.record_typed(hex_diagnostics::Level::Debug, hex_diagnostics::Category::Audio,
                        &format!("vst3_editor v=1 stage=drain device={} binding={:?} transport_playing={} first_sequence={} last_sequence={} first_frame={} last_frame={} receipts={} pending={} ready={}",
                        source.device, source.binding, control.status.playing.load(Acquire), first.sequence,
                        last.sequence, first.first_frame, last.first_frame, batch.edits.as_slice().len(), batch.edits.pending, batch.edits.ready));
                }
                #[cfg(all(test, feature = "allocation-probe"))]
                {
                    source.observed_latency_receipt = true;
                }
                let binding = source.binding;
                self.observe_recording_order(binding, batch.edits.as_slice());
                if recovery
                    && batch.pending_latency_restart.is_some()
                    && self.latency_notice.is_none_or(|(last, at)| {
                        last != active
                            || now.saturating_duration_since(at)
                                >= std::time::Duration::from_millis(150)
                    })
                {
                    self.latency_notice = Some((active, now));
                    return Some(active);
                }
            }
            return None;
        }
        None
    }

    pub fn take_parameter_touch(&mut self) -> Option<EditorAudioEvent> {
        self.sources.iter_mut().find_map(|source| {
            let touch = source.pending_touch.take()?;
            (source.accepted && !source.retiring).then_some(
                EditorAudioEvent::PluginParameterTouched {
                    source: touch.source,
                    binding: source.binding,
                    device: source.device,
                    parameter: touch.parameter,
                    sequence: touch.sequence,
                },
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(device: u64, active: AppliedAudioSource, parameter: u32) -> Source {
        Source {
            binding: Binding {
                root: 1,
                instance: device,
                graph: 3,
                epoch: 4,
            },
            owner: ProcessorChainOwner::Main,
            device,
            project_generation: active.context.project_generation,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: true,
            editor_open: false,
            edit_backlog: false,
            edit_recovery: None,
            pending_touch: Some(Touch {
                source: active,
                parameter,
                sequence: 8,
            }),
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }
    #[test]
    fn coalesced_device_touches_survive_backpressure_and_retirement_suppresses_delivery() {
        let active = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            output_identity: 2,
            audio_revision: 9,
        };
        let mut plugins = Plugins::default();
        plugins.sources.push(source(10, active, 42));
        plugins.sources.push(source(20, active, 88));
        assert!(!plugins.idle()); // UI delivery has not taken either device.
        plugins.sources[0].pending_touch = Some(Touch {
            source: active,
            parameter: 43,
            sequence: 9,
        });
        assert!(matches!(
            plugins.take_parameter_touch(),
            Some(EditorAudioEvent::PluginParameterTouched {
                device: 10,
                parameter: 43,
                sequence: 9,
                ..
            })
        ));
        assert!(matches!(
            plugins.take_parameter_touch(),
            Some(EditorAudioEvent::PluginParameterTouched {
                device: 20,
                parameter: 88,
                ..
            })
        ));
        assert!(plugins.take_parameter_touch().is_none());
        assert!(plugins.idle());
        plugins.sources[0].pending_touch = Some(Touch {
            source: active,
            parameter: 43,
            sequence: 10,
        });
        plugins.sources[0].retiring = true;
        assert!(plugins.take_parameter_touch().is_none());
        plugins.sources[0].retiring = false;
        plugins.sources[0].pending_touch = Some(Touch {
            source: active,
            parameter: 43,
            sequence: 11,
        });
        plugins.output_closed();
        assert!(plugins.take_parameter_touch().is_none());
    }
    #[test]
    fn latest_touch_follows_native_intent_without_treating_end_as_another_touch() {
        let source = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            output_identity: 2,
            audio_revision: 9,
        };
        let edit = |sequence, id, kind| ParameterEdit {
            sequence,
            id,
            kind,
            ..Default::default()
        };
        let edits = [
            edit(1, 42, ParameterEditKind::Perform),
            edit(2, 88, ParameterEditKind::Begin),
            edit(3, 42, ParameterEditKind::End),
        ];
        assert_eq!(
            latest_touch(source, &edits),
            Some(Touch {
                source,
                parameter: 88,
                sequence: 2
            })
        );
        assert_eq!(latest_touch(source, &edits[2..]), None);
    }
}
