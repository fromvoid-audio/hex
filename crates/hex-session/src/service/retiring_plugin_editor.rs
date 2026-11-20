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

impl RetiringEditorEventKind {
    fn is_group_marker(self) -> bool {
        matches!(self, Self::GroupBegin | Self::GroupEnd | Self::GroupAborted)
    }
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

/// Lifetime of a single close decision. The caller retains the strong owner
/// until close or cancellation. Queued requests and native worker custody
/// retain only weak references so they cannot keep a cancelled attempt alive.
#[derive(Debug, Default)]
pub struct PluginEditorCloseAttempt(std::sync::atomic::AtomicBool);

impl PluginEditorCloseAttempt {
    /// The native control owner invalidates the decision if any of its exact
    /// editor sources change before close. A new decision must get new holds.
    pub fn invalidate(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_valid(&self) -> bool {
        !self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// Native admission alone cannot prove that any accepted edits were saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEditorCloseAdmission {
    Pending,
    Held { admitted_edits: bool },
    Refused,
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

/// Whether the exact successor values were established. This can be a newly
/// published revision or a guarded capture matching the installed canonical
/// state. Receipt acknowledgement can fail after either confirmation.
pub enum RetiringEditorReplayDisposition {
    Refused,
    Confirmed { receipt_accepted: bool },
}

/// New successor input is allowed only for edits admitted after preparation.
/// Earlier input can be acknowledged after read-only successor verification.
#[derive(Clone, Copy, Debug)]
pub enum RetiringEditorReplayMode<'a> {
    VerifyExisting {
        points: &'a [(u32, f64)],
    },
    Apply {
        verify_existing: &'a [(u32, f64)],
        points: &'a [(u32, f64)],
        publication: AudioSourceContext,
    },
}

/// Why the original retiring editor source still needs the Session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetiringEditorCloseBlocker {
    Transferring,
    Unresolved,
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
            if event.sequence <= previous
                || event.gesture == 0
                || event.gesture > event.sequence
                || begins != (event.gesture == event.sequence)
                || event.parameter > i32::MAX as u32
                || (event.kind.is_group_marker() && event.parameter != 0)
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

struct ReplayPoints {
    earlier: Vec<(u32, f64)>,
    later: Vec<(u32, f64)>,
}

impl RetiringEditorBatch {
    fn replay_points(&self) -> Result<ReplayPoints, &'static str> {
        let cut = self
            .predecessor_admitted_through
            .ok_or("vst3 editor replacement order could not be established")?;
        use RetiringEditorEventKind::Perform;
        if self
            .events()
            .iter()
            .any(|event| event.kind.is_group_marker())
        {
            self.validate_grouped_replay(cut)?;
        }
        let mut plan = ReplayPoints {
            earlier: Vec::new(),
            later: Vec::new(),
        };
        for event in self.events().iter().filter(|event| event.kind == Perform) {
            let before_cut = event.sequence <= cut;
            let points = if before_cut {
                &mut plan.earlier
            } else {
                &mut plan.later
            };
            if let Some(point) = points.iter_mut().find(|point| point.0 == event.parameter) {
                if before_cut && point.1 != event.normalized {
                    return Err("earlier vst3 editor values need a separate ordered disposition");
                }
                point.1 = event.normalized;
            } else {
                points.push((event.parameter, event.normalized));
            }
        }
        if plan.earlier.is_empty() && plan.later.is_empty() {
            return Err("vst3 editor receipts have no replayable value");
        }
        Ok(plan)
    }

    /// Earlier complete cohorts are checked in place. Later values must fit
    /// one guarded SDK flush: ordinary values or one timestamp cohort.
    /// Every grouped sequence must close, and no cohort can straddle the cut.
    fn validate_grouped_replay(&self, cut: u64) -> Result<(), &'static str> {
        use RetiringEditorEventKind::{Begin, End, GroupBegin, GroupEnd, Perform};
        const UNRESOLVED: &str = "vst3 editor group needs a separate ordered disposition";
        let earlier_has_group = self
            .events()
            .iter()
            .take_while(|event| event.sequence <= cut)
            .any(|event| event.kind.is_group_marker());
        let mut cohort = None;
        // None means no later value yet; Some(None) means ordinary input.
        let mut later_value_cohort = None;
        let mut active = std::collections::BTreeMap::new();
        for event in self.events() {
            let earlier = event.sequence <= cut;
            if earlier && !earlier_has_group {
                continue;
            }
            if cohort.is_some_and(|(_, started_earlier)| started_earlier != earlier) {
                return Err(UNRESOLVED);
            }
            match event.kind {
                GroupBegin if cohort.is_none() => cohort = Some((event.gesture, earlier)),
                GroupEnd if cohort == Some((event.gesture, earlier)) => cohort = None,
                Begin => {
                    if active.insert(event.parameter, event.gesture).is_some() {
                        return Err(UNRESOLVED);
                    }
                }
                Perform if active.get(&event.parameter) == Some(&event.gesture) => {
                    let current = cohort.map(|(gesture, _)| gesture);
                    if !earlier && *later_value_cohort.get_or_insert(current) != current {
                        return Err(UNRESOLVED);
                    }
                }
                End if active.get(&event.parameter) == Some(&event.gesture) => {
                    active.remove(&event.parameter);
                }
                _ => return Err(UNRESOLVED),
            }
        }
        if cohort.is_some() || !active.is_empty() {
            return Err(UNRESOLVED);
        }
        Ok(())
    }
}

pub(super) struct ReplayOutcome {
    pub(super) prepared: Option<std::sync::Arc<hex_engine::PreparedRevision>>,
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
        if points
            .earlier
            .iter()
            .chain(&points.later)
            .any(|&(parameter, _)| {
                let target = hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter };
                original.automation_target(target).is_some()
            })
        {
            return Err("vst3 editor replay requires automation disposition".into());
        }
        let mode = if points.later.is_empty() {
            RetiringEditorReplayMode::VerifyExisting {
                points: &points.earlier,
            }
        } else {
            RetiringEditorReplayMode::Apply {
                verify_existing: &points.earlier,
                points: &points.later,
                publication: AudioSourceContext {
                    editor_revision: context
                        .editor_revision
                        .checked_add(1)
                        .ok_or("vst3 editor replay revision exhausted")?,
                    ..context
                },
            }
        };
        let mut capture = sink.capture_retiring_editor_replay(&batch, mode)?;
        capture.before =
            super::resident_preview::plugin_parameter::bind_capture(&state, capture.before)
                .map_err(|error| error.to_string())?;
        capture.after =
            super::resident_preview::plugin_parameter::bind_capture(&state, capture.after)
                .map_err(|error| error.to_string())?;
        let prepared = match mode {
            RetiringEditorReplayMode::Apply { publication, .. }
                if capture.before != capture.after =>
            {
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
                Some(prepared)
            }
            _ => {
                // Confirmation settles the receipt only when both exact
                // captures agree with the canonical opaque state.
                if capture.before != state || capture.after != state {
                    return Err("vst3 replay confirmation differs from canonical state".into());
                }
                None
            }
        };
        // Canonical values are confirmed. An acknowledgement failure retains
        // the receipt and cannot undo an already-published revision.
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
                RetiringEditorReplayDisposition::Confirmed {
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
    fn one_complete_value_cohort_can_replay_but_ambiguous_groups_stay_in_custody() {
        use RetiringEditorEventKind::{Begin, End, GroupAborted, GroupBegin, GroupEnd, Perform};

        let device = 7;
        let context = AudioSourceContext {
            project_generation: 3,
            editor_revision: 4,
        };
        let project_values = |events: Vec<RetiringEditorEvent>, cut| {
            RetiringEditorBatch::new(binding(2), binding(3), context, device, Some(cut), events)
                .unwrap()
                .replay_points()
                .map(|points| (points.earlier, points.later))
        };
        let complete = vec![
            event(1, 1, GroupBegin),
            event(2, 2, Begin),
            event(3, 2, Perform),
            event(4, 2, End),
            event(5, 1, GroupEnd),
        ];
        assert_eq!(
            project_values(complete.clone(), 0),
            Ok((vec![], vec![(0, 0.75)]))
        );
        assert_eq!(
            project_values(complete.clone(), 5),
            Ok((vec![(0, 0.75)], vec![]))
        );

        let mut aborted = complete.clone();
        aborted[4].kind = GroupAborted;
        assert!(project_values(aborted, 0).is_err());
        let mut still_held = complete.clone();
        still_held.remove(3);
        assert!(project_values(still_held, 0).is_err());
        let mut wrong_cohort = complete.clone();
        wrong_cohort[4].gesture = 2;
        assert!(project_values(wrong_cohort, 0).is_err());
        assert!(project_values(complete.clone(), 1).is_err());

        let mut several_values = complete;
        several_values.extend([
            event(6, 6, GroupBegin),
            event(7, 7, Begin),
            event(8, 7, Perform),
            event(9, 7, End),
            event(10, 6, GroupEnd),
        ]);
        assert!(project_values(several_values.clone(), 0).is_err());
        assert_eq!(
            project_values(several_values, 5),
            Ok((vec![(0, 0.75)], vec![(0, 0.75)]))
        );
    }

    #[test]
    fn ordinary_values_before_the_cut_can_only_be_verified_as_existing() {
        use RetiringEditorEventKind::{Begin, End, Perform};

        let context = AudioSourceContext {
            project_generation: 3,
            editor_revision: 4,
        };
        let plan = |events: Vec<RetiringEditorEvent>, cut| {
            RetiringEditorBatch::new(binding(2), binding(3), context, 7, Some(cut), events)
                .unwrap()
                .replay_points()
        };
        let mut events = vec![
            event(1, 1, Begin),
            event(2, 1, Perform),
            event(3, 1, Perform),
            event(4, 1, End),
        ];
        let existing = plan(events.clone(), 4).unwrap();
        assert!(existing.later.is_empty());
        assert_eq!(existing.earlier, vec![(0, 0.75)]);

        events[1].normalized = 0.5;
        assert!(plan(events.clone(), 4).is_err());
        let mixed = plan(events.clone(), 2).unwrap();
        assert_eq!(mixed.earlier, vec![(0, 0.5)]);
        assert_eq!(mixed.later, vec![(0, 0.75)]);
        let replay = plan(events, 0).unwrap();
        assert!(replay.earlier.is_empty());
        assert_eq!(replay.later, vec![(0, 0.75)]);
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
