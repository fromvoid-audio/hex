//! Bounded multi-port MIDI input aggregation with shared monotonic held-note
//! tokens and isolated per-port framing and overflow recovery.

use crate::framing::{ChannelFilter, Controllers, Decoder, for_each_bounded_packet};
use crate::held_notes::{HeldNoteInput, HeldNotes};
use crate::mpe::{
    MAX_MPE_MASTER_PITCH_BEND_SEMITONES, MAX_MPE_PITCH_BEND_SEMITONES, MpeConfig, MpeDecoder,
    MpeExpressionEvent,
};
use crate::note_events::{ControlChange, NoteEvent};
use crate::timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
};

/// Maximum number of MIDI input ports aggregated into one logical live input stream.
pub const MAX_AGGREGATED_PORTS: usize = 16;

/// Bounded per-port wait-free SPSC packet queue capacity (power of two).
const PORT_QUEUE_CAPACITY: usize = 64;
const PORT_QUEUE_MASK: usize = PORT_QUEUE_CAPACITY - 1;
/// Maximum payload bytes carried in one wait-free queued packet slot.
const PORT_PACKET_WORDS: usize = 32;
const PORT_PACKET_MAX_BYTES: usize = PORT_PACKET_WORDS * 8;

/// Losslessly pack [`Controllers`] into 63 bits for lock-free atomic reads.
fn pack_controllers(c: Controllers) -> u64 {
    let mut word = u64::from(c.pressure & 0x7f)
        | (u64::from(c.bend as u16) << 7)
        | (u64::from(c.wheel & 0x7f) << 23)
        | (u64::from(c.sustain & 0x7f) << 30)
        | (u64::from(c.expression & 0x7f) << 37);
    if let Some(cc) = c.last_cc {
        word |= (1_u64 << 44)
            | (u64::from(cc.channel & 0x0f) << 45)
            | (u64::from(cc.cc & 0x7f) << 49)
            | (u64::from(cc.value & 0x7f) << 56);
    }
    word
}

fn unpack_controllers(word: u64) -> Controllers {
    let last_cc = if (word >> 44) & 1 != 0 {
        Some(ControlChange {
            channel: ((word >> 45) & 0x0f) as u8,
            cc: ((word >> 49) & 0x7f) as u8,
            value: ((word >> 56) & 0x7f) as u8,
        })
    } else {
        None
    };
    Controllers {
        pressure: (word & 0x7f) as u8,
        bend: ((word >> 7) & 0xffff) as u16 as i16,
        wheel: ((word >> 23) & 0x7f) as u8,
        sustain: ((word >> 30) & 0x7f) as u8,
        expression: ((word >> 37) & 0x7f) as u8,
        last_cc,
    }
}

struct PacketSlot {
    timestamp_ticks: AtomicU64,
    len: AtomicU16,
    words: [AtomicU64; PORT_PACKET_WORDS],
}

impl PacketSlot {
    fn new() -> Self {
        Self {
            timestamp_ticks: AtomicU64::new(0),
            len: AtomicU16::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn write(&self, timestamp_ticks: u64, bytes: &[u8]) {
        self.timestamp_ticks
            .store(timestamp_ticks, Ordering::Relaxed);
        let len = bytes.len().min(PORT_PACKET_MAX_BYTES);
        self.len.store(len as u16, Ordering::Relaxed);
        let full_words = len / 8;
        for (i, chunk) in bytes[..full_words * 8].chunks_exact(8).enumerate() {
            let mut buf = [0_u8; 8];
            buf.copy_from_slice(chunk);
            self.words[i].store(u64::from_le_bytes(buf), Ordering::Relaxed);
        }
        let rem = len % 8;
        if rem > 0 {
            let mut buf = [0_u8; 8];
            buf[..rem].copy_from_slice(&bytes[full_words * 8..len]);
            self.words[full_words].store(u64::from_le_bytes(buf), Ordering::Relaxed);
        }
    }

    fn read(&self, out: &mut [u8; PORT_PACKET_MAX_BYTES]) -> (u64, usize) {
        let timestamp_ticks = self.timestamp_ticks.load(Ordering::Relaxed);
        let len = usize::from(self.len.load(Ordering::Relaxed)).min(PORT_PACKET_MAX_BYTES);
        let word_count = len.div_ceil(8);
        for i in 0..word_count {
            let bytes = self.words[i].load(Ordering::Relaxed).to_le_bytes();
            let start = i * 8;
            let end = (start + 8).min(len);
            out[start..end].copy_from_slice(&bytes[..(end - start)]);
        }
        (timestamp_ticks, len)
    }
}

/// Safe, wait-free, allocation-free per-port SPSC packet ring buffer.
///
/// The CoreMIDI Mach callback pushes packets using only atomics (zero mutexes,
/// zero heap allocation, zero caller closures); the worker/pump thread drains
/// packets into [`MultiPortAggregator`].
pub(crate) struct PortPacketQueue {
    slots: Box<[PacketSlot; PORT_QUEUE_CAPACITY]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    overflowed: AtomicBool,
}

impl PortPacketQueue {
    pub(crate) fn new() -> Self {
        Self {
            slots: Box::new(std::array::from_fn(|_| PacketSlot::new())),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            overflowed: AtomicBool::new(false),
        }
    }

    /// Wait-free enqueue from the port callback thread. Returns `true` if any
    /// slot was queued (or overflow was latched so the consumer should wake).
    pub(crate) fn enqueue_packet(&self, timestamp_ticks: u64, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let mut offset = 0;
        let mut tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        while offset < bytes.len() {
            if tail.wrapping_sub(head) >= PORT_QUEUE_CAPACITY {
                self.overflowed.store(true, Ordering::Release);
                return true;
            }
            let end = (offset + PORT_PACKET_MAX_BYTES).min(bytes.len());
            self.slots[tail & PORT_QUEUE_MASK].write(timestamp_ticks, &bytes[offset..end]);
            tail = tail.wrapping_add(1);
            offset = end;
        }
        self.tail.store(tail, Ordering::Release);
        true
    }

    pub(crate) fn pop_packet(&self, out: &mut [u8; PORT_PACKET_MAX_BYTES]) -> Option<(u64, usize)> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let packet = self.slots[head & PORT_QUEUE_MASK].read(out);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(packet)
    }

    pub(crate) fn take_overflowed(&self) -> bool {
        self.overflowed.swap(false, Ordering::AcqRel)
    }
}

struct AggregatedState {
    decoders: Vec<Decoder>,
    mpe_decoders: Vec<MpeDecoder>,
    current: Controllers,
    clock: MonotonicTimestampClock,
    merged_through: u64,
}

struct DeliveryCallbacks {
    publish: Box<dyn Fn(Controllers) + Send>,
    note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
}

/// Bounded multi-port MIDI input aggregator (up to [`MAX_AGGREGATED_PORTS`] ports).
///
/// Each port retains its own isolated [`Decoder`] (and optional [`MpeDecoder`])
/// and a wait-free [`PortPacketQueue`] so CoreMIDI Mach callbacks never lock a
/// mutex or invoke caller closures. `controllers()` and `set_mpe_pitch_bend_ranges()`
/// are completely lock-free via atomics, and delivery releases decoder state
/// before invoking caller `publish`/`note` closures.
pub struct MultiPortAggregator {
    filter: ChannelFilter,
    mpe: Option<MpeConfig>,
    timebase: MachTimebase,
    notes: Arc<HeldNoteInput>,
    current_packed: AtomicU64,
    pending_mpe_ranges: AtomicU16,
    port_queues: Vec<Arc<PortPacketQueue>>,
    state: std::sync::Mutex<AggregatedState>,
    callbacks: std::sync::Mutex<DeliveryCallbacks>,
}

impl MultiPortAggregator {
    /// Construct a bounded multi-port aggregator for `port_count` distinct input ports
    /// (`1..=MAX_AGGREGATED_PORTS`) using `filter` and `timebase`.
    pub fn new(
        port_count: usize,
        filter: ChannelFilter,
        timebase: MachTimebase,
        notes: Arc<HeldNoteInput>,
        publish: Box<dyn Fn(Controllers) + Send>,
        note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
    ) -> Result<Self, &'static str> {
        if port_count == 0 || port_count > MAX_AGGREGATED_PORTS {
            return Err("controller port count must be between 1 and 16");
        }
        if !filter.is_valid() {
            return Err("controller channel is invalid");
        }
        let mut decoders = Vec::new();
        decoders
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate midi port decoders")?;
        let mut port_queues = Vec::new();
        port_queues
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate midi port queues")?;
        for _ in 0..port_count {
            decoders.push(Decoder::default());
            port_queues.push(Arc::new(PortPacketQueue::new()));
        }
        Ok(Self {
            filter,
            mpe: None,
            timebase,
            notes,
            current_packed: AtomicU64::new(pack_controllers(Controllers::default())),
            pending_mpe_ranges: AtomicU16::new(0),
            port_queues,
            state: std::sync::Mutex::new(AggregatedState {
                decoders,
                mpe_decoders: Vec::new(),
                current: Controllers::default(),
                clock: MonotonicTimestampClock::default(),
                merged_through: 0,
            }),
            callbacks: std::sync::Mutex::new(DeliveryCallbacks { publish, note }),
        })
    }

    /// Construct a bounded multi-port MPE aggregator for `port_count` distinct
    /// input ports (`1..=MAX_AGGREGATED_PORTS`) with one validated [`MpeConfig`].
    pub fn new_mpe(
        port_count: usize,
        config: MpeConfig,
        timebase: MachTimebase,
        notes: Arc<HeldNoteInput>,
        publish: Box<dyn Fn(Controllers) + Send>,
        note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
    ) -> Result<Self, &'static str> {
        if port_count == 0 || port_count > MAX_AGGREGATED_PORTS {
            return Err("controller port count must be between 1 and 16");
        }
        if !config.is_valid() {
            return Err("controller mpe zone configuration is invalid");
        }
        let mut decoders = Vec::new();
        decoders
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate midi port decoders")?;
        let mut mpe_decoders = Vec::new();
        mpe_decoders
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate mpe port decoders")?;
        let mut port_queues = Vec::new();
        port_queues
            .try_reserve_exact(port_count)
            .map_err(|_| "could not allocate midi port queues")?;
        for _ in 0..port_count {
            decoders.push(Decoder::default());
            mpe_decoders.push(MpeDecoder::new(config));
            port_queues.push(Arc::new(PortPacketQueue::new()));
        }
        Ok(Self {
            filter: ChannelFilter::Omni,
            mpe: Some(config),
            timebase,
            notes,
            current_packed: AtomicU64::new(pack_controllers(Controllers::default())),
            pending_mpe_ranges: AtomicU16::new(0),
            port_queues,
            state: std::sync::Mutex::new(AggregatedState {
                decoders,
                mpe_decoders,
                current: Controllers::default(),
                clock: MonotonicTimestampClock::default(),
                merged_through: 0,
            }),
            callbacks: std::sync::Mutex::new(DeliveryCallbacks { publish, note }),
        })
    }

    /// Borrow the wait-free per-port packet queue for `port_index` so native
    /// callbacks push raw MIDI bytes without acquiring any mutex.
    pub(crate) fn port_queue(&self, port_index: usize) -> Option<Arc<PortPacketQueue>> {
        self.port_queues.get(port_index).map(Arc::clone)
    }

    /// Wait-free enqueue of one timestamped packet onto `port_index`'s queue.
    pub fn enqueue_packet(&self, port_index: usize, timestamp_ticks: u64, bytes: &[u8]) -> bool {
        self.port_queues
            .get(port_index)
            .is_some_and(|queue| queue.enqueue_packet(timestamp_ticks, bytes))
    }

    /// Drain all queued per-port lock-free queues on the consumer thread and
    /// deliver decoded updates. Returns the number of packets drained.
    pub fn drain_queued_ports(&self) -> usize {
        if self.notes.is_cancelled() {
            return 0;
        }
        let mut drained = 0usize;
        let mut scratch = [0_u8; PORT_PACKET_MAX_BYTES];
        for (port_index, queue) in self.port_queues.iter().enumerate() {
            while let Some((timestamp_ticks, len)) = queue.pop_packet(&mut scratch) {
                self.deliver_timed_packets(
                    port_index,
                    [TimedPacket {
                        timestamp_ticks,
                        data: &scratch[..len],
                    }],
                );
                drained += 1;
                if self.notes.is_cancelled() {
                    return drained;
                }
            }
            if queue.take_overflowed() {
                self.handle_port_queue_overflow(port_index);
                drained += 1;
            }
        }
        drained
    }

    fn handle_port_queue_overflow(&self, port_index: usize) {
        if self.notes.is_cancelled() {
            return;
        }
        let mut emitted_controllers: Vec<Controllers> = Vec::new();
        let mut emitted_notes: Vec<TimestampedNoteEvent> = Vec::new();
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if self.notes.is_cancelled() || port_index >= state.decoders.len() {
                return;
            }
            let port_count = state.decoders.len();
            let timestamp_ns = state.clock.advance(self.timebase, 0);
            let AggregatedState {
                decoders,
                mpe_decoders,
                current,
                merged_through,
                ..
            } = &mut *state;
            let decoder = &mut decoders[port_index];
            let notes_out = std::cell::RefCell::new(&mut emitted_notes);
            let emit = |event| {
                notes_out.borrow_mut().push(TimestampedNoteEvent {
                    timestamp_ns,
                    event,
                });
            };
            if let Some(mpe) = mpe_decoders.get_mut(port_index) {
                if port_count > 1 {
                    for ch in 0_u8..16 {
                        if let Some(active) = mpe.active_note(ch) {
                            emit(NoteEvent::Off {
                                channel: ch,
                                pitch: active.pitch,
                                velocity: 0,
                            });
                        }
                    }
                    mpe.reset();
                    decoder.clear_held_and_framing();
                } else {
                    mpe.reset();
                    *current = Controllers::default();
                    self.current_packed
                        .store(pack_controllers(*current), Ordering::Release);
                    emitted_controllers.push(*current);
                    decoder.reset(&emit);
                }
            } else if port_count > 1 {
                decoder.reset_port_isolated(self.filter, &emit);
            } else {
                decoder.reset(&emit);
            }
            if let Some(port_snapshot) = decoder.take_held_notes() {
                *merged_through = (*merged_through).max(port_snapshot.through());
                let merged = HeldNotes::merge_from(
                    decoders.iter().map(Decoder::held_notes),
                    *merged_through,
                );
                self.notes.publish(merged);
            }
        }
        self.dispatch_emitted(&emitted_controllers, &emitted_notes);
    }

    fn dispatch_emitted(
        &self,
        emitted_controllers: &[Controllers],
        emitted_notes: &[TimestampedNoteEvent],
    ) {
        if emitted_controllers.is_empty() && emitted_notes.is_empty() {
            return;
        }
        let Ok(callbacks) = self.callbacks.lock() else {
            return;
        };
        for &ctrl in emitted_controllers {
            (callbacks.publish)(ctrl);
        }
        for &ev in emitted_notes {
            (callbacks.note)(ev);
        }
    }

    /// Deliver one batch of timestamped packets from `port_index` into its isolated
    /// decoder and publish merged held-note / controller / ordered-event updates.
    ///
    /// Releases internal decoder state before invoking caller `publish`/`note`
    /// closures so re-entrant queries (`controllers()`, `set_mpe_pitch_bend_ranges()`)
    /// never block or deadlock.
    pub fn deliver_timed_packets<'a>(
        &self,
        port_index: usize,
        packets: impl IntoIterator<Item = TimedPacket<'a>>,
    ) {
        if self.notes.is_cancelled() {
            return;
        }
        let mut emitted_controllers: Vec<Controllers> = Vec::new();
        let mut emitted_notes: Vec<TimestampedNoteEvent> = Vec::new();
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if self.notes.is_cancelled() || port_index >= state.decoders.len() {
                return;
            }
            let packed_ranges = self.pending_mpe_ranges.swap(0, Ordering::AcqRel);
            if packed_ranges != 0 {
                let member = (packed_ranges >> 8) as u8;
                let master = (packed_ranges & 0xff) as u8;
                for decoder in &mut state.mpe_decoders {
                    let _ = decoder.set_pitch_bend_ranges(member, master);
                }
            }
            let port_count = state.decoders.len();
            let AggregatedState {
                decoders,
                mpe_decoders,
                current,
                clock,
                merged_through,
            } = &mut *state;
            let decoder = &mut decoders[port_index];
            let mut mpe_decoder = if self.mpe.is_some() {
                mpe_decoders.get_mut(port_index)
            } else {
                None
            };
            let mut faulted = false;
            for_each_bounded_packet(packets, |timestamp_ticks, accepted, overflow| {
                let timestamp_ns = clock.advance(self.timebase, timestamp_ticks);
                let notes_out = std::cell::RefCell::new(&mut emitted_notes);
                let emit = |event| {
                    notes_out.borrow_mut().push(TimestampedNoteEvent {
                        timestamp_ns,
                        event,
                    });
                };
                if let Some(mpe) = mpe_decoder.as_deref_mut() {
                    let emit_mpe = |event: MpeExpressionEvent| {
                        let mapped = match event {
                            MpeExpressionEvent::NoteOn {
                                channel,
                                pitch,
                                velocity,
                            } => Some(NoteEvent::On {
                                channel,
                                pitch,
                                velocity,
                            }),
                            MpeExpressionEvent::NoteOff {
                                channel,
                                pitch,
                                velocity,
                            } => Some(NoteEvent::Off {
                                channel,
                                pitch,
                                velocity,
                            }),
                            MpeExpressionEvent::NotePitchBend {
                                channel,
                                pitch,
                                cents,
                            } => Some(NoteEvent::NotePitchBend {
                                channel,
                                pitch,
                                cents,
                            }),
                            MpeExpressionEvent::NoteSlide {
                                channel,
                                pitch,
                                value,
                            } => Some(NoteEvent::NoteTimbre {
                                channel,
                                pitch,
                                value,
                            }),
                            MpeExpressionEvent::NotePressure {
                                channel,
                                pitch,
                                value,
                            } => Some(NoteEvent::PolyPressure {
                                channel,
                                pitch,
                                value,
                            }),
                            MpeExpressionEvent::MasterControlChange { channel, cc, value } => {
                                Some(NoteEvent::ControlChange { channel, cc, value })
                            }
                            MpeExpressionEvent::AllNotesOff => Some(NoteEvent::AllNotesOff),
                            MpeExpressionEvent::MasterPitchBend { .. }
                            | MpeExpressionEvent::MasterPressure { .. } => None,
                        };
                        if let Some(note_event) = mapped {
                            emit(note_event);
                        }
                    };
                    for &byte in accepted {
                        if let Some(next) = decoder.byte_with_mpe_with_token(
                            byte,
                            mpe,
                            *current,
                            merged_through,
                            &emit_mpe,
                        ) {
                            *current = next;
                            self.current_packed
                                .store(pack_controllers(*current), Ordering::Release);
                            emitted_controllers.push(*current);
                        }
                        if let Some(fault) = decoder.held_note_fault() {
                            emit(NoteEvent::InputLost(fault));
                            self.notes.fail(fault);
                            faulted = true;
                            return false;
                        }
                    }
                    if overflow {
                        if port_count > 1 {
                            // Release only active notes owned by this overflowing MPE port.
                            for ch in 0_u8..16 {
                                if let Some(active) = mpe.active_note(ch) {
                                    emit(NoteEvent::Off {
                                        channel: ch,
                                        pitch: active.pitch,
                                        velocity: 0,
                                    });
                                }
                            }
                            mpe.reset();
                            decoder.clear_held_and_framing();
                        } else {
                            mpe.reset();
                            *current = Controllers::default();
                            self.current_packed
                                .store(pack_controllers(*current), Ordering::Release);
                            emitted_controllers.push(*current);
                            decoder.reset(&emit);
                        }
                    }
                } else {
                    for &byte in accepted {
                        if let Some(next) = decoder.byte_with_notes_filtered_with_token(
                            byte,
                            self.filter,
                            *current,
                            merged_through,
                            &emit,
                        ) {
                            *current = next;
                            self.current_packed
                                .store(pack_controllers(*current), Ordering::Release);
                            emitted_controllers.push(*current);
                        }
                        if let Some(fault) = decoder.held_note_fault() {
                            self.notes.fail(fault);
                            faulted = true;
                            return false;
                        }
                    }
                    if overflow {
                        if port_count > 1 {
                            decoder.reset_port_isolated(self.filter, &emit);
                        } else {
                            decoder.reset(&emit);
                        }
                    }
                }
                true
            });
            if !faulted && let Some(fault) = decoder.held_note_fault() {
                self.notes.fail(fault);
                faulted = true;
            }
            if !faulted && let Some(port_snapshot) = decoder.take_held_notes() {
                *merged_through = (*merged_through).max(port_snapshot.through());
                let merged = HeldNotes::merge_from(
                    decoders.iter().map(Decoder::held_notes),
                    *merged_through,
                );
                self.notes.publish(merged);
            }
        }
        self.dispatch_emitted(&emitted_controllers, &emitted_notes);
    }

    /// Lock-free snapshot of the latest aggregated controller state.
    pub fn controllers(&self) -> Controllers {
        unpack_controllers(self.current_packed.load(Ordering::Acquire))
    }

    /// Lock-free update of member (`1..=96` st) and master (`1..=48` st) MPE
    /// pitch-bend ranges across every aggregated port without blocking any
    /// callback or resetting active notes.
    pub fn set_mpe_pitch_bend_ranges(
        &self,
        member_semitones: u8,
        master_semitones: u8,
    ) -> Result<(), &'static str> {
        if self.mpe.is_none() {
            return Err("controller input is not in mpe mode");
        }
        if member_semitones == 0
            || member_semitones > MAX_MPE_PITCH_BEND_SEMITONES
            || master_semitones == 0
            || master_semitones > MAX_MPE_MASTER_PITCH_BEND_SEMITONES
        {
            return Err("controller mpe pitch bend range is invalid");
        }
        let packed = (u16::from(member_semitones) << 8) | u16::from(master_semitones);
        self.pending_mpe_ranges.store(packed, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::CALLBACK_PACKETS;
    use crate::held_notes::HeldNotesRead;
    use crate::mpe::MpeZone;

    #[test]
    fn multi_port_aggregator_isolates_running_status_merges_held_notes_and_enforces_bounds() {
        let held = Arc::new(HeldNoteInput::default());
        assert!(
            MultiPortAggregator::new(
                0,
                ChannelFilter::Omni,
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );
        assert!(
            MultiPortAggregator::new(
                MAX_AGGREGATED_PORTS + 1,
                ChannelFilter::Omni,
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );
        assert!(
            MultiPortAggregator::new(
                2,
                ChannelFilter::Single(16),
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(|_| {}),
                Box::new(|_| {}),
            )
            .is_err()
        );

        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered_sink = Arc::clone(&delivered);
        let agg = MultiPortAggregator::new(
            2,
            ChannelFilter::Omni,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(move |ev| {
                delivered_sink.lock().unwrap().push(ev);
            }),
        )
        .unwrap();

        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 100,
                data: &[0x90, 60, 100],
            }],
        );
        agg.deliver_timed_packets(
            1,
            [TimedPacket {
                timestamp_ticks: 90,
                data: &[0x92, 60, 95, 64, 80],
            }],
        );
        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 200,
                data: &[60, 0],
            }],
        );

        let events = delivered.lock().unwrap();
        assert_eq!(
            *events,
            vec![
                TimestampedNoteEvent {
                    timestamp_ns: 100,
                    event: NoteEvent::On {
                        channel: 0,
                        pitch: 60,
                        velocity: 100
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 100,
                    event: NoteEvent::On {
                        channel: 2,
                        pitch: 60,
                        velocity: 95
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 100,
                    event: NoteEvent::On {
                        channel: 2,
                        pitch: 64,
                        velocity: 80
                    }
                },
                TimestampedNoteEvent {
                    timestamp_ns: 200,
                    event: NoteEvent::Off {
                        channel: 0,
                        pitch: 60,
                        velocity: 0
                    }
                },
            ]
        );
        let HeldNotesRead::Snapshot(merged) = held.read() else {
            panic!("expected merged snapshot");
        };
        let mut pitches: Vec<u8> = merged.after(0).map(|n| n.pitch).collect();
        pitches.sort_unstable();
        assert_eq!(pitches, vec![60, 64]);
    }

    #[test]
    fn multi_port_monotonic_tokens_survive_cross_port_floor_reset() {
        let held = Arc::new(HeldNoteInput::default());
        let agg = MultiPortAggregator::new(
            2,
            ChannelFilter::Omni,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(|_| {}),
        )
        .unwrap();

        // Port 0 presses 10 notes -> merged.through() reaches 10.
        for i in 0_u8..10 {
            agg.deliver_timed_packets(
                0,
                [TimedPacket {
                    timestamp_ticks: u64::from(i) * 10,
                    data: &[0x90, 48 + i, 100, 0x80, 48 + i, 0],
                }],
            );
        }
        let HeldNotesRead::Snapshot(after_port0) = held.read() else {
            panic!("expected snapshot after port 0 presses");
        };
        let floor = after_port0.through();
        assert_eq!(floor, 10);

        // First press on port 1 MUST get token 11 (> floor 10), never token 1!
        agg.deliver_timed_packets(
            1,
            [TimedPacket {
                timestamp_ticks: 200,
                data: &[0x91, 60, 96],
            }],
        );
        let HeldNotesRead::Snapshot(after_port1) = held.read() else {
            panic!("expected snapshot after port 1 press");
        };
        assert_eq!(after_port1.through(), 11);
        let visible: Vec<_> = after_port1.after(floor).collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].pitch, 60);
        assert_eq!(visible[0].token, 11);
    }

    #[test]
    fn multi_port_burst_overflow_and_omni_cc123_preserve_healthy_other_ports() {
        let held = Arc::new(HeldNoteInput::default());
        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered_sink = Arc::clone(&delivered);
        let agg = MultiPortAggregator::new(
            2,
            ChannelFilter::Omni,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(move |ev| {
                delivered_sink.lock().unwrap().push(ev.event);
            }),
        )
        .unwrap();

        // Port 0 holds ch 0 pitch 60; Port 1 holds ch 2 pitch 67.
        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 10,
                data: &[0x90, 60, 100],
            }],
        );
        agg.deliver_timed_packets(
            1,
            [TimedPacket {
                timestamp_ticks: 20,
                data: &[0x92, 67, 90],
            }],
        );
        delivered.lock().unwrap().clear();

        // Port 1 overflows CALLBACK_PACKETS -> must release ONLY port 1's pitch 67,
        // emit NO global AllNotesOff, and keep port 0's pitch 60 held!
        let overflow_packets: Vec<_> = (0..=CALLBACK_PACKETS)
            .map(|i| TimedPacket {
                timestamp_ticks: 30 + i as u64,
                data: &[0xf8_u8][..],
            })
            .collect();
        agg.deliver_timed_packets(1, overflow_packets);

        let events = delivered.lock().unwrap().clone();
        assert!(
            !events.contains(&NoteEvent::AllNotesOff),
            "per-port overflow must not emit global AllNotesOff: {events:?}"
        );
        assert_eq!(
            events,
            vec![NoteEvent::Off {
                channel: 2,
                pitch: 67,
                velocity: 0,
            }]
        );
        let HeldNotesRead::Snapshot(after_overflow) = held.read() else {
            panic!("expected snapshot after port 1 overflow");
        };
        let remaining: Vec<u8> = after_overflow.after(0).map(|n| n.pitch).collect();
        assert_eq!(remaining, vec![60]);

        // Omni CC123 on port 1 channel 3 (idle) must emit nothing and leave port 0 pitch 60 intact.
        delivered.lock().unwrap().clear();
        agg.deliver_timed_packets(
            1,
            [TimedPacket {
                timestamp_ticks: 2000,
                data: &[0xb3, 123, 0],
            }],
        );
        assert!(delivered.lock().unwrap().is_empty());
        let HeldNotesRead::Snapshot(after_cc123) = held.read() else {
            panic!("expected snapshot");
        };
        assert_eq!(
            after_cc123.after(0).map(|n| n.pitch).collect::<Vec<_>>(),
            vec![60]
        );
    }

    #[test]
    fn mpe_multi_port_aggregator_decodes_lower_and_upper_zones_tracks_held_notes_and_rejects_invalid_zones()
     {
        let held = Arc::new(HeldNoteInput::default());
        for bad in [
            MpeConfig {
                zone: MpeZone::Lower { member_channels: 0 },
                pitch_bend_range_semitones: 48,
                master_pitch_bend_range_semitones: 2,
            },
            MpeConfig {
                zone: MpeZone::Upper {
                    member_channels: 16,
                },
                pitch_bend_range_semitones: 48,
                master_pitch_bend_range_semitones: 2,
            },
            MpeConfig {
                zone: MpeZone::Lower { member_channels: 7 },
                pitch_bend_range_semitones: 0,
                master_pitch_bend_range_semitones: 2,
            },
            MpeConfig {
                zone: MpeZone::Lower { member_channels: 7 },
                pitch_bend_range_semitones: 48,
                master_pitch_bend_range_semitones: 0,
            },
        ] {
            assert!(
                MultiPortAggregator::new_mpe(
                    1,
                    bad,
                    MachTimebase::NANOSECONDS,
                    Arc::clone(&held),
                    Box::new(|_| {}),
                    Box::new(|_| {}),
                )
                .is_err()
            );
        }

        let config = MpeConfig::new(MpeZone::Upper { member_channels: 4 }, 48).unwrap();
        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered_sink = Arc::clone(&delivered);
        let agg = MultiPortAggregator::new_mpe(
            1,
            config,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(move |ev| {
                delivered_sink.lock().unwrap().push(ev);
            }),
        )
        .unwrap();

        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 500,
                data: &[
                    0x92, 55, 100, // out-of-zone -> ignored
                    0x9e, 60, 100, // member ch 14 NoteOn
                    0xee, 127, 127, // member ch 14 max pitch bend (+4800c)
                    0xbe, 74, 99, // member ch 14 CC74 timbre
                    0xde, 80, // member ch 14 channel pressure
                    0xbf, 1, 64, // master ch 15 mod wheel
                ],
            }],
        );
        let events = delivered.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                TimestampedNoteEvent {
                    timestamp_ns: 500,
                    event: NoteEvent::On {
                        channel: 14,
                        pitch: 60,
                        velocity: 100,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 500,
                    event: NoteEvent::NotePitchBend {
                        channel: 14,
                        pitch: 60,
                        cents: 4800,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 500,
                    event: NoteEvent::NoteTimbre {
                        channel: 14,
                        pitch: 60,
                        value: 99,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 500,
                    event: NoteEvent::PolyPressure {
                        channel: 14,
                        pitch: 60,
                        value: 80,
                    },
                },
                TimestampedNoteEvent {
                    timestamp_ns: 500,
                    event: NoteEvent::ControlChange {
                        channel: 15,
                        cc: 1,
                        value: 64,
                    },
                },
            ]
        );
        assert_eq!(agg.controllers().wheel, 64);
        let HeldNotesRead::Snapshot(held_now) = held.read() else {
            panic!("expected held snapshot");
        };
        assert_eq!(
            held_now.after(0).map(|n| n.pitch).collect::<Vec<_>>(),
            vec![60]
        );

        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 900,
                data: &[0xff],
            }],
        );
        let HeldNotesRead::Snapshot(after_reset) = held.read() else {
            panic!("expected snapshot after reset");
        };
        assert_eq!(after_reset.after(0).count(), 0);
        assert_eq!(
            delivered.lock().unwrap().last().unwrap().event,
            NoteEvent::AllNotesOff
        );
    }

    #[test]
    fn mpe_aggregator_updates_bend_ranges_live_without_retuning_held_notes_or_double_applying_master_bend()
     {
        let held = Arc::new(HeldNoteInput::default());
        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delivered_sink = Arc::clone(&delivered);
        let config = MpeConfig::with_ranges(MpeZone::Lower { member_channels: 4 }, 48, 12).unwrap();
        let agg = MultiPortAggregator::new_mpe(
            1,
            config,
            MachTimebase::NANOSECONDS,
            Arc::clone(&held),
            Box::new(|_| {}),
            Box::new(move |ev| {
                delivered_sink.lock().unwrap().push(ev);
            }),
        )
        .unwrap();

        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 100,
                data: &[0x91, 60, 100],
            }],
        );
        assert!(agg.set_mpe_pitch_bend_ranges(0, 2).is_err());
        assert!(agg.set_mpe_pitch_bend_ranges(12, 49).is_err());
        agg.set_mpe_pitch_bend_ranges(12, 2).unwrap();
        let HeldNotesRead::Snapshot(still_held) = held.read() else {
            panic!("expected held snapshot");
        };
        assert_eq!(
            still_held.after(0).map(|n| n.pitch).collect::<Vec<_>>(),
            vec![60]
        );

        agg.deliver_timed_packets(
            0,
            [TimedPacket {
                timestamp_ticks: 200,
                data: &[
                    0x92, 64, 100, // ch 2 captures 12 st range
                    0xe1, 0x00, 0x50, // ch 1 (+2048 at 48 st = +1200c)
                    0xe2, 0x7f, 0x7f, // ch 2 (+8191 at 12 st = +1200c)
                    0xe0, 0x7f, 0x7f, // master (+8191 at 2 st = +200c)
                ],
            }],
        );
        assert_eq!(agg.controllers().bend, 0);
        let events: Vec<_> = delivered.lock().unwrap().iter().map(|e| e.event).collect();
        assert_eq!(
            events,
            vec![
                NoteEvent::On {
                    channel: 1,
                    pitch: 60,
                    velocity: 100,
                },
                NoteEvent::On {
                    channel: 2,
                    pitch: 64,
                    velocity: 100,
                },
                NoteEvent::NotePitchBend {
                    channel: 1,
                    pitch: 60,
                    cents: 1200,
                },
                NoteEvent::NotePitchBend {
                    channel: 2,
                    pitch: 64,
                    cents: 1200,
                },
                NoteEvent::NotePitchBend {
                    channel: 1,
                    pitch: 60,
                    cents: 1400,
                },
                NoteEvent::NotePitchBend {
                    channel: 2,
                    pitch: 64,
                    cents: 1400,
                },
            ]
        );
    }

    #[test]
    fn controllers_atomic_packing_round_trips_all_fields() {
        let samples = [
            Controllers::default(),
            Controllers {
                pressure: 127,
                bend: -8192,
                wheel: 64,
                sustain: 127,
                expression: 1,
                last_cc: Some(ControlChange {
                    channel: 15,
                    cc: 127,
                    value: 127,
                }),
            },
            Controllers {
                pressure: 0,
                bend: 8191,
                wheel: 127,
                sustain: 0,
                expression: 127,
                last_cc: Some(ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 0,
                }),
            },
        ];
        for ctrl in samples {
            assert_eq!(unpack_controllers(pack_controllers(ctrl)), ctrl);
        }
    }

    #[test]
    fn lock_free_per_port_queues_deliver_in_order_and_allow_reentrant_queries_in_callbacks() {
        let held = Arc::new(HeldNoteInput::default());
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let agg_slot: Arc<std::sync::OnceLock<Arc<MultiPortAggregator>>> =
            Arc::new(std::sync::OnceLock::new());
        let observed_notes = Arc::clone(&observed);
        let agg_for_note = Arc::clone(&agg_slot);
        let agg_for_pub = Arc::clone(&agg_slot);
        let agg = Arc::new(
            MultiPortAggregator::new_mpe(
                2,
                MpeConfig::new(MpeZone::Lower { member_channels: 4 }, 48).unwrap(),
                MachTimebase::NANOSECONDS,
                Arc::clone(&held),
                Box::new(move |ctrl| {
                    // Re-entrantly reading controllers() inside publish must never deadlock.
                    if let Some(a) = agg_for_pub.get() {
                        assert_eq!(a.controllers().wheel, ctrl.wheel);
                    }
                }),
                Box::new(move |ev| {
                    // Re-entrantly querying controllers() and updating MPE ranges inside note
                    // callback must never deadlock.
                    if let Some(a) = agg_for_note.get() {
                        let _ = a.controllers();
                        let _ = a.set_mpe_pitch_bend_ranges(24, 2);
                    }
                    observed_notes.lock().unwrap().push(ev);
                }),
            )
            .unwrap(),
        );
        agg_slot.set(Arc::clone(&agg)).ok();

        // Simulate two CoreMIDI Mach threads pushing into lock-free per-port queues.
        assert!(agg.enqueue_packet(0, 100, &[0x91, 60, 100, 0xb0, 1, 77]));
        assert!(agg.enqueue_packet(1, 200, &[0x92, 64, 90]));
        assert_eq!(agg.drain_queued_ports(), 2);

        assert_eq!(agg.controllers().wheel, 77);
        let events: Vec<_> = observed.lock().unwrap().iter().map(|e| e.event).collect();
        assert_eq!(
            events,
            vec![
                NoteEvent::On {
                    channel: 1,
                    pitch: 60,
                    velocity: 100,
                },
                NoteEvent::ControlChange {
                    channel: 0,
                    cc: 1,
                    value: 77,
                },
                NoteEvent::On {
                    channel: 2,
                    pitch: 64,
                    velocity: 90,
                },
            ]
        );
    }
}
