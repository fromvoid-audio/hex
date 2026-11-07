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
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Intent {
    #[default]
    Point,
    Idle,
    Hold,
    Update(u64),
    Release(u64),
}

#[derive(Clone, Copy)]
struct Hold {
    ordinal: u64,
    id: u32,
}

#[derive(Clone, Copy, Debug)]
enum Terminal {
    Processed(ProcessedParameterChange),
    Flushed,
    Released { sequence: u64, first_frame: u64 },
    Refused,
    Withdrawn,
}
#[derive(Default)]
pub(super) struct Slot {
    ordinal: u64,
    phase: Phase,
    point: ParameterPoint,
    intent: Intent,
    require_arrangement_origin: bool,
    result: Option<Terminal>,
}
#[derive(Default)]
pub(super) struct Work {
    ordinal: Option<u64>,
    point: ParameterPoint,
    input: Option<(u64, u64)>,
    hold: Option<Hold>,
    releasing: Option<u32>,
    result: Option<Terminal>,
}

/// Exact original worker and operation; neither a source/endpoint lease nor
/// canonical edit authority. Retain the one current token until its exact
/// terminal, including after Stop. If custody is lost, retire the original
/// source; never rebind it as though the input had been consumed.
pub struct NativeWorkerParameterChange {
    source: Weak<Signal>,
    binding: Binding,
    ordinal: u64,
    parameter: u32,
    pcm_delivery: bool,
    hold_id: Option<u32>,
}
impl NativeWorkerParameterChange {
    pub(crate) fn delivery_identity(&self) -> Option<(Binding, u64, u32)> {
        self.pcm_delivery
            .then_some((self.binding, self.ordinal, self.parameter))
    }
}
#[cfg(test)]
impl NativeWorkerParameterChange {
    pub(crate) fn original_worker_released_on_control(&self) -> bool {
        self.source.upgrade().is_none()
    }
}

/// A live hold command always refers to the exact original worker. The
/// opening operation token doubles as the hold identity after SDK acceptance.
#[derive(Clone, Copy)]
pub enum NativeParameterHold<'a> {
    Begin {
        id: u32,
        normalized: f64,
    },
    Change {
        hold: &'a NativeWorkerParameterChange,
        normalized: f64,
    },
    Release {
        hold: &'a NativeWorkerParameterChange,
    },
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
    /// A zero-frame exchange acknowledged by the original processor and controller.
    Flushed,
    /// The hold ended before this SDK quantum. Any applicable authored points
    /// for the previously held target can now reach the processor.
    Released {
        sequence: u64,
        first_frame: u64,
    },
    /// The accepted input already contains this ID, or its quantum/physical
    /// callback budget has no remaining slot.
    /// Its authored points and PCM are preserved; no host point was injected.
    Refused,
    /// Withdrawn while queued, before any native input could consume it.
    Withdrawn,
    Failed,
}
impl NativeWorker {
    /// Drain the exact admitted input after its original worker has stopped.
    /// A queued idle operation is flushed by joining control on the healthy
    /// original source. Other queued live operations remain withdrawn. No
    /// callback, PCM prefix or transport is restarted by this control I/O.
    /// This reports SDK processing only, not delivery or canonical acceptance.
    pub fn drain_stopped_parameter_change_on_control(
        &mut self,
        request: &NativeWorkerParameterChange,
    ) -> Result<NativeWorkerParameterProgress, &'static str> {
        self.require_parameter_change_source(request)?;
        if self.retirement_requested
            || self.transport_is_live_on_control()
            || !self.retains_original_source_on_control()
        {
            return Err("native stopped parameter source differs");
        }
        {
            let mailbox = match self.reaper.completed.capture.try_lock() {
                Ok(mailbox) => mailbox,
                Err(TryLockError::WouldBlock) => return Ok(NativeWorkerParameterProgress::Pending),
                Err(TryLockError::Poisoned(_)) => return Err("native parameter mailbox failed"),
            };
            if !mailbox.parameter_change.matches(request) {
                return Err("native parameter input operation differs");
            }
        }
        if self
            .reaper
            .join
            .as_ref()
            .is_some_and(|join| !join.is_finished())
        {
            return Ok(NativeWorkerParameterProgress::Pending);
        }
        let joined = self.reaper.join_on_control();
        let state = self
            .reaper
            .worker
            .as_mut()
            .ok_or("native parameter owner missing")?;
        if joined.is_ok()
            && state.native_error.is_none()
            && let Err(error) = state.advance_idle_parameter_change(&self.reaper.completed)
        {
            state.native_error = Some(error);
        }
        let healthy = joined.is_ok()
            && state.native_error.is_none()
            && matches!(state.processor, Some(Processor::Native(_)));
        let mut mailbox = match self.reaper.completed.capture.try_lock() {
            Ok(mailbox) => mailbox,
            Err(TryLockError::WouldBlock) => return Ok(NativeWorkerParameterProgress::Pending),
            Err(TryLockError::Poisoned(_)) => return Err("native parameter mailbox failed"),
        };
        let slot = &mut mailbox.parameter_change;
        if !slot.matches(request) {
            return Err("native parameter input operation differs");
        }
        let terminal = if !healthy {
            None
        } else {
            match slot.phase {
                Phase::Queued if slot.intent == Intent::Idle => {
                    return Ok(NativeWorkerParameterProgress::Pending);
                }
                Phase::Queued => Some(Terminal::Withdrawn),
                Phase::Complete => slot.result,
                Phase::InFlight if state.parameter_change.ordinal == Some(request.ordinal) => {
                    state.parameter_change.result
                }
                _ => None,
            }
        };
        if healthy && terminal.is_none() {
            // An unexplained missing receipt is not a terminal. Keep the
            // original owner and fence intact for explicit fault recovery.
            return Err("native parameter input terminal missing");
        }
        slot.phase = Phase::Idle;
        slot.result = None;
        if state.parameter_change.ordinal == Some(request.ordinal) {
            state.parameter_change = Work::default();
        }
        self.reaper
            .completed
            .parameter_change_requested
            .store(false, Release);
        Ok(match terminal {
            Some(Terminal::Processed(value)) => NativeWorkerParameterProgress::Processed(value),
            Some(Terminal::Flushed) => NativeWorkerParameterProgress::Flushed,
            Some(Terminal::Released {
                sequence,
                first_frame,
            }) => NativeWorkerParameterProgress::Released {
                sequence,
                first_frame,
            },
            Some(Terminal::Refused) => NativeWorkerParameterProgress::Refused,
            Some(Terminal::Withdrawn) => NativeWorkerParameterProgress::Withdrawn,
            None => NativeWorkerParameterProgress::Failed,
        })
    }

    pub(crate) fn parameter_change_pending_on_control(&self) -> Result<bool, &'static str> {
        let mailbox = match self.reaper.completed.capture.try_lock() {
            Ok(mailbox) => mailbox,
            Err(TryLockError::WouldBlock) => return Err("native parameter mailbox is busy"),
            Err(TryLockError::Poisoned(_)) => return Err("native parameter mailbox failed"),
        };
        Ok(mailbox.parameter_change.phase != Phase::Idle)
    }
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
        self.begin_parameter_change(id, normalized, false)
    }

    pub(crate) fn begin_parameter_change(
        &mut self,
        id: u32,
        normalized: f64,
        idle: bool,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        if idle && !self.transport_is_live_on_control() {
            self.prepare_stopped_capture()?;
        }
        self.queue_parameter_change(
            id,
            normalized,
            if idle { Intent::Idle } else { Intent::Point },
        )
    }

    /// Begin one live manual hold on a target in this worker's authored program.
    /// Keep this operation token after observing its successful terminal. It
    /// also names the hold for all updates and for the explicit release. A
    /// caller must release a successful hold or retire its original source.
    pub fn hold_parameter_on_control(
        &mut self,
        id: u32,
        normalized: f64,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.parameter_hold_on_control(NativeParameterHold::Begin { id, normalized })
    }

    pub fn update_held_parameter_on_control(
        &mut self,
        hold: &NativeWorkerParameterChange,
        normalized: f64,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.parameter_hold_on_control(NativeParameterHold::Change { hold, normalized })
    }

    pub fn release_held_parameter_on_control(
        &mut self,
        hold: &NativeWorkerParameterChange,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.parameter_hold_on_control(NativeParameterHold::Release { hold })
    }

    pub(crate) fn parameter_hold_on_control(
        &mut self,
        command: NativeParameterHold<'_>,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.parameter_hold_with_before(command, false, |_| Ok(()))
            .map(|(request, ())| request)
    }

    pub(crate) fn parameter_hold_with_before<T>(
        &mut self,
        command: NativeParameterHold<'_>,
        require_arrangement_origin: bool,
        before_publish: impl FnOnce(&NativeWorkerParameterChange) -> Result<T, &'static str>,
    ) -> Result<(NativeWorkerParameterChange, T), &'static str> {
        let (id, normalized, intent) = match command {
            NativeParameterHold::Begin { id, normalized } => (id, normalized, Intent::Hold),
            NativeParameterHold::Change { hold, normalized } => {
                self.require_parameter_change_source(hold)?;
                let id = hold
                    .hold_id
                    .ok_or("native parameter token does not own a hold")?;
                (id, normalized, Intent::Update(hold.ordinal))
            }
            NativeParameterHold::Release { hold } => {
                self.require_parameter_change_source(hold)?;
                let id = hold
                    .hold_id
                    .ok_or("native parameter token does not own a hold")?;
                (id, 0.0, Intent::Release(hold.ordinal))
            }
        };
        self.queue_parameter_change_with_before(
            id,
            normalized,
            intent,
            require_arrangement_origin,
            before_publish,
        )
    }

    pub(crate) fn parameter_change_with_before<T>(
        &mut self,
        id: u32,
        normalized: f64,
        require_arrangement_origin: bool,
        before_publish: impl FnOnce(&NativeWorkerParameterChange) -> Result<T, &'static str>,
    ) -> Result<(NativeWorkerParameterChange, T), &'static str> {
        self.queue_parameter_change_with_before(
            id,
            normalized,
            Intent::Point,
            require_arrangement_origin,
            before_publish,
        )
    }

    fn queue_parameter_change(
        &mut self,
        id: u32,
        normalized: f64,
        intent: Intent,
    ) -> Result<NativeWorkerParameterChange, &'static str> {
        self.queue_parameter_change_with_before(id, normalized, intent, false, |_| Ok(()))
            .map(|(request, ())| request)
    }

    /// Reserve copied control data while the exact request is still private.
    /// Refusal leaves the slot/ordinal intact. After this hook succeeds, queue
    /// publication cannot fail and carries both handles through late close.
    /// Origin-required input retains the queued slot until an accepted PCM
    /// request has a real compensated Arrangement origin.
    fn queue_parameter_change_with_before<T>(
        &mut self,
        id: u32,
        normalized: f64,
        intent: Intent,
        require_arrangement_origin: bool,
        before_publish: impl FnOnce(&NativeWorkerParameterChange) -> Result<T, &'static str>,
    ) -> Result<(NativeWorkerParameterChange, T), &'static str> {
        let point = ParameterPoint {
            frame: 0,
            id,
            normalized,
        };
        Parameters::default()
            .push(point)
            .map_err(|_| "invalid native parameter input")?;
        if self.retirement_requested
            || !self.retains_original_source_on_control()
            || (!self.transport_is_live_on_control()
                && (intent != Intent::Idle
                    || self.reaper.join.is_some()
                    || self.reaper.worker.is_none()))
        {
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
        let request = NativeWorkerParameterChange {
            source: Arc::downgrade(&self.reaper.completed),
            binding: self.binding,
            ordinal,
            parameter: id,
            pcm_delivery: matches!(
                intent,
                Intent::Point | Intent::Hold | Intent::Update(_) | Intent::Release(_)
            ),
            hold_id: (intent == Intent::Hold).then_some(id),
        };
        let reserved = before_publish(&request)?;
        slot.ordinal = ordinal;
        slot.point = point;
        slot.intent = intent;
        slot.require_arrangement_origin = require_arrangement_origin;
        slot.phase = Phase::Queued;
        slot.result = None;
        self.reaper
            .completed
            .parameter_change_requested
            .store(true, Release);
        Ok((request, reserved))
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
        if !self.transport_is_live_on_control() && slot.intent != Intent::Idle {
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
    /// Consume the exact result once. Revocation returns Failed without consuming
    /// the token; its owner must then drain the stopped input before quiescence.
    /// Foreign/old tokens cannot consume another request's result.
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
            Terminal::Flushed => NativeWorkerParameterProgress::Flushed,
            Terminal::Released {
                sequence,
                first_frame,
            } => NativeWorkerParameterProgress::Released {
                sequence,
                first_frame,
            },
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
fn without_parameter(points: &Parameters, id: u32) -> Result<Parameters, Fault> {
    let mut filtered = Parameters::default();
    for point in points.as_slice().iter().filter(|point| point.id != id) {
        filtered.push(*point)?;
    }
    Ok(filtered)
}

impl State {
    /// Serialized with PCM on the native worker, or called by joining control
    /// after its actual terminal. Idle points use the same exact mailbox/token
    /// and never enter a PCM block. No mailbox guard spans native I/O.
    pub(super) fn advance_idle_parameter_change(
        &mut self,
        signal: &Signal,
    ) -> Result<(), hex_plugin_host::Error> {
        use hex_plugin_host::Error;
        if self.parameter_change.ordinal.is_some()
            || !signal.parameter_change_requested.load(Acquire)
        {
            return Ok(());
        }
        let (ordinal, point) = {
            let mut mailbox = match signal.capture.try_lock() {
                Ok(slot) => slot,
                Err(TryLockError::WouldBlock) => return Ok(()),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(Error::Protocol("native parameter mailbox failed"));
                }
            };
            let slot = &mut mailbox.parameter_change;
            if slot.phase != Phase::Queued || slot.intent != Intent::Idle {
                return Ok(());
            }
            slot.phase = Phase::InFlight;
            signal.parameter_change_requested.store(false, Release);
            (slot.ordinal, slot.point)
        };
        let Some(Processor::Native(instance)) = self.processor.as_mut() else {
            return Err(Error::Protocol("native parameter source missing"));
        };
        let mut points = Parameters::default();
        points
            .push(point)
            .map_err(|_| Error::Protocol("invalid native parameter input"))?;
        instance.flush_idle_parameters_on_worker(&points)?;
        self.parameter_change.ordinal = Some(ordinal);
        self.parameter_change.result = Some(Terminal::Flushed);
        Ok(())
    }
    pub(super) fn stage_parameter_change(
        &mut self,
        signal: &Signal,
        block: &mut Block,
        parameter_capacity: usize,
        authored: bool,
        manual_origin_available: bool,
    ) -> Result<Option<ParameterPoint>, Fault> {
        let work = &mut self.parameter_change;
        let pending = work.ordinal.is_none() && signal.parameter_change_requested.load(Acquire);
        if !pending && (!authored || work.hold.is_none()) {
            return Ok(None);
        }
        let original = block.parameters;
        if authored && let Some(hold) = work.hold {
            block.parameters = without_parameter(&original, hold.id)?;
        }
        if !pending {
            return Ok(None);
        }
        let mut mailbox = match signal.capture.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => return Err(Fault::Protocol),
        };
        let slot = &mut mailbox.parameter_change;
        // Withdrawal may win after the worker's atomic observation and before
        // this lock. It must not become a source fault or a late mutation.
        if slot.phase != Phase::Queued || slot.intent == Intent::Idle {
            return Ok(None);
        }
        // Preserve queued custody through compensated pre-roll. Existing held
        // masking still applies, but an observed operation may only consume a
        // quantum whose exact first input sample has an Arrangement origin.
        if slot.require_arrangement_origin && !manual_origin_available {
            return Ok(None);
        }
        let (ordinal, point, intent) = (slot.ordinal, slot.point, slot.intent);
        work.ordinal = Some(ordinal);
        slot.phase = Phase::InFlight;
        signal.parameter_change_requested.store(false, Release);
        drop(mailbox);

        let holds = |ordinal| {
            work.hold
                .is_some_and(|hold| hold.ordinal == ordinal && hold.id == point.id)
        };
        let start = match intent {
            Intent::Point if work.hold.is_none_or(|hold| hold.id != point.id) => None,
            Intent::Hold
                if work.hold.is_none()
                    && authored
                    && matches!(&self.processor, Some(Processor::Native(instance))
                    if instance.has_armed_authored_parameter(point.id)) =>
            {
                Some(Hold {
                    ordinal,
                    id: point.id,
                })
            }
            Intent::Update(lease) if holds(lease) => None,
            Intent::Release(lease) if holds(lease) => {
                work.hold = None;
                work.releasing = Some(point.id);
                work.input = Some((block.sequence, block.first_frame));
                block.parameters = original;
                return Ok(None);
            }
            _ => {
                work.result = Some(Terminal::Refused);
                return Ok(None);
            }
        };

        let parameters = match start {
            Some(hold) => without_parameter(&original, hold.id)?,
            None => block.parameters,
        };
        // Removed points were counted by this physical callback. Reclaim those
        // slots only after admitting the exact hold and accepting the new point.
        let capacity = parameter_capacity + original.as_slice().len() - parameters.as_slice().len();
        let Some(merged) = merge_parameter_input(&parameters, point, capacity)? else {
            work.result = Some(Terminal::Refused);
            return Ok(None);
        };
        if let Some(hold) = start {
            work.hold = Some(hold);
        }
        block.parameters = merged;
        work.point = point;
        work.input = Some((block.sequence, block.first_frame));
        Ok(Some(point))
    }
    pub(super) fn complete_parameter_change(
        &mut self,
        reply: &Block,
    ) -> Result<Option<ManualParameterInput>, Fault> {
        let work = &mut self.parameter_change;
        let Some((sequence, first_frame)) = work.input.take() else {
            return Ok(None);
        };
        if sequence != reply.sequence || first_frame != reply.first_frame {
            return Err(Fault::Protocol);
        }
        let operation = work
            .ordinal
            .and_then(std::num::NonZeroU64::new)
            .ok_or(Fault::Protocol)?;
        if let Some(parameter) = work.releasing {
            work.result = Some(Terminal::Released {
                sequence,
                first_frame,
            });
            return Ok(Some(ManualParameterInput {
                operation,
                action: ManualParameterAction::Release { parameter },
            }));
        }
        let point = work.point;
        work.result = Some(Terminal::Processed(ProcessedParameterChange {
            point,
            sequence,
            first_frame,
        }));
        Ok(Some(ManualParameterInput {
            operation,
            action: ManualParameterAction::Point(point),
        }))
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
        self.parameter_change = Work {
            hold: self.parameter_change.hold,
            ..Work::default()
        };
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
