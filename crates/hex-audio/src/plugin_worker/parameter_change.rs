//! One host-originated parameter input on the original accepted PCM stream.
//! Fixed request/result storage belongs to the original completion allocation;
//! no SDK callback, invented process block, reader or source owner is added.
use super::*;
use std::sync::{TryLockError, Weak};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Queued,
    InFlight,
    Complete,
}
#[derive(Clone, Copy, Debug)]
enum Terminal {
    Processed(ProcessedParameterChange),
    Refused,
    Withdrawn,
}
#[derive(Default)]
pub(super) struct Slot {
    ordinal: u64,
    phase: Phase,
    point: ParameterPoint,
    result: Option<Terminal>,
}
#[derive(Default)]
pub(super) struct Work {
    ordinal: Option<u64>,
    point: ParameterPoint,
    input: Option<(u64, u64)>,
    result: Option<Terminal>,
}

/// Exact original worker and operation; neither a source/endpoint lease nor
/// canonical edit authority. Retain at most the current operation's weak token.
pub struct NativeWorkerParameterChange {
    source: Weak<Signal>,
    binding: Binding,
    ordinal: u64,
}
/// The requested point travelled in this successful SDK input/reply exchange.
/// This is not controller readback, final host delivery, captured state or a
/// canonical acknowledgement. Plugin output may supersede the requested value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcessedParameterChange {
    pub point: ParameterPoint,
    pub sequence: u64,
    pub first_frame: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeWorkerParameterProgress {
    Pending,
    Processed(ProcessedParameterChange),
    /// The accepted input already contains this ID, or its quantum/physical
    /// callback budget has no remaining slot.
    /// Its authored points and PCM are preserved; no host point was injected.
    Refused,
    /// Withdrawn while queued, before any native input could consume it.
    Withdrawn,
    Failed,
}
impl NativeWorker {
    /// Queue one point at offset zero of the next actual accepted PCM quantum.
    /// Admission does no SDK/socket I/O; invalid/full/busy refusal preserves the
    /// prior request and source. The caller must first validate current SDK
    /// metadata/writability. Native SDK validation remains authoritative.
    /// Stopped owners use the existing actual quiescent flush contract instead;
    /// this operation never manufactures PCM or revives a stopped transport.
    pub fn begin_parameter_change_on_control(
        &mut self,
        id: u32,
        normalized: f64,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        let point = ParameterPoint {
            frame: 0,
            id,
            normalized,
        };
        Parameters::default()
            .push(point)
            .map_err(|_| "invalid native parameter input")?;
        if !self.transport_is_live_on_control() || !self.retains_original_source_on_control() {
            return Err("native parameter source is not live");
        }
        let mut mailbox = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native parameter mailbox is busy")?;
        let slot = &mut mailbox.parameter_change;
        if slot.phase != Phase::Idle {
            return Err("native parameter input is already pending");
        }
        let ordinal = slot
            .ordinal
            .checked_add(1)
            .ok_or("native parameter input ordinal exhausted")?;
        slot.ordinal = ordinal;
        slot.point = point;
        slot.phase = Phase::Queued;
        slot.result = None;
        self.reaper
            .completed
            .parameter_change_requested
            .store(true, Release);
        Ok(NativeWorkerParameterChange {
            source: Arc::downgrade(&self.reaper.completed),
            binding: self.binding,
            ordinal,
        })
    }
    /// Withdraw only an exact queued operation. False means it already crossed
    /// into processing or has a terminal to consume; callers must observe that
    /// actual result, not pretend an admitted mutation was cancelled.
    pub fn withdraw_parameter_change_on_control(
        &mut self,
        request: &NativeWorkerParameterChange,
    ) -> Result<bool, &'static str> {
        self.require_parameter_change_source(request)?;
        let mut mailbox = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native parameter mailbox is busy")?;
        let slot = &mut mailbox.parameter_change;
        if !slot.matches(request) {
            return Err("native parameter input operation differs");
        }
        if !self.transport_is_live_on_control() {
            return Ok(false);
        }
        if slot.phase != Phase::Queued {
            return Ok(false);
        }
        slot.phase = Phase::Complete;
        slot.result = Some(Terminal::Withdrawn);
        self.reaper
            .completed
            .parameter_change_requested
            .store(false, Release);
        Ok(true)
    }
    /// Consume the exact result once. Revocation wins over a late processed
    /// result, and foreign/old tokens cannot consume another request's result.
    pub fn poll_parameter_change_on_control(
        &mut self,
        request: &NativeWorkerParameterChange,
    ) -> Result<NativeWorkerParameterProgress, &'static str> {
        self.require_parameter_change_source(request)?;
        let mut mailbox = match self.reaper.completed.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(NativeWorkerParameterProgress::Pending),
            Err(TryLockError::Poisoned(_)) => return Err("native parameter mailbox failed"),
        };
        let slot = &mut mailbox.parameter_change;
        if !slot.matches(request) {
            return Err("native parameter input operation differs");
        }
        if !self.transport_is_live_on_control() {
            return Ok(NativeWorkerParameterProgress::Failed);
        }
        if slot.phase != Phase::Complete {
            return Ok(NativeWorkerParameterProgress::Pending);
        }
        let result = slot
            .result
            .ok_or("native parameter input terminal missing")?;
        slot.phase = Phase::Idle;
        slot.result = None;
        Ok(match result {
            Terminal::Processed(value) => NativeWorkerParameterProgress::Processed(value),
            Terminal::Refused => NativeWorkerParameterProgress::Refused,
            Terminal::Withdrawn => NativeWorkerParameterProgress::Withdrawn,
        })
    }
    fn require_parameter_change_source(
        &self,
        request: &NativeWorkerParameterChange,
    ) -> Result<(), &'static str> {
        if request.binding != self.binding
            || request.source.as_ptr() != Arc::as_ptr(&self.reaper.completed)
        {
            return Err("native parameter input source differs");
        }
        Ok(())
    }
}
impl Slot {
    fn matches(&self, request: &NativeWorkerParameterChange) -> bool {
        self.phase != Phase::Idle && self.ordinal == request.ordinal
    }
}
fn merge_parameter_input(
    authored: &Parameters,
    point: ParameterPoint,
    parameter_capacity: usize,
) -> Result<Option<Parameters>, Fault> {
    if parameter_capacity == 0
        || authored.as_slice().len() == MAX_PARAMETER_POINTS
        || authored
            .as_slice()
            .iter()
            .any(|existing| existing.id == point.id)
    {
        return Ok(None);
    }
    let mut merged = Parameters::default();
    merged.push(point)?;
    for existing in authored.as_slice() {
        merged.push(*existing)?;
    }
    Ok(Some(merged))
}
impl State {
    pub(super) fn stage_parameter_change(
        &mut self,
        signal: &Signal,
        block: &mut Block,
        parameter_capacity: usize,
    ) -> Result<(), Fault> {
        if self.parameter_change.ordinal.is_some()
            || !signal.parameter_change_requested.load(Acquire)
        {
            return Ok(());
        }
        let mut mailbox = match signal.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(_)) => return Err(Fault::Protocol),
        };
        let slot = &mut mailbox.parameter_change;
        // Withdrawal may win after the worker's atomic observation and before
        // this lock. It must not become a source fault or a late mutation.
        if slot.phase != Phase::Queued {
            return Ok(());
        }
        self.parameter_change.ordinal = Some(slot.ordinal);
        self.parameter_change.point = slot.point;
        slot.phase = Phase::InFlight;
        signal.parameter_change_requested.store(false, Release);
        drop(mailbox);
        let point = self.parameter_change.point;
        let Some(merged) = merge_parameter_input(&block.parameters, point, parameter_capacity)?
        else {
            self.parameter_change.result = Some(Terminal::Refused);
            return Ok(());
        };
        block.parameters = merged;
        self.parameter_change.input = Some((block.sequence, block.first_frame));
        Ok(())
    }
    pub(super) fn complete_parameter_change(&mut self, reply: &Block) -> Result<(), Fault> {
        if let Some((sequence, first_frame)) = self.parameter_change.input {
            if sequence != reply.sequence || first_frame != reply.first_frame {
                return Err(Fault::Protocol);
            }
            self.parameter_change.result = Some(Terminal::Processed(ProcessedParameterChange {
                point: self.parameter_change.point,
                sequence,
                first_frame,
            }));
            self.parameter_change.input = None;
        }
        Ok(())
    }
    pub(super) fn publish_parameter_change(&mut self, signal: &Signal) -> Result<(), Fault> {
        let Some(result) = self.parameter_change.result else {
            return Ok(());
        };
        let mut mailbox = match signal.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(_)) => return Err(Fault::Protocol),
        };
        let slot = &mut mailbox.parameter_change;
        if Some(slot.ordinal) != self.parameter_change.ordinal || slot.phase != Phase::InFlight {
            return Err(Fault::Protocol);
        }
        slot.result = Some(result);
        slot.phase = Phase::Complete;
        self.parameter_change = Work::default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_merge_preserves_authored_order_and_refuses_conflict_or_capacity_without_overwrite() {
        let host = ParameterPoint {
            frame: 0,
            id: 0,
            normalized: 0.75,
        };
        let mut authored = Parameters::default();
        for (frame, id) in [(0, 7), (3, 8), (7, 7)] {
            authored
                .push(ParameterPoint {
                    frame,
                    id,
                    normalized: 0.25,
                })
                .unwrap();
        }
        let before = authored;
        let merged = merge_parameter_input(&authored, host, 64).unwrap().unwrap();
        assert_eq!(
            merged.as_slice(),
            &[
                host,
                before.as_slice()[0],
                before.as_slice()[1],
                before.as_slice()[2]
            ]
        );
        assert_eq!(authored, before);
        assert_eq!(
            merge_parameter_input(&authored, ParameterPoint { id: 7, ..host }, 64).unwrap(),
            None
        );
        let mut full = Parameters::default();
        for id in 1..=64 {
            full.push(ParameterPoint {
                frame: 0,
                id,
                normalized: 0.25,
            })
            .unwrap();
        }
        let before = full;
        assert_eq!(merge_parameter_input(&full, host, 64).unwrap(), None);
        assert_eq!(full, before);
        let mut almost_full = Parameters::default();
        for id in 1..=63 {
            almost_full
                .push(ParameterPoint {
                    frame: 0,
                    id,
                    normalized: 0.25,
                })
                .unwrap();
        }
        assert_eq!(
            merge_parameter_input(&almost_full, host, 0).unwrap(),
            None,
            "completed quantum's free slot does not override an exhausted physical callback"
        );
        assert_eq!(
            merge_parameter_input(&almost_full, host, 1)
                .unwrap()
                .unwrap()
                .as_slice()
                .len(),
            64
        );
    }
}
