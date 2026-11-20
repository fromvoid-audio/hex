//! Copied, bounded editor receipts offered by the native source owner.
//! Session must retain every accepted event through its final disposition;
//! these values neither own a worker nor authorize a project transition.

use hex_engine::AudioSourceContext;
use hex_lattice_contract::ForeignProcessorBinding;

/// Bounded source-owned sequence presented after the original producer seals.
pub const MAX_RETIRING_EDITOR_EVENTS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiringEditorEventKind {
    Begin,
    Perform,
    End,
    GroupBegin,
    GroupEnd,
    GroupAborted,
}

#[derive(Clone, Copy, Debug)]
pub struct RetiringEditorEvent {
    pub sequence: u64,
    pub gesture: u64,
    pub parameter: u32,
    pub normalized: f64,
    pub first_frame: u64,
    pub kind: RetiringEditorEventKind,
}

impl PartialEq for RetiringEditorEvent {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
            && self.gesture == other.gesture
            && self.parameter == other.parameter
            && self.normalized.to_bits() == other.normalized.to_bits()
            && self.first_frame == other.first_frame
            && self.kind == other.kind
    }
}

/// A sealed original receipt sequence and the successor named at transition.
/// Validate the current source and canonical project revision before applying it.
#[derive(Clone, Debug, PartialEq)]
pub struct RetiringEditorBatch {
    pub source: ForeignProcessorBinding,
    pub successor: ForeignProcessorBinding,
    pub successor_context: AudioSourceContext,
    pub device: u64,
    /// Highest sequence admitted on this binding before successor preparation.
    /// A later receipt proves later admission, not a difference in saved state.
    pub predecessor_admitted_through: Option<u64>,
    events: Vec<RetiringEditorEvent>,
}

/// Whether the successor revision reached the actual audio owner. Receipt
/// acknowledgement can still fail after that revision becomes canonical.
pub enum RetiringEditorReplayDisposition {
    Refused,
    Published { receipt_accepted: bool },
}

impl RetiringEditorBatch {
    pub fn new(
        source: ForeignProcessorBinding,
        successor: ForeignProcessorBinding,
        successor_context: AudioSourceContext,
        device: u64,
        predecessor_admitted_through: Option<u64>,
        events: Vec<RetiringEditorEvent>,
    ) -> Result<Self, &'static str> {
        if !source.is_valid()
            || !successor.is_valid()
            || source == successor
            || source.root != successor.root
            || device == 0
            || events.is_empty()
            || events.len() > MAX_RETIRING_EDITOR_EVENTS
        {
            return Err("invalid retiring vst3 editor batch");
        }
        let mut previous = 0;
        let mut previous_frame = 0;
        for event in &events {
            let begins = matches!(
                event.kind,
                RetiringEditorEventKind::Begin | RetiringEditorEventKind::GroupBegin
            );
            let group = matches!(
                event.kind,
                RetiringEditorEventKind::GroupBegin
                    | RetiringEditorEventKind::GroupEnd
                    | RetiringEditorEventKind::GroupAborted
            );
            if event.sequence <= previous
                || event.gesture == 0
                || event.gesture > event.sequence
                || begins != (event.gesture == event.sequence)
                || event.parameter > i32::MAX as u32
                || (group && event.parameter != 0)
                || !event.normalized.is_finite()
                || !(0.0..=1.0).contains(&event.normalized)
                || (event.kind != RetiringEditorEventKind::Perform && event.normalized != 0.0)
                || event.first_frame < previous_frame
            {
                return Err("invalid retiring vst3 editor event");
            }
            previous = event.sequence;
            previous_frame = event.first_frame;
        }
        Ok(Self {
            source,
            successor,
            successor_context,
            device,
            predecessor_admitted_through,
            events,
        })
    }

    pub fn events(&self) -> &[RetiringEditorEvent] {
        &self.events
    }
}

impl RetiringEditorBatch {
    fn replay_points(&self) -> Result<Vec<(u32, f64)>, &'static str> {
        let cut = self
            .predecessor_admitted_through
            .ok_or("vst3 editor replacement order could not be established")?;
        let mut points: Vec<(u32, f64)> = Vec::new();
        for event in self.events() {
            match event.kind {
                RetiringEditorEventKind::Perform if event.sequence > cut => {
                    if let Some(point) = points.iter_mut().find(|point| point.0 == event.parameter)
                    {
                        point.1 = event.normalized;
                    } else {
                        points.push((event.parameter, event.normalized));
                    }
                }
                RetiringEditorEventKind::Perform
                | RetiringEditorEventKind::GroupBegin
                | RetiringEditorEventKind::GroupEnd
                | RetiringEditorEventKind::GroupAborted => {
                    return Err("vst3 editor receipts need a separate ordered disposition");
                }
                _ => {}
            }
        }
        if points.is_empty() {
            return Err("vst3 editor receipts have no replayable value");
        }
        Ok(points)
    }
}

pub(super) struct ReplayOutcome {
    pub(super) prepared: std::sync::Arc<hex_engine::PreparedRevision>,
    pub(super) receipt_failure: Option<String>,
}

pub(super) fn apply(
    session: &mut crate::Session,
    sink: &mut dyn super::WorkerAudioSink,
    current: &mut Option<super::resident_audio::Snapshot>,
    context: AudioSourceContext,
    admission: super::sampler_export::Admission<'_>,
) -> Result<Option<ReplayOutcome>, String> {
    use super::resident_audio::PluginParameterCommit;
    let Some(batch) = sink.next_retiring_editor_replay() else {
        return Ok(None);
    };
    let result = (|| {
        if batch.successor_context != context || current.is_none() {
            return Err("vst3 editor replay project revision changed".into());
        }
        let original = session
            .project()
            .devices()
            .find(|device| device.id().value() == batch.device)
            .ok_or("vst3 editor replay device was removed")?;
        let device = original.id();
        let state = original.state().clone();
        let points = batch.replay_points()?;
        if points.iter().any(|&(parameter, _)| {
            let target = hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter };
            original.automation_target(target).is_some()
        }) {
            return Err("vst3 editor replay requires automation disposition".into());
        }
        let publication = AudioSourceContext {
            editor_revision: context
                .editor_revision
                .checked_add(1)
                .ok_or("vst3 editor replay revision exhausted")?,
            ..context
        };
        let mut capture = sink.capture_retiring_editor_replay(&batch, &points, publication)?;
        capture.before =
            super::resident_preview::plugin_parameter::bind_capture(&state, capture.before)
                .map_err(|error| error.to_string())?;
        capture.after =
            super::resident_preview::plugin_parameter::bind_capture(&state, capture.after)
                .map_err(|error| error.to_string())?;
        if capture.before == capture.after {
            return Err("vst3 editor replay did not change captured state".into());
        }
        let snapshot = session
            .plugin_parameter_resident_snapshot(
                PluginParameterCommit::Captured {
                    device,
                    capture: Box::new(capture),
                },
                publication,
                sink,
                admission,
                current.as_ref(),
            )
            .map_err(|error| error.to_string())?;
        let prepared = std::sync::Arc::clone(&snapshot.prepared);
        *current = Some(snapshot);
        // Publication is already canonical. Failure here retains the receipt
        // and cannot undo that acknowledged project transition.
        let receipt_failure = sink.accept_retiring_editor_batch(&batch).err();
        Ok(ReplayOutcome {
            prepared,
            receipt_failure,
        })
    })();
    match result {
        Ok(mut outcome) => {
            if let Err(error) = sink.finish_retiring_editor_replay(
                &batch,
                RetiringEditorReplayDisposition::Published {
                    receipt_accepted: outcome.receipt_failure.is_none(),
                },
            ) {
                outcome.receipt_failure.get_or_insert(error);
            }
            Ok(Some(outcome))
        }
        Err(reason) => {
            match sink
                .finish_retiring_editor_replay(&batch, RetiringEditorReplayDisposition::Refused)
            {
                Ok(()) => Err(reason),
                Err(restore) => Err(format!("{reason} · {restore}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(instance: u64) -> ForeignProcessorBinding {
        ForeignProcessorBinding {
            root: 1,
            instance,
            graph: instance,
            epoch: 1,
        }
    }

    fn event(sequence: u64, gesture: u64, kind: RetiringEditorEventKind) -> RetiringEditorEvent {
        RetiringEditorEvent {
            sequence,
            gesture,
            parameter: 0,
            normalized: if kind == RetiringEditorEventKind::Perform {
                0.75
            } else {
                0.0
            },
            first_frame: 64,
            kind,
        }
    }

    #[test]
    fn batch_preserves_order_and_rejects_stale_or_invalid_identity() {
        use RetiringEditorEventKind::{Begin, End, GroupBegin, GroupEnd, Perform};
        let context = AudioSourceContext {
            project_generation: 3,
            editor_revision: 4,
        };
        let events = vec![
            event(1, 1, GroupBegin),
            event(2, 2, Begin),
            event(3, 2, Perform),
            event(4, 2, End),
            event(5, 1, GroupEnd),
        ];
        let batch =
            RetiringEditorBatch::new(binding(2), binding(3), context, 7, Some(0), events.clone())
                .unwrap();
        assert_eq!(batch.events(), events);
        assert!(
            RetiringEditorBatch::new(binding(2), binding(2), context, 7, Some(0), events.clone())
                .is_err()
        );
        let mut other_root = binding(3);
        other_root.root = 8;
        assert!(
            RetiringEditorBatch::new(binding(2), other_root, context, 7, Some(0), events.clone())
                .is_err()
        );
        assert!(
            RetiringEditorBatch::new(binding(2), binding(3), context, 7, Some(0), vec![]).is_err()
        );
        let mut out_of_order = events.clone();
        out_of_order[2].sequence = 2;
        assert!(
            RetiringEditorBatch::new(binding(2), binding(3), context, 7, Some(0), out_of_order)
                .is_err()
        );
        let mut invalid = events;
        invalid[2].normalized = f64::NAN;
        assert!(
            RetiringEditorBatch::new(binding(2), binding(3), context, 7, Some(0), invalid).is_err()
        );
        let positive_zero = event(3, 2, Perform);
        let mut negative_zero = positive_zero;
        negative_zero.normalized = -0.0;
        let mut positive_zero = positive_zero;
        positive_zero.normalized = 0.0;
        assert_ne!(positive_zero, negative_zero);
        assert!(
            RetiringEditorBatch::new(
                binding(2),
                binding(3),
                context,
                7,
                Some(0),
                vec![event(1, 1, Begin); MAX_RETIRING_EDITOR_EVENTS + 1]
            )
            .is_err()
        );
    }
}
