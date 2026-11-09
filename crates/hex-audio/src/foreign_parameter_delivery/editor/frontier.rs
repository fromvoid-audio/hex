//! Exact direct Main confirmation for a single passive editor scope. The
//! physical endpoint supplies bounded, accepted PCM quanta; this owner neither
//! applies values nor consumes the ordered SDK controller reader.
use super::AbortReason;
use crate::prepared::AppliedAudioSource;
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use hex_plugin_host::transport::AcceptedEditorQuantum;
use std::{collections::VecDeque, num::NonZeroU64};

/// The normal 256-frame foreign quantum fits the full 65,536-frame delay
/// in this queue. A smaller quantum or unserviced reader may exhaust credit;
/// refuse explicitly and keep the already confirmed prefix.
pub const MAX_EDITOR_COMPENSATED_INPUT_QUANTA: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub source: AppliedAudioSource,
    pub binding: Binding,
    pub transport_epoch: u64,
    pub scope: NonZeroU64,
    pub quantum_frames: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfirmedQuantum {
    pub target: Target,
    pub input: AcceptedEditorQuantum,
    /// First affected sample on the final Main performance clock.
    pub main_performance_frame: u64,
}

struct Pending {
    input: AcceptedEditorQuantum,
    due: u64,
}

/// Control allocates this ring's backing before exposure. Push at most one
/// quantum per physical callback; no callback growth, sorting or scanning.
pub(super) struct Frontier {
    pending: VecDeque<Pending>,
    input_sequence: Option<u64>,
    end: Option<u64>,
}

impl Frontier {
    pub(super) fn new() -> Self {
        Self {
            pending: VecDeque::with_capacity(MAX_EDITOR_COMPENSATED_INPUT_QUANTA),
            input_sequence: None,
            end: None,
        }
    }

    pub(super) fn heap_bytes(&self) -> Option<usize> {
        self.pending
            .capacity()
            .checked_mul(std::mem::size_of::<Pending>())
    }

    pub(super) fn storage_quote() -> Option<usize> {
        MAX_EDITOR_COMPENSATED_INPUT_QUANTA.checked_mul(std::mem::size_of::<Pending>())
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.input_sequence = None;
        self.end = None;
    }

    pub(super) fn finish(
        &mut self,
        target: Target,
        marker: Option<AcceptedEditorQuantum>,
        end: u64,
        main_latency: u32,
    ) -> Result<Option<ConfirmedQuantum>, AbortReason> {
        let quantum = u64::from(target.quantum_frames);
        let start = self
            .end
            .replace(end)
            .unwrap_or_else(|| end.saturating_sub(quantum));
        if end.checked_sub(start).is_none_or(|frames| frames > quantum) {
            return Err(AbortReason::TransportChanged);
        }
        if let Some(input) = marker
            && input.binding == target.binding
            && input.scope == target.scope
        {
            if input.sequence.checked_mul(quantum) != Some(input.first_frame)
                || u64::from(input.callback_offset) >= quantum
                || self
                    .input_sequence
                    .is_some_and(|previous| previous.checked_add(1) != Some(input.sequence))
            {
                return Err(AbortReason::MissedOperation);
            }
            self.input_sequence = Some(input.sequence);
            if input.lost_origin {
                return Err(AbortReason::MissingOrigin);
            }
            if let Some(origin) = input.origin {
                let due = origin
                    .performance_frame
                    .checked_add(u64::from(main_latency))
                    .ok_or(AbortReason::ClockExhausted)?;
                if due < start
                    || self
                        .pending
                        .back()
                        .is_some_and(|pending| due.saturating_sub(pending.due) < quantum)
                {
                    return Err(AbortReason::MissedOperation);
                }
                // A due predecessor releases one credit before this append.
                // Do not otherwise extend an allocation from the callback.
                if self.pending.len() == MAX_EDITOR_COMPENSATED_INPUT_QUANTA
                    && self
                        .pending
                        .front()
                        .is_none_or(|pending| pending.due >= end)
                {
                    return Err(AbortReason::Saturated);
                }
                if self.pending.len() < MAX_EDITOR_COMPENSATED_INPUT_QUANTA {
                    self.pending.push_back(Pending { input, due });
                } else {
                    let confirmed = self.confirm(target, start, end)?;
                    self.pending.push_back(Pending { input, due });
                    return Ok(confirmed);
                }
            } else if !input.performs.as_slice().is_empty()
                || !input.restorations.as_slice().is_empty()
            {
                return Err(AbortReason::MissingOrigin);
            }
        }
        self.confirm(target, start, end)
    }

    fn confirm(
        &mut self,
        target: Target,
        start: u64,
        end: u64,
    ) -> Result<Option<ConfirmedQuantum>, AbortReason> {
        if self
            .pending
            .front()
            .is_none_or(|pending| pending.due >= end)
        {
            return Ok(None);
        }
        let pending = self
            .pending
            .pop_front()
            .ok_or(AbortReason::MissedOperation)?;
        if pending.due < start {
            return Err(AbortReason::MissedOperation);
        }
        Ok(Some(ConfirmedQuantum {
            target,
            input: pending.input,
            main_performance_frame: pending.due,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_host::transport::ManualParameterOrigin;

    fn target() -> Target {
        Target {
            source: AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 2,
                },
                audio_revision: 3,
                output_identity: 4,
            },
            binding: Binding {
                root: 4,
                instance: 5,
                graph: 6,
                epoch: 7,
            },
            transport_epoch: 9,
            scope: NonZeroU64::MIN,
            quantum_frames: 8,
        }
    }

    fn marker(target: Target, sequence: u64) -> AcceptedEditorQuantum {
        AcceptedEditorQuantum {
            binding: target.binding,
            scope: target.scope,
            sequence,
            first_frame: sequence * u64::from(target.quantum_frames),
            callback_offset: 2,
            origin: Some(ManualParameterOrigin {
                performance_frame: sequence * 8,
                arrangement_frame: 80,
                loop_pass: sequence,
            }),
            performs: Default::default(),
            restorations: Default::default(),
            lost_origin: false,
        }
    }

    #[test]
    fn compensated_confirmation_preserves_loop_origin_and_exclusive_boundary() {
        let mut frontier = Frontier::new();
        let target = target();
        let one = marker(target, 1);
        let two = marker(target, 2);
        assert_eq!(frontier.finish(target, Some(one), 24, 29), Ok(None));
        assert_eq!(frontier.finish(target, None, 28, 29), Ok(None));
        assert_eq!(frontier.finish(target, Some(two), 32, 29), Ok(None));
        assert_eq!(frontier.finish(target, None, 37, 29), Ok(None));
        assert_eq!(
            frontier.finish(target, None, 38, 29),
            Ok(Some(ConfirmedQuantum {
                target,
                input: one,
                main_performance_frame: 37,
            }))
        );
        assert_eq!(
            frontier.finish(target, None, 46, 29),
            Ok(Some(ConfirmedQuantum {
                target,
                input: two,
                main_performance_frame: 45,
            }))
        );
    }

    #[test]
    fn a_gap_or_old_scope_does_not_invent_main_delivery() {
        let mut frontier = Frontier::new();
        let target = target();
        let mut stale = marker(target, 0);
        stale.scope = NonZeroU64::new(2).unwrap();
        assert_eq!(frontier.finish(target, Some(stale), 19, 24), Ok(None));
        assert_eq!(
            frontier.finish(target, Some(marker(target, 1)), 27, 24),
            Ok(None)
        );
        assert_eq!(
            frontier.finish(target, Some(marker(target, 3)), 35, 24),
            Err(AbortReason::MissedOperation)
        );
        frontier.clear();
        assert_eq!(frontier.finish(target, None, 35, 24), Ok(None));
    }

    #[test]
    fn shorter_quanta_refuse_when_the_fixed_compensation_queue_is_full() {
        let mut frontier = Frontier::new();
        let target = target();
        let capacity = frontier.pending.capacity();
        for sequence in 0..MAX_EDITOR_COMPENSATED_INPUT_QUANTA as u64 {
            assert_eq!(
                frontier.finish(
                    target,
                    Some(marker(target, sequence)),
                    19 + sequence * 8,
                    4096
                ),
                Ok(None)
            );
        }
        let next = MAX_EDITOR_COMPENSATED_INPUT_QUANTA as u64;
        assert_eq!(
            frontier.finish(target, Some(marker(target, next)), 19 + next * 8, 4096),
            Err(AbortReason::Saturated)
        );
        assert_eq!(frontier.pending.capacity(), capacity);
    }
}
