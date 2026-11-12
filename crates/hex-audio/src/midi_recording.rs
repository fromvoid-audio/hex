//! Bounded MIDI recording commands and receipts for confirmed direct output.
//!
//! The engine supplies accepted note transitions from one completed render.
//! Only the native output owner may confirm them after conversion into its sink.
//! Control retains take construction/history. Neither endpoint owns a project,
//! graph, input worker or native device. Ordinary receipt pressure interrupts a
//! take explicitly; independent terminal capacity keeps Stop effective.
//! There is one pending Arm, 1024 note receipts and two critical receipts
//! (Armed plus terminal). One callback consumes at most one Arm and the engine
//! bounds a delivered batch to 576 transitions. A take admits at most 16384 On
//! events; control must consume the terminal before admission can be reused.
use hex_engine::midi_capture::{
    AcceptedMidiNote, MidiCaptureBinding, MidiCaptureClock, MidiCaptureEvent, MidiCaptureRender,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU64,
        Ordering::{AcqRel, Acquire, Release},
    },
};

const EVENT_CAPACITY: usize = 1024;
const TERMINAL_CAPACITY: usize = 2; // one Armed plus one Finished/Aborted per take
pub const MAX_TAKE_NOTES: u64 = 16_384;
const FINISH: u64 = 1;
const CANCEL: u64 = 2;
const MAX_TAKE_ID: u64 = u64::MAX >> 2;

/// Audio-source correlation is checked against the actual installed callback
/// owner; a copied target alone grants no admission or delivery authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub capture: MidiCaptureBinding,
    pub project_generation: u64,
    pub canonical_revision: u64,
    pub audio_revision: u64,
    pub output_identity: u64,
    pub transport_epoch: u64,
}
impl Target {
    fn valid(self) -> bool {
        (1..=MAX_TAKE_ID).contains(&self.capture.take_id)
            && self.canonical_revision != 0
            && self.audio_revision != 0
            && self.output_identity != 0
            && self.transport_epoch != 0
            && (8000..=192_000).contains(&self.capture.sample_rate)
            && self.capture.first_frame < self.capture.end_frame
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    SourceChanged,
    TransportChanged,
    InputChanged,
    OutputFailed,
    ReceiptPressure,
    TakeFull,
    InvalidClock,
    Cancelled,
    Capacity,
    RenderFailed,
    RouteChanged,
    UnsupportedOutput,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Armed {
        target: Target,
        clock: MidiCaptureClock,
    },
    Note {
        target: Target,
        note: AcceptedMidiNote,
        content_frame: u64,
    },
    /// None means Stop preceded the first delivered recording quantum. No note
    /// receipts then exist, so control may finish the empty take without an edit.
    Finished {
        target: Target,
        clock: Option<MidiCaptureClock>,
    },
    Aborted {
        target: Target,
        reason: AbortReason,
        last_delivered: Option<MidiCaptureClock>,
    },
}
impl Receipt {
    pub fn target(self) -> Target {
        match self {
            Self::Armed { target, .. }
            | Self::Note { target, .. }
            | Self::Finished { target, .. }
            | Self::Aborted { target, .. } => target,
        }
    }
}

pub struct Control {
    commands: Producer<Target>,
    events: Consumer<Receipt>,
    terminals: Consumer<Receipt>,
    finish_intent: Arc<AtomicU64>,
    target: Option<Target>,
    last_take: u64,
    armed_seen: bool,
    status: Option<Arc<crate::prepared::Status>>,
}
pub(crate) struct Callback {
    commands: Consumer<Target>,
    events: Producer<Receipt>,
    terminals: Producer<Receipt>,
    finish_intent: Arc<AtomicU64>,
    target: Option<Target>,
    staged: Option<(MidiCaptureClock, usize)>,
    last_delivered: Option<MidiCaptureClock>,
    armed_delivered: bool,
    note_ons: u64,
    pub(crate) receipt_fault: bool,
}
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<Target>(1)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(EVENT_CAPACITY)?)?
        .checked_add(ChannelStorage::ring_bytes::<Receipt>(TERMINAL_CAPACITY)?)?
        .checked_add(std::mem::size_of::<AtomicU64>())
}
pub(crate) fn channel() -> (Control, Callback) {
    let (commands, incoming) = RingBuffer::new(1);
    let (events, outgoing) = RingBuffer::new(EVENT_CAPACITY);
    let (terminals, returned) = RingBuffer::new(TERMINAL_CAPACITY);
    let finish_intent = Arc::new(AtomicU64::new(0));
    (
        Control {
            commands,
            events: outgoing,
            terminals: returned,
            finish_intent: Arc::clone(&finish_intent),
            target: None,
            last_take: 0,
            armed_seen: false,
            status: None,
        },
        Callback {
            commands: incoming,
            events,
            terminals,
            finish_intent,
            target: None,
            staged: None,
            last_delivered: None,
            armed_delivered: false,
            note_ons: 0,
            receipt_fault: false,
        },
    )
}
impl Control {
    pub(crate) fn bind_status(&mut self, status: Arc<crate::prepared::Status>) {
        self.status = Some(status);
    }
    pub fn target(&self) -> Option<Target> {
        self.target
    }
    pub fn begin(&mut self, target: Target) -> Result<(), &'static str> {
        if self.target.is_some() {
            return Err("finish the current midi take first");
        }
        if !target.valid() || target.capture.take_id <= self.last_take {
            return Err("midi recording target is invalid or stale");
        }
        if let Some(status) = &self.status {
            status.reserve_midi_begin()?;
        }
        self.finish_intent.store(0, Release);
        if self.commands.push(target).is_err() {
            if let Some(status) = &self.status {
                status.midi_end();
            }
            return Err("midi recording arm is still pending");
        }
        self.last_take = target.capture.take_id;
        self.armed_seen = false;
        self.target = Some(target);
        Ok(())
    }
    /// Out-of-band and take scoped. Called BEFORE input/output revocation by
    /// priority Stop, including when ordinary command/receipt queues are full.
    /// Call Cancel first for a known input interruption. A normal Stop finishes
    /// the confirmed prefix; it cannot infer whether input cancellation was
    /// caused by its own revocation or occurred earlier on a different producer.
    pub fn finish(&self) {
        if let Some(target) = self.target {
            let cancelled = (target.capture.take_id << 2) | CANCEL;
            let finished = (target.capture.take_id << 2) | FINISH;
            let _ = self.finish_intent.fetch_update(AcqRel, Acquire, |current| {
                (current != cancelled).then_some(finished)
            });
        }
    }
    pub fn cancel(&self) {
        if let Some(target) = self.target {
            self.finish_intent
                .store((target.capture.take_id << 2) | CANCEL, Release);
        }
    }
    pub fn try_receipt(&mut self) -> Option<Receipt> {
        // Before Armed, do not inspect the event ring: the producer can publish
        // between reads of the two independent queues. A refused/unstarted take
        // has only its terminal. Once Armed is seen, drain the event prefix.
        let receipt = if !self.armed_seen {
            let receipt = self.terminals.pop().ok()?;
            self.armed_seen = matches!(receipt, Receipt::Armed { .. });
            receipt
        } else if let Ok(event) = self.events.pop() {
            event
        } else {
            // Acquire the terminal, then RECHECK events. Its release follows all
            // event pushes, including those racing the earlier empty-ring read.
            self.terminals.peek().ok()?;
            self.events
                .pop()
                .ok()
                .or_else(|| self.terminals.pop().ok())?
        };
        if matches!(receipt, Receipt::Finished { .. } | Receipt::Aborted { .. })
            && Some(receipt.target()) == self.target
        {
            self.target = None;
            if let Some(status) = &self.status {
                status.midi_end();
            }
        }
        Some(receipt)
    }
}
impl Callback {
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.commands.buffer())?
            .plus(ChannelStorage::ring(self.events.buffer())?)?
            .plus(ChannelStorage::ring(self.terminals.buffer())?)?
            .plus(ChannelStorage::paired_payload(self.finish_intent.as_ref()))?
            // The prepared factory binds exactly one additional Status handle
            // in Control; its payload/group is already owned by the output root.
            .plus(ChannelStorage {
                data_bytes: 0,
                shared_groups: 0,
                shared_handles: 1,
            })
    }
    pub(crate) fn target(&self) -> Option<Target> {
        self.target
    }
    pub(crate) fn next_arm(&mut self) -> Option<Target> {
        self.commands.pop().ok()
    }
    pub(crate) fn accept(&mut self, target: Target) {
        self.target = Some(target);
        self.staged = None;
        self.last_delivered = None;
        self.armed_delivered = false;
        self.note_ons = 0;
    }
    pub(crate) fn requested_finish(&self) -> Option<bool> {
        let target = self.target?;
        let word = self.finish_intent.load(Acquire);
        (word >> 2 == target.capture.take_id)
            .then_some(word & 3 == CANCEL)
            .filter(|_| word & 3 != 0)
    }
    pub(crate) fn has_staged(&self) -> bool {
        self.staged.is_some()
    }
    pub(crate) fn stage(&mut self, clock: MidiCaptureClock, frames: usize) {
        self.staged = Some((clock, frames));
    }
    fn terminal(&mut self, receipt: Receipt) {
        if self.terminals.push(receipt).is_err() {
            self.receipt_fault = true;
        }
        self.target = None;
        self.staged = None;
    }
    pub(crate) fn abort(&mut self, reason: AbortReason) {
        if let Some(target) = self.target {
            self.terminal(Receipt::Aborted {
                target,
                reason,
                last_delivered: self.last_delivered,
            });
        }
    }
    /// Normal Stop uses the cached delivered boundary, never reset playback
    /// position. A queued Arm also receives its reserved empty terminal.
    pub(crate) fn finish(&mut self) {
        if self.requested_finish() == Some(true) {
            self.abort(AbortReason::Cancelled);
            return;
        }
        if let Some(target) = self.target {
            self.terminal(Receipt::Finished {
                target,
                clock: self.last_delivered,
            });
        }
    }
    /// Confirm only ordinary direct-copy output. Delayed/plugin transport has
    /// a separate delivery contract and is refused by this initial recorder.
    pub(crate) fn delivered(&mut self, render: MidiCaptureRender<'_>) {
        let Some(target) = self.target else {
            return;
        };
        let Some((clock, frames)) = self.staged.take() else {
            self.abort(AbortReason::InvalidClock);
            return;
        };
        if render.binding != target.capture
            || render.clock != clock
            || render.frames == 0
            || render.frames > frames
            || render.events.len() > hex_engine::midi_capture::MAX_ACCEPTED_MIDI_NOTES_PER_RENDER
        {
            self.abort(AbortReason::InvalidClock);
            return;
        }
        let Some(end) = u64::try_from(render.frames)
            .ok()
            .and_then(|n| clock.content_frame.checked_add(n))
        else {
            self.abort(AbortReason::InvalidClock);
            return;
        };
        let Some(performance_end) = u64::try_from(render.frames)
            .ok()
            .and_then(|n| clock.performance_frame.checked_add(n))
        else {
            self.abort(AbortReason::InvalidClock);
            return;
        };
        self.last_delivered = Some(MidiCaptureClock {
            content_frame: end.min(target.capture.end_frame),
            performance_frame: performance_end,
        });
        if !self.armed_delivered {
            if self
                .terminals
                .push(Receipt::Armed { target, clock })
                .is_err()
            {
                self.receipt_fault = true;
                self.abort(AbortReason::ReceiptPressure);
                return;
            }
            self.armed_delivered = true;
        }
        for note in render.events {
            let Some(offset) = note.performance_frame.checked_sub(clock.performance_frame) else {
                self.abort(AbortReason::InvalidClock);
                return;
            };
            let Some(content_frame) = clock.content_frame.checked_add(offset) else {
                self.abort(AbortReason::InvalidClock);
                return;
            };
            if content_frame >= target.capture.end_frame {
                continue;
            }
            if content_frame < target.capture.first_frame || content_frame >= end {
                self.abort(AbortReason::InvalidClock);
                return;
            }
            if matches!(note.event, MidiCaptureEvent::On { .. }) {
                if self.note_ons >= MAX_TAKE_NOTES {
                    self.abort(AbortReason::TakeFull);
                    return;
                }
                self.note_ons += 1;
            }
            if self
                .events
                .push(Receipt::Note {
                    target,
                    note: *note,
                    content_frame,
                })
                .is_err()
            {
                self.abort(AbortReason::ReceiptPressure);
                return;
            }
        }
        if end >= target.capture.end_frame {
            self.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{computer_keyboard::ComputerKeyboardInput, midi_capture::MidiCaptureSource};
    fn target() -> Target {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "take".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let input = ComputerKeyboardInput::default()
            .with_recording_support()
            .unwrap();
        Target {
            capture: MidiCaptureBinding {
                take_id: 1,
                track,
                device: editor
                    .project()
                    .track(track)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id(),
                sample_rate: 48_000,
                output_epoch: 1,
                keys: input.begin_ordered_keys_on_control().unwrap(),
                hardware: None,
                first_frame: 0,
                end_frame: 48_000,
            },
            project_generation: 1,
            canonical_revision: 1,
            audio_revision: 1,
            output_identity: 1,
            transport_epoch: 1,
        }
    }
    fn note(target: Target, frame: u64) -> AcceptedMidiNote {
        AcceptedMidiNote {
            source: MidiCaptureSource::Computer {
                activation: target.capture.keys,
                key: 0,
                press_token: frame / 2 + 1,
            },
            occurrence_id: frame / 2 + 1,
            performance_frame: frame,
            event: if frame.is_multiple_of(2) {
                MidiCaptureEvent::On {
                    pitch: 60,
                    velocity: 100,
                }
            } else {
                MidiCaptureEvent::Off
            },
        }
    }
    #[test]
    fn event_pressure_preserves_prefix_and_reserved_aborted_terminal_before_rearm() {
        let (mut control, mut callback) = channel();
        let target = target();
        control.begin(target).unwrap();
        let requested = callback.next_arm().unwrap();
        callback.accept(requested);
        let mut notes = [note(target, 0); 512];
        for block in 0..3 {
            let start = block * 512;
            for (index, event) in notes.iter_mut().enumerate() {
                *event = note(target, start + index as u64);
            }
            callback.stage(
                MidiCaptureClock {
                    content_frame: start,
                    performance_frame: start,
                },
                512,
            );
            callback.delivered(MidiCaptureRender {
                binding: target.capture,
                clock: MidiCaptureClock {
                    content_frame: start,
                    performance_frame: start,
                },
                frames: 512,
                events: &notes,
            });
        }
        assert!(callback.target().is_none());
        assert!(
            !callback.receipt_fault,
            "terminal capacity is independent of note pressure"
        );
        let next = Target {
            capture: MidiCaptureBinding {
                take_id: 2,
                ..target.capture
            },
            ..target
        };
        assert!(
            control.begin(next).is_err(),
            "unconsumed terminal retains take admission"
        );
        assert!(matches!(control.try_receipt(), Some(Receipt::Armed { .. })));
        for frame in 0..1024 {
            assert!(
                matches!(control.try_receipt(), Some(Receipt::Note { content_frame, .. }) if content_frame == frame)
            );
        }
        assert!(matches!(
            control.try_receipt(),
            Some(Receipt::Aborted {
                reason: AbortReason::ReceiptPressure,
                last_delivered: Some(MidiCaptureClock {
                    content_frame: 1536,
                    ..
                }),
                ..
            })
        ));
        assert!(control.try_receipt().is_none());
        assert!(control.begin(next).is_ok());
    }
    #[test]
    fn note_limit_and_clock_refusals_are_explicit_and_do_not_publish_finish() {
        for invalid_clock in [false, true] {
            let (mut control, mut callback) = channel();
            let target = target();
            control.begin(target).unwrap();
            let requested = callback.next_arm().unwrap();
            callback.accept(requested);
            callback.note_ons = MAX_TAKE_NOTES - 1;
            let notes = [note(target, 0), note(target, 2)];
            let clock = MidiCaptureClock {
                content_frame: 0,
                performance_frame: 0,
            };
            callback.stage(clock, 4);
            callback.delivered(MidiCaptureRender {
                binding: target.capture,
                clock: MidiCaptureClock {
                    content_frame: u64::from(invalid_clock),
                    ..clock
                },
                frames: 4,
                events: &notes,
            });
            let receipts: Vec<_> = std::iter::from_fn(|| control.try_receipt()).collect();
            assert!(
                matches!(receipts.last(), Some(Receipt::Aborted { reason, .. }) if *reason == if invalid_clock { AbortReason::InvalidClock } else { AbortReason::TakeFull })
            );
            assert!(
                !receipts
                    .iter()
                    .any(|receipt| matches!(receipt, Receipt::Finished { .. }))
            );
        }
    }
}
