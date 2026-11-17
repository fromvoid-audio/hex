//! The single native receipt reader also publishes disposable last-touch facts.
//! One pending fact per admitted device plus one output delivery slot bounds UI
//! backpressure. Native gesture receipts retain their existing ordered reader;
//! only the UI projection coalesces. No callback work or canonical edit is added.
use super::*;
use hex_audio::plugin_worker::{
    ParameterEdit, ParameterEditKind, RecoveryEdits, StoppedEditProgress,
};
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_session::device_gesture::PluginParameterValue;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct PendingRead {
    recovery: bool,
    source: AppliedAudioSource,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Touch {
    source: AppliedAudioSource,
    parameter: u32,
    sequence: u64,
    value: Option<PluginParameterValue>,
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
            value: (edit.kind == ParameterEditKind::Perform)
                .then(|| PluginParameterValue::new(edit.normalized).ok())
                .flatten(),
        })
    })
}

fn touch_for_read(
    current: AppliedAudioSource,
    admitted_at: AppliedAudioSource,
    edits: &[ParameterEdit],
) -> Option<Touch> {
    let mut touch = latest_touch(current, edits)?;
    if admitted_at != current {
        // Events can have arrived on either side of the source transition.
        touch.value = None;
    }
    Some(touch)
}

fn continue_stopped_read(
    live: Result<Option<RecoveryEdits>, &'static str>,
    pending: Option<PendingRead>,
    stopped: impl FnOnce(bool) -> Result<StoppedEditProgress, &'static str>,
) -> Result<Option<RecoveryEdits>, &'static str> {
    match (live, pending) {
        (Err("native edit source is not live"), Some(pending)) => {
            match stopped(pending.recovery)? {
                StoppedEditProgress::Pending => Ok(None),
                StoppedEditProgress::Completed(batch) => Ok(Some(batch)),
            }
        }
        (result, _) => result,
    }
}

#[derive(Default)]
pub(super) struct PollSchedule {
    ordinary: usize,
    expedited: usize,
    next_ordinary: Option<Instant>,
}

impl PollSchedule {
    pub(super) fn request_ordinary_poll(&mut self) {
        self.next_ordinary = None;
    }

    fn select(
        &mut self,
        sources: &[Source],
        generation: u64,
        recording: Option<Binding>,
        now: Instant,
    ) -> [Option<usize>; 2] {
        let eligible = |source: &Source| {
            source.accepted && !source.retiring && source.project_generation == generation
        };
        let mut ordinary = None;
        if self.next_ordinary.is_none_or(|next| now >= next) {
            self.next_ordinary = now.checked_add(Duration::from_millis(10));
            if let Some(recording) = recording {
                ordinary = sources
                    .iter()
                    .position(|source| source.binding == recording && eligible(source));
            } else {
                for _ in 0..sources.len() {
                    let index = self.ordinary % sources.len();
                    self.ordinary = (index + 1) % sources.len();
                    if eligible(&sources[index]) {
                        ordinary = Some(index);
                        break;
                    }
                }
            }
        }
        let mut expedited = None;
        for _ in 0..sources.len() {
            let index = self.expedited % sources.len();
            self.expedited = (index + 1) % sources.len();
            let source = &sources[index];
            if Some(index) != ordinary
                && Some(source.binding) != recording
                && eligible(source)
                && (source.editor_open || source.edit_recovery.is_some() || source.edit_backlog)
            {
                expedited = Some(index);
                break;
            }
        }
        [ordinary, expedited]
    }
}

impl Plugins {
    /// Poll at most two distinct original workers per control turn, without
    /// waiting. Ordinary polling retains its 10ms cadence and recording-first
    /// selection, otherwise rotating over all accepted current sources. One extra
    /// round-robin poll progresses an open editor, admitted read or receipt backlog,
    /// excluding the recording source. Idle inventory cannot delay that rotation.
    /// A recovery notice stops the turn before another source is polled.
    ///
    /// Ordinary resident graphs retain stopped-edit flushing and latency recovery.
    /// Other retained sources observe without opting into that recovery contract.
    /// Drain a previously requested mode before switching, so one reader owns the
    /// mailbox. Expedited polling changes neither receipt order nor capacity.
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
        let recording = self.recording_binding().filter(|binding| {
            self.sources
                .iter()
                .any(|source| source.binding == *binding && source.accepted && !source.retiring)
        });
        let selected =
            self.parameter_poll
                .select(&self.sources, context.project_generation, recording, now);
        for index in selected.into_iter().flatten() {
            if let Some(notice) = self.poll_parameter_source(
                control,
                active,
                recovery && recording.is_none(),
                index,
                now,
            ) {
                return Some(notice);
            }
        }
        None
    }

    fn poll_parameter_source(
        &mut self,
        control: &mut PublicationControl,
        active: AppliedAudioSource,
        recovery: bool,
        index: usize,
        now: Instant,
    ) -> Option<AppliedAudioSource> {
        let source = &mut self.sources[index];
        let pending_read = source.edit_recovery;
        let requested_recovery = pending_read.map(|read| read.recovery).unwrap_or(recovery);
        let receipt_source = pending_read.map_or(active, |read| read.source);
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
        let result = continue_stopped_read(result, pending_read, |mode| {
            control.poll_native_source_stopped_edits_on_control(source.binding, mode)
        });
        let batch = match result {
            Ok(batch) => {
                source.edit_recovery = batch.is_none().then_some(PendingRead {
                    recovery: requested_recovery,
                    source: receipt_source,
                });
                batch
            }
            Err(
                reason @ ("native stopped edit worker failed"
                | "native stopped edit read failed"
                | "native stopped edit source differs"
                | "native stopped edit owner missing"
                | "native stopped edit read was not admitted"
                | "native edit observation mode differs"
                | "native edit result custody differs"
                | "native edit mailbox failed"),
            ) => {
                source.edit_recovery = None;
                if let Some(log) = &self.diagnostics {
                    let _ = log.record_typed(hex_diagnostics::Level::Warn, hex_diagnostics::Category::Audio,
                            &format!("vst3_editor v=1 stage=stopped_read_failed device={} binding={:?} reason={reason}", source.device, source.binding));
                }
                return None;
            }
            Err(_) => return None, // Busy preserves the admitted request and prior touch.
        };
        if let Some(batch) = batch {
            source.edit_backlog = batch.edits.pending != 0;
            if let Some(touch) = touch_for_read(active, receipt_source, batch.edits.as_slice()) {
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
                if let Some(progress) = &self.progress
                    && let Ok(mut progress) = progress.lock()
                {
                    for edit in batch.edits.as_slice() {
                        let index = progress.editor_receipt_count;
                        if let Some(slot) = progress.editor_receipts.get_mut(index) {
                            *slot = Some(*edit);
                            progress.editor_receipt_count += 1;
                        }
                    }
                }
            }
            let binding = source.binding;
            self.observe_recording_order(binding, batch.edits.as_slice());
            if requested_recovery
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
                    value: touch.value,
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
            editor_transfer: false,
            edit_backlog: false,
            edit_recovery: None,
            pending_touch: Some(Touch {
                source: active,
                parameter,
                sequence: 8,
                value: None,
            }),
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }
    fn scheduling_sources() -> (AppliedAudioSource, Vec<Source>) {
        let active = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            output_identity: 2,
            audio_revision: 9,
        };
        let sources = (1..=24).map(|id| source(id, active, 42)).collect();
        (active, sources)
    }

    #[test]
    fn editor_receipts_progress_between_background_reads_through_pending_and_backlog() {
        let (active, mut sources) = scheduling_sources();
        let mut schedule = PollSchedule::default();
        let now = Instant::now();
        sources[23].editor_open = true;
        assert_eq!(schedule.select(&sources, 3, None, now), [Some(0), Some(23)]);
        // The editor closes after submitting a read. Its exact pending reader
        // must still progress without waiting for 23 idle source polls.
        sources[23].editor_open = false;
        sources[23].edit_recovery = Some(PendingRead {
            recovery: true,
            source: active,
        });
        assert_eq!(
            schedule.select(&sources, 3, None, now + Duration::from_millis(5)),
            [None, Some(23)]
        );
        sources[23].edit_recovery = None;
        sources[23].edit_backlog = true;
        assert_eq!(
            schedule.select(&sources, 3, None, now + Duration::from_millis(6)),
            [None, Some(23)]
        );
        sources[23].edit_backlog = false;
        assert_eq!(
            schedule.select(&sources, 3, None, now + Duration::from_millis(7)),
            [None, None]
        );
        sources[7].editor_open = true;
        sources[23].editor_open = true;
        assert_eq!(
            schedule.select(&sources, 3, None, now + Duration::from_millis(8)),
            [None, Some(7)]
        );
        schedule.request_ordinary_poll();
        assert_eq!(
            schedule.select(&sources, 3, None, now + Duration::from_millis(9)),
            [Some(1), Some(23)],
            "requesting an immediate read preserves both fair rotations"
        );
    }

    #[test]
    fn sustained_editors_rotate_fairly_without_starving_background_or_duplicate_reads() {
        let (_, mut sources) = scheduling_sources();
        sources[2].editor_open = true;
        sources[7].edit_backlog = true;
        let mut schedule = PollSchedule::default();
        let now = Instant::now();
        let mut served = [0; 24];
        for turn in 0..24 {
            let selected =
                schedule.select(&sources, 3, None, now + Duration::from_millis(turn * 10));
            assert_eq!(selected[0], Some(turn as usize));
            assert_ne!(selected[0], selected[1]);
            assert!(matches!(selected[1], Some(2 | 7)));
            for index in selected.into_iter().flatten() {
                served[index] += 1;
            }
        }
        assert!(served.iter().all(|count| *count >= 1));
        assert!(served[2] >= 12 && served[7] >= 12);
    }

    #[test]
    fn recording_retains_ordinary_cadence_and_invalid_sources_are_never_expedited() {
        let (_, mut sources) = scheduling_sources();
        sources[0].editor_open = true;
        sources[1].editor_open = true;
        sources[1].retiring = true;
        sources[2].edit_backlog = true;
        sources[2].accepted = false;
        sources[3].editor_open = true;
        sources[3].project_generation = 99;
        sources[23].edit_backlog = true;
        let recording = Some(sources[0].binding);
        let now = Instant::now();
        let mut schedule = PollSchedule::default();
        assert_eq!(
            schedule.select(&sources, 3, recording, now),
            [Some(0), Some(23)]
        );
        assert_eq!(
            schedule.select(&sources, 3, recording, now + Duration::from_millis(5)),
            [None, Some(23)]
        );
        assert_eq!(
            schedule.select(&sources, 3, recording, now + Duration::from_millis(10)),
            [Some(0), Some(23)]
        );
        // Starting recording requests its first ordinary read immediately,
        // without resetting either source rotation.
        schedule.request_ordinary_poll();
        assert_eq!(
            schedule.select(&sources, 3, recording, now + Duration::from_millis(11)),
            [Some(0), Some(23)]
        );
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
            value: None,
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
            value: None,
        });
        plugins.sources[0].retiring = true;
        assert!(plugins.take_parameter_touch().is_none());
        plugins.sources[0].retiring = false;
        plugins.sources[0].pending_touch = Some(Touch {
            source: active,
            parameter: 43,
            sequence: 11,
            value: None,
        });
        plugins.output_closed();
        assert!(plugins.take_parameter_touch().is_none());
    }
    #[test]
    fn latest_touch_follows_native_intent_and_ignores_non_touch_markers() {
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
            normalized: if kind == ParameterEditKind::Perform {
                0.625
            } else {
                0.0
            },
            ..Default::default()
        };
        let edits = [
            edit(1, 42, ParameterEditKind::Perform),
            edit(2, 88, ParameterEditKind::Begin),
            edit(3, 42, ParameterEditKind::End),
            edit(4, 0, ParameterEditKind::GroupEnd),
        ];
        assert_eq!(
            latest_touch(source, &edits),
            Some(Touch {
                source,
                parameter: 88,
                sequence: 2,
                value: None,
            })
        );
        assert_eq!(latest_touch(source, &edits[2..]), None);
        assert_eq!(
            latest_touch(source, &edits[..1]).unwrap().value,
            Some(PluginParameterValue::new(0.625).unwrap())
        );
        let next = AppliedAudioSource {
            audio_revision: source.audio_revision + 1,
            ..source
        };
        let same = touch_for_read(source, source, &edits[..1]).unwrap();
        assert_eq!(same.value, Some(PluginParameterValue::new(0.625).unwrap()));
        let crossed = touch_for_read(next, source, &edits[..1]).unwrap();
        assert_eq!(
            (crossed.source, crossed.parameter, crossed.sequence),
            (next, 42, 1)
        );
        assert_eq!(crossed.value, None);
    }

    #[test]
    fn revoked_source_resumes_only_its_admitted_read_and_preserves_latency() {
        let source = AppliedAudioSource {
            context: AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            output_identity: 2,
            audio_revision: 9,
        };
        let read = PendingRead {
            recovery: true,
            source,
        };
        let latency = std::num::NonZeroU64::new(9).unwrap();
        let received =
            continue_stopped_read(Err("native edit source is not live"), Some(read), |mode| {
                assert!(mode);
                Ok(StoppedEditProgress::Completed(RecoveryEdits {
                    edits: Default::default(),
                    pending_latency_restart: Some(latency),
                }))
            })
            .unwrap()
            .unwrap();
        assert_eq!(received.pending_latency_restart, Some(latency));
        assert!(
            continue_stopped_read(Err("native edit source is not live"), None, |_| panic!(
                "no admitted read"
            ))
            .is_err()
        );
        assert!(
            continue_stopped_read(
                Err("native edit observation is busy"),
                Some(read),
                |_| panic!("live read is busy")
            )
            .is_err()
        );
        assert!(matches!(
            continue_stopped_read(Err("native edit source is not live"), Some(read), |_| Err(
                "native stopped edit worker failed"
            )),
            Err("native stopped edit worker failed")
        ));
    }
}
