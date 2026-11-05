//! Bounded live gain commands and receipts for successfully written output quantums.
//! Control owns take construction/history. The callback owns only fixed-size values.
use rtrb::{Consumer, Producer, RingBuffer};

#[cfg(test)]
use crate::live_delivery::IN_FLIGHT;
use crate::live_delivery::{CAPACITY, Credits, Pending};
pub const MAX_TAKE_EVENTS: u64 = 1022;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub track_id: u64,
    pub project_generation: u64,
    pub canonical_revision: u64,
    pub audio_revision: u64,
    pub recording_epoch: u64,
    pub transport_epoch: u64,
    pub gesture_serial: u64,
    pub sample_rate: u32,
}

/// Fresh fixed channel payload quote; opaque allocator/OS costs are excluded.
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Command>(CAPACITY)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(CAPACITY)?)
}

#[cfg(test)]
mod tests;

impl Target {
    fn valid(self) -> bool {
        self.track_id != 0
            && self.canonical_revision != 0
            && self.audio_revision != 0
            && self.recording_epoch != 0
            && self.transport_epoch != 0
            && self.gesture_serial != 0
            && (8000..=192000).contains(&self.sample_rate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    SourceChanged,
    TransportChanged,
    OutputFailed,
    Cancelled,
    ClockExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Receipt {
    /// The value first affects this content frame and reached the supplied sink.
    Applied {
        target: Target,
        sequence: u64,
        gain: f32,
        performance_frame: u64,
        content_frame: u64,
        loop_pass: u64,
    },
    /// A newer command at the same application boundary replaced this value.
    Superseded { target: Target, sequence: u64 },
    Finished {
        target: Target,
        performance_frame: u64,
        content_frame: u64,
        loop_pass: u64,
    },
    /// The entire uncommitted take must be discarded; the original lane survives.
    Aborted { target: Target, reason: AbortReason },
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
    Begin(f32),
    Change(f32),
    Release,
    Cancel,
}
#[derive(Clone, Copy)]
struct Command {
    target: Target,
    sequence: u64,
    kind: Kind,
}

pub struct Control {
    commands: Producer<Command>,
    receipts: Consumer<Receipt>,
    target: Option<Target>,
    sequence: u64,
    credits: Credits,
    finishing: bool,
    status: Option<std::sync::Arc<crate::prepared::Status>>,
    #[cfg(test)]
    pub(crate) begin_gate: Option<std::sync::Arc<std::sync::Barrier>>,
}
pub(crate) struct Callback {
    commands: Consumer<Command>,
    receipts: Producer<Receipt>,
    owner: Option<Target>,
    value: Option<(Target, f32)>,
    staged: Option<(Command, Clock)>,
    terminal: Option<(Target, Clock)>,
    pending: Pending<Receipt>,
    /// Defensive telemetry; reservation invariants make this unreachable in normal use.
    pub(crate) receipt_fault: bool,
}
/// A process/content clock from the active prepared renderer, never a wall clock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Clock {
    pub performance_frame: u64,
    pub content_frame: u64,
    pub delivery_delay: u64,
    pub loop_pass: u64,
}
pub(crate) fn channel() -> (Control, Callback) {
    let (commands, incoming) = RingBuffer::new(CAPACITY);
    let (returning, receipts) = RingBuffer::new(CAPACITY);
    (
        Control {
            commands,
            receipts,
            target: None,
            sequence: 0,
            credits: Credits::default(),
            finishing: false,
            status: None,
            #[cfg(test)]
            begin_gate: None,
        },
        Callback {
            commands: incoming,
            receipts: returning,
            owner: None,
            value: None,
            staged: None,
            terminal: None,
            pending: Pending::default(),
            receipt_fault: false,
        },
    )
}
impl Control {
    pub(crate) fn bind_status(&mut self, status: std::sync::Arc<crate::prepared::Status>) {
        self.status = Some(status);
    }

    pub fn target(&self) -> Option<Target> {
        self.target
    }
    pub fn begin(&mut self, target: Target, gain: f32) -> Result<(), &'static str> {
        if !self.credits.is_empty() && self.target.is_none() {
            return Err("prior gain recording receipts are still draining");
        }
        if self.target.is_some() {
            return Err("finish the current gain take first");
        }
        if !target.valid() {
            return Err("gain recording target is invalid");
        }
        validate_gain(gain)?;
        // Split actors reserve from the same idle word as publication BEFORE
        // queue mutation. Legacy unsplit Control retains its historical ordering.
        let reserved = self
            .status
            .as_ref()
            .map_or(Ok(false), |s| s.reserve_gain_begin())?;
        #[cfg(test)]
        if let Some(gate) = &self.begin_gate {
            gate.wait();
            gate.wait();
        }
        if self
            .commands
            .push(Command {
                target,
                sequence: 1,
                kind: Kind::Begin(gain),
            })
            .is_err()
        {
            if reserved && let Some(status) = &self.status {
                status.gain_end();
            }
            return Err("gain recording command queue is full");
        }
        if let Some(status) = &self.status {
            status.gain_begin_committed();
        }
        self.target = Some(target);
        self.sequence = 1;
        self.credits.issued();
        self.finishing = false;
        Ok(())
    }
    pub fn change(&mut self, gain: f32) -> Result<(), &'static str> {
        validate_gain(gain)?;
        let target = self.target.ok_or("gain recording has not begun")?;
        if self.finishing {
            return Err("gain recording is finishing");
        }
        if self.sequence >= MAX_TAKE_EVENTS {
            return Err("gain take is full; release to save it");
        }
        if !self.credits.available() {
            return Err("gain recording is applying; retry the value");
        }
        let sequence = self.sequence + 1;
        self.commands
            .push(Command {
                target,
                sequence,
                kind: Kind::Change(gain),
            })
            .map_err(|_| "gain recording command queue is full")?;
        self.sequence = sequence;
        self.credits.issued();
        Ok(())
    }
    pub fn release(&mut self) -> Result<(), &'static str> {
        self.finish(Kind::Release)
    }
    pub fn cancel(&mut self) -> Result<(), &'static str> {
        self.finish(Kind::Cancel)
    }
    fn finish(&mut self, kind: Kind) -> Result<(), &'static str> {
        let target = self.target.ok_or("gain recording has not begun")?;
        if self.finishing {
            return Ok(());
        }
        // Ordinary commands leave independent terminal capacity in both rings.
        self.commands
            .push(Command {
                target,
                sequence: 0,
                kind,
            })
            .map_err(|_| "gain recording terminal reservation is unavailable")?;
        self.finishing = true;
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
        if Some(receipt.target()) == self.target {
            match receipt {
                Receipt::Applied { .. } | Receipt::Superseded { .. } => {}
                Receipt::Finished { .. } | Receipt::Aborted { .. } => {
                    self.target = None;
                    self.finishing = false;
                    if let Some(status) = &self.status {
                        status.gain_end();
                    }
                }
            }
        }
        Some(receipt)
    }
}
fn validate_gain(gain: f32) -> Result<(), &'static str> {
    if gain.is_finite() && (0.0..=2.0).contains(&gain) {
        Ok(())
    } else {
        Err("recorded gain must be finite and within 0..2")
    }
}
impl Callback {
    pub(crate) fn has_take(&self) -> bool {
        self.owner.is_some()
            || self.value.is_some()
            || self.staged.is_some()
            || self.terminal.is_some()
            || !self.pending.is_empty()
    }
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.commands.buffer())?
            .plus(ChannelStorage::ring(self.receipts.buffer())?)
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
    /// Captures one finite queue prefix. Values superseded before rendering are
    /// never reported as applied. The caller installs the returned raw-fader override.
    pub(crate) fn prepare(
        &mut self,
        mut admit: impl FnMut(Target) -> Result<Clock, AbortReason>,
    ) -> Option<(u64, f32)> {
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
            if !matches!(command.kind, Kind::Begin(_)) && self.owner != Some(command.target) {
                self.discard(command);
                continue;
            }
            if matches!(command.kind, Kind::Cancel) {
                if self.owner == Some(command.target) {
                    self.abort(AbortReason::Cancelled);
                }
                continue;
            }
            let clock = match admit(command.target) {
                Ok(clock) => clock,
                Err(reason) => {
                    self.discard(command);
                    if self.owner.is_none() {
                        self.owner = Some(command.target);
                    }
                    self.abort(reason);
                    continue;
                }
            };
            match command.kind {
                Kind::Begin(gain) if self.owner.is_none() => {
                    self.owner = Some(command.target);
                    self.value = Some((command.target, gain));
                    self.staged = Some((command, clock));
                }
                Kind::Change(gain)
                    if self.owner == Some(command.target) && self.value.is_some() =>
                {
                    if let Some((previous, _)) = self.staged.take() {
                        self.discard(previous);
                    }
                    self.value = Some((command.target, gain));
                    self.staged = Some((command, clock));
                }
                Kind::Release if self.owner == Some(command.target) => {
                    if let Some((previous, _)) = self.staged.take() {
                        self.discard(previous);
                    }
                    self.value = None;
                    self.terminal = Some((command.target, clock));
                }
                _ => self.discard(command),
            }
        }
        self.value.map(|(target, value)| (target.track_id, value))
    }
    fn discard_marker_value(&mut self, receipt: Receipt) {
        if let Receipt::Applied {
            target, sequence, ..
        } = receipt
        {
            self.emit(Receipt::Superseded { target, sequence });
        }
    }
    fn queue_marker(&mut self, clock: Clock, receipt: Receipt) {
        let Some(due) = clock.performance_frame.checked_add(clock.delivery_delay) else {
            self.discard_marker_value(receipt);
            self.abort(AbortReason::ClockExhausted);
            return;
        };
        if self.pending.push(due, receipt).is_err() {
            self.discard_marker_value(receipt);
            self.receipt_fault = true;
            self.abort(AbortReason::OutputFailed);
        }
    }
    /// Called AFTER final native conversion/write of this quantum. Receipt
    /// markers stay pending until their compensated Main sample was written.
    pub(crate) fn finish_quantum(&mut self, end_performance_frame: u64, valid: bool) {
        if !valid {
            self.abort(AbortReason::OutputFailed);
            return;
        }
        if let Some((command, clock)) = self.staged.take()
            && let Kind::Begin(gain) | Kind::Change(gain) = command.kind
        {
            self.queue_marker(
                clock,
                Receipt::Applied {
                    target: command.target,
                    sequence: command.sequence,
                    gain,
                    performance_frame: clock.performance_frame,
                    content_frame: clock.content_frame,
                    loop_pass: clock.loop_pass,
                },
            );
        }
        if let Some((target, mut clock)) = self.terminal.take() {
            // Preserve the release timestamp and delay publication through
            // the initial rejoin window and the downstream Main latency.
            let Some(delay) = clock
                .delivery_delay
                .checked_add(u64::from(target.sample_rate.div_ceil(200).max(1) - 1))
            else {
                self.abort(AbortReason::ClockExhausted);
                return;
            };
            clock.delivery_delay = delay;
            self.queue_marker(
                clock,
                Receipt::Finished {
                    target,
                    performance_frame: clock.performance_frame,
                    content_frame: clock.content_frame,
                    loop_pass: clock.loop_pass,
                },
            );
        }
        while let Some(receipt) = self.pending.pop_due(end_performance_frame) {
            self.emit(receipt);
            if matches!(receipt, Receipt::Finished { .. }) {
                self.owner = None;
            }
        }
    }
    pub(crate) fn abort(&mut self, reason: AbortReason) {
        let mut owner = self.owner.take();
        self.value = None;
        if let Some((command, _)) = self.staged.take() {
            self.discard(command);
        }
        self.terminal = None;
        while let Some(receipt) = self.pending.pop() {
            self.discard_marker_value(receipt);
        }
        // Drain the captured take before its terminal; no late normal receipt
        // can release credits belonging to a subsequent take.
        let count = self.commands.slots().min(CAPACITY);
        for _ in 0..count {
            if let Ok(command) = self.commands.pop() {
                owner = owner.or(Some(command.target));
                self.discard(command);
            }
        }
        if let Some(target) = owner {
            self.emit(Receipt::Aborted { target, reason });
        }
    }
}
