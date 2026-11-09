//! One source-fenced native control endpoint. Commands and receipts carry Copy
//! values only; engine processing and final checked sink delivery are separate.
//! This endpoint requires playing transport and direct output. Stopped
//! monitoring/audition and the delayed whole-output worker profile are not
//! implemented. Callback admission refuses these requests without fabricating
//! delivery clocks, retiring their normal credits and one aborted terminal.
//! Sixty-four normal commands may be outstanding; Release and Cancel each keep
//! a reserved terminal slot. There is no recording-duration/event lifetime cap.
use crate::{
    live_delivery::{CAPACITY, Credits, IN_FLIGHT, Pending},
    prepared::{AppliedAudioSource, Status},
};
use hex_engine::{NativeParameterApplication, NativeParameterInputTarget};
use hex_lattice::{NativeTransientOperation, NativeTransientValue};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;

/// Maximum outstanding receipts for ordinary parameter values (including
/// superseded values) before control must consume delivery credits.
pub const MAX_PENDING_VALUE_RECEIPTS: usize = IN_FLIGHT;

/// Maximum outstanding receipts including reserved terminal capacity.
pub const MAX_PENDING_RECEIPTS: usize = CAPACITY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub source: AppliedAudioSource,
    pub input: NativeParameterInputTarget,
    pub transport_epoch: u64,
    /// Strictly increasing per endpoint, including cancelled gestures.
    pub gesture_serial: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    SourceChanged,
    /// The source still matches, but its current runtime refuses this target.
    TargetUnavailable,
    TransportChanged,
    OutputFailed,
    UnsupportedOutput,
    Cancelled,
    ClockExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Receipt {
    Applied {
        target: Target,
        sequence: u64,
        application: NativeParameterApplication,
    },
    Superseded {
        target: Target,
        sequence: u64,
    },
    Finished {
        target: Target,
        application: NativeParameterApplication,
    },
    Aborted {
        target: Target,
        reason: AbortReason,
    },
}
impl Receipt {
    pub fn target(self) -> Target {
        match self {
            Self::Applied { target, .. }
            | Self::Superseded { target, .. }
            | Self::Finished { target, .. }
            | Self::Aborted { target, .. } => target,
        }
    }
}
#[derive(Clone, Copy)]
enum Kind {
    Set(NativeTransientValue),
    Release,
    Cancel,
}
#[derive(Clone, Copy)]
struct Command {
    target: Target,
    sequence: u64,
    kind: Kind,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Finishing {
    Release,
    Cancel,
}
pub struct Control {
    commands: Producer<Command>,
    receipts: Consumer<Receipt>,
    target: Option<Target>,
    last_gesture: u64,
    sequence: u64,
    credits: Credits,
    finishing: Option<Finishing>,
    status: Option<Arc<Status>>,
}
pub(crate) struct Callback {
    commands: Consumer<Command>,
    receipts: Producer<Receipt>,
    owner: Option<Target>,
    value: Option<NativeTransientOperation>,
    staged: Option<Command>,
    pending: Pending<Receipt>,
    // Control issues strictly increasing gesture serials. Keep the terminal
    // watermark after the owner is cleared: late old commands still owe a
    // normal receipt, but cannot consume another reserved terminal slot.
    last_terminated_gesture: u64,
    pub(crate) receipt_fault: bool,
}
/// Allocate the entire native endpoint on control. Its fixed pending receipts
/// must not inflate every outer prepared-callback constructor/move stack frame.
/// The existing prepared callback retains and retires this Box on control.
pub(crate) fn channel() -> (Control, Box<Callback>) {
    let (commands, incoming) = RingBuffer::new(CAPACITY);
    let (returning, receipts) = RingBuffer::new(CAPACITY);
    (
        Control {
            commands,
            receipts,
            target: None,
            last_gesture: 0,
            sequence: 0,
            credits: Credits::default(),
            finishing: None,
            status: None,
        },
        Box::new(Callback {
            commands: incoming,
            receipts: returning,
            owner: None,
            value: None,
            staged: None,
            pending: Pending::default(),
            last_terminated_gesture: 0,
            receipt_fault: false,
        }),
    )
}
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Command>(CAPACITY)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(CAPACITY)?)?
        .checked_add(std::mem::size_of::<Callback>())
}
impl Control {
    pub(crate) fn bind_status(&mut self, status: Arc<Status>) {
        self.status = Some(status);
    }
    pub fn target(&self) -> Option<Target> {
        self.target
    }
    /// Queue a playing-transport gesture. Queue acceptance is not application:
    /// callback admission authoritatively refuses stopped/stale sources and
    /// delayed whole-output worker profiles, returning credits and one terminal
    /// through the same receipt channel.
    pub fn begin(
        &mut self,
        target: Target,
        value: NativeTransientValue,
    ) -> Result<(), &'static str> {
        if self.target.is_some() || !self.credits.is_empty() {
            return Err("finish native parameter input and drain its receipts first");
        }
        if target.source.output_identity == 0
            || target.source.audio_revision == 0
            || target.source.context.editor_revision == 0
            || target.transport_epoch == 0
            || target.gesture_serial <= self.last_gesture
        {
            return Err("native parameter input target is invalid or reused");
        }
        if value.target() != target.input.target() {
            return Err("native parameter input value targets another control");
        }
        let status = self
            .status
            .as_ref()
            .ok_or("native parameter input output is unavailable")?;
        if status.recording_source() != Some(target.source)
            || status
                .transport_epoch
                .load(std::sync::atomic::Ordering::Acquire)
                != target.transport_epoch
        {
            return Err("native parameter input source changed");
        }
        status.reserve_native_parameter_input()?;
        if self
            .commands
            .push(Command {
                target,
                sequence: 1,
                kind: Kind::Set(value),
            })
            .is_err()
        {
            status.native_parameter_input_end();
            return Err("native parameter input command queue is full");
        }
        self.target = Some(target);
        self.last_gesture = target.gesture_serial;
        self.sequence = 1;
        self.credits.issued();
        self.finishing = None;
        Ok(())
    }
    pub fn change(&mut self, value: NativeTransientValue) -> Result<(), &'static str> {
        let target = self.target.ok_or("native parameter input has not begun")?;
        if self.finishing.is_some() {
            return Err("native parameter input is finishing");
        }
        if value.target() != target.input.target() {
            return Err("native parameter input value targets another control");
        }
        if !self.credits.available() {
            return Err("native parameter input is applying; retry the value");
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or("native parameter input sequence is exhausted")?;
        self.commands
            .push(Command {
                target,
                sequence,
                kind: Kind::Set(value),
            })
            .map_err(|_| "native parameter input command queue is full")?;
        self.sequence = sequence;
        self.credits.issued();
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), &'static str> {
        if self.finishing.is_some() {
            return Ok(());
        }
        let target = self.target.ok_or("native parameter input has not begun")?;
        self.commands
            .push(Command {
                target,
                sequence: 0,
                kind: Kind::Release,
            })
            .map_err(|_| "native parameter input terminal reservation is unavailable")?;
        self.finishing = Some(Finishing::Release);
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<(), &'static str> {
        if self.finishing == Some(Finishing::Cancel) {
            return Ok(());
        }
        let target = self.target.ok_or("native parameter input has not begun")?;
        // This bounded marker also wins after Release was consumed but before
        // its actual sink-delivery check. The second terminal slot stays reserved.
        if let Some(status) = &self.status {
            status.revoke_native_parameter_input(target.gesture_serial);
        }
        self.commands
            .push(Command {
                target,
                sequence: 0,
                kind: Kind::Cancel,
            })
            .map_err(|_| "native parameter input cancel reservation is unavailable")?;
        self.finishing = Some(Finishing::Cancel);
        Ok(())
    }
    pub fn try_receipt(&mut self) -> Option<Receipt> {
        let receipt = self.receipts.pop().ok()?;
        if matches!(
            receipt,
            Receipt::Applied { .. } | Receipt::Superseded { .. }
        ) {
            self.credits.retired();
        }
        if self.target == Some(receipt.target())
            && matches!(receipt, Receipt::Finished { .. } | Receipt::Aborted { .. })
        {
            self.target = None;
            self.finishing = None;
            if let Some(status) = &self.status {
                status.native_parameter_input_end();
            }
        }
        Some(receipt)
    }
}
impl Callback {
    pub(crate) fn has_input(&self) -> bool {
        self.owner.is_some()
            || self.value.is_some()
            || self.staged.is_some()
            || !self.pending.is_empty()
            || !self.commands.is_empty()
    }
    pub(crate) fn owner(&self) -> Option<Target> {
        self.owner
    }
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.commands.buffer())?
            .plus(ChannelStorage::ring(self.receipts.buffer())?)?
            .plus(ChannelStorage {
                // Exact private channel factory Box backing. The enclosing
                // prepared owner separately counts its one Box pointer.
                data_bytes: std::mem::size_of_val(self),
                shared_groups: 0,
                shared_handles: 0,
            })
    }
    fn terminated(&self, target: Target) -> bool {
        target.gesture_serial <= self.last_terminated_gesture
    }
    fn emit(&mut self, receipt: Receipt) {
        if self.receipts.push(receipt).is_err() {
            self.receipt_fault = true;
        }
    }
    fn discard(&mut self, command: Command) {
        if command.sequence != 0 {
            self.emit(Receipt::Superseded {
                target: command.target,
                sequence: command.sequence,
            });
        }
    }
    pub(crate) fn prepare(
        &mut self,
        mut admit: impl FnMut(Target) -> Result<(), AbortReason>,
    ) -> Option<NativeTransientOperation> {
        if let Some(owner) = self.owner
            && let Err(reason) = admit(owner)
        {
            self.abort(reason);
        }
        let count = self.commands.slots().min(CAPACITY);
        for _ in 0..count {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            if self.terminated(command.target) {
                self.discard(command);
                continue;
            }
            if matches!(command.kind, Kind::Cancel) {
                if self.owner == Some(command.target) {
                    self.abort(AbortReason::Cancelled);
                }
                continue;
            }
            if self.owner.is_some_and(|owner| owner != command.target) {
                self.discard(command);
                continue;
            }
            if let Err(reason) = admit(command.target) {
                self.discard(command);
                self.owner = Some(command.target);
                self.abort(reason);
                continue;
            }
            if let Some(previous) = self.staged.take() {
                self.discard(previous);
            }
            self.owner = Some(command.target);
            self.value = Some(match command.kind {
                Kind::Set(value) => NativeTransientOperation::Set(value),
                Kind::Release => NativeTransientOperation::Clear(command.target.input.target()),
                Kind::Cancel => continue,
            });
            self.staged = Some(command);
        }
        self.value
    }
    /// Engine observations are speculative until this final checked sink write.
    /// True means Clear reached the sink. Its remaining host ramp is owned by
    /// the renderer and can finish without a live endpoint request.
    pub(crate) fn finish_quantum(
        &mut self,
        application: Option<NativeParameterApplication>,
        end: u64,
        valid: bool,
        cancelled: bool,
    ) -> bool {
        if cancelled {
            self.abort(AbortReason::Cancelled);
            return false;
        }
        if !valid {
            self.abort(AbortReason::OutputFailed);
            return false;
        }
        if let (Some(command), Some(application)) = (self.staged, application) {
            if application.identity != command.target.input.identity()
                || Some(application.operation) != self.value
            {
                self.abort(AbortReason::SourceChanged);
                return false;
            }
            let Some(due) = application
                .performance_frame
                .checked_add(u64::from(application.remaining_latency_frames))
            else {
                self.abort(AbortReason::ClockExhausted);
                return false;
            };
            let receipt = if command.sequence == 0 {
                Receipt::Finished {
                    target: command.target,
                    application,
                }
            } else {
                Receipt::Applied {
                    target: command.target,
                    sequence: command.sequence,
                    application,
                }
            };
            if self.pending.push(due, receipt).is_err() {
                self.receipt_fault = true;
                self.abort(AbortReason::OutputFailed);
                return false;
            }
            self.staged = None;
        }
        let mut finished = false;
        while let Some(receipt) = self.pending.pop_due(end) {
            self.emit(receipt);
            if let Receipt::Finished { target, .. } = receipt {
                self.last_terminated_gesture =
                    self.last_terminated_gesture.max(target.gesture_serial);
                self.owner = None;
                self.value = None;
                finished = true;
            }
        }
        finished
    }
    pub(crate) fn abort(&mut self, reason: AbortReason) {
        let mut owner = self.owner.take();
        self.value = None;
        if let Some(command) = self.staged.take() {
            self.discard(command);
        }
        while let Some(receipt) = self.pending.pop() {
            if let Receipt::Applied {
                target, sequence, ..
            } = receipt
            {
                self.emit(Receipt::Superseded { target, sequence });
            }
        }
        let count = self.commands.slots().min(CAPACITY);
        for _ in 0..count {
            if let Ok(command) = self.commands.pop() {
                if !self.terminated(command.target) {
                    owner = owner.or(Some(command.target));
                }
                self.discard(command);
            }
        }
        if let Some(target) = owner.filter(|target| !self.terminated(*target)) {
            self.last_terminated_gesture = self.last_terminated_gesture.max(target.gesture_serial);
            self.emit(Receipt::Aborted { target, reason });
        }
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    #[test]
    fn native_channel_quote_accounts_for_actual_boxed_callback_and_ring_backing() {
        let (_control, callback) = channel();
        let actual = callback.constructor_channel_storage().unwrap();
        let rings = crate::native_callback_owner::ChannelStorage::ring_bytes::<Command>(CAPACITY)
            .unwrap()
            .checked_add(
                crate::native_callback_owner::ChannelStorage::ring_bytes::<Receipt>(CAPACITY)
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(actual.data_bytes, constructor_storage_quote().unwrap());
        assert_eq!(
            actual.data_bytes - rings,
            std::mem::size_of_val(callback.as_ref()),
            "exclusive Box pointee is counted once in addition to both real rings"
        );
        assert_eq!((actual.shared_groups, actual.shared_handles), (2, 4));
        println!(
            "native callback={} pending={} command={} receipt={} rings={} total={}",
            std::mem::size_of::<Callback>(),
            std::mem::size_of::<Pending<Receipt>>(),
            std::mem::size_of::<Command>(),
            std::mem::size_of::<Receipt>(),
            rings,
            actual.data_bytes
        );
    }
}
