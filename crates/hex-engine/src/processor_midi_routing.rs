//! Bounded downstream routing of MIDI events emitted by hosted VST3 processors.
//!
//! Preserves existing authored track MIDI routing (`hex_project::Command::SetMidiSource`)
//! while providing explicit `(source_track, device_id)` source identity for processor-emitted
//! output events, sample-offset-ordered quantum delivery, an explicit orphan/overlap/release
//! policy, strict binary snapshot persistence (`HXPM` v1), and zero callback allocations.

use hex_lattice::{
    ForeignNote, ForeignNoteKind, ForeignOutputEvents, ForeignProcessorNode, ForeignProcessors,
};
use hex_lattice_contract::DeviceContractError;
use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU32;

/// Maximum processor-output MIDI routes in one project.
pub const MAX_PROCESSOR_MIDI_ROUTES: usize = 64;
/// Fixed active-note ledger capacity per route (zero heap allocation on callback).
pub const MAX_ACTIVE_ROUTED_NOTES: usize = 128;

const SNAPSHOT_MAGIC: &[u8; 4] = b"HXPM";
const SNAPSHOT_VERSION: u8 = 1;

/// Explicit identity of a hosted processor emitting MIDI output events.
/// Distinct from authored track clips so input notes are never echoed or doubled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessorMidiSource {
    pub track_id: u64,
    pub device_id: u64,
}

/// Authored route from one processor MIDI output source to a receiver track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorMidiRoute {
    pub source: ProcessorMidiSource,
    pub destination_track: u64,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessorMidiRoutingError {
    SelfRoute,
    DuplicateRoute,
    TooManyRoutes,
    CycleDetected,
    InvalidEventOffset,
    UnorderedEvents,
    OutputBufferOverflow,
    MalformedSnapshot,
    Foreign(DeviceContractError),
}

impl fmt::Display for ProcessorMidiRoutingError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::SelfRoute => "processor midi output cannot route to its own track",
            Self::DuplicateRoute => "duplicate processor midi output route",
            Self::TooManyRoutes => "processor midi output route count exceeds capacity",
            Self::CycleDetected => "processor midi output routing cycle is unsupported",
            Self::InvalidEventOffset => "processor midi output event offset exceeds block length",
            Self::UnorderedEvents => "processor midi output events are not ordered by frame offset",
            Self::OutputBufferOverflow => "processor midi destination output buffer overflowed",
            Self::MalformedSnapshot => "processor midi route snapshot is malformed",
            Self::Foreign(_) => "foreign processor midi output unavailable",
        })
    }
}

impl std::error::Error for ProcessorMidiRoutingError {}

/// Validated, cycle-free table of processor MIDI output routes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessorMidiRouteTable {
    routes: Vec<ProcessorMidiRoute>,
}

impl ProcessorMidiRouteTable {
    pub fn new(routes: Vec<ProcessorMidiRoute>) -> Result<Self, ProcessorMidiRoutingError> {
        if routes.len() > MAX_PROCESSOR_MIDI_ROUTES {
            return Err(ProcessorMidiRoutingError::TooManyRoutes);
        }
        let mut seen = BTreeSet::new();
        for route in &routes {
            if route.source.track_id == route.destination_track {
                return Err(ProcessorMidiRoutingError::SelfRoute);
            }
            if !seen.insert((route.source, route.destination_track)) {
                return Err(ProcessorMidiRoutingError::DuplicateRoute);
            }
        }
        // Reject directed track cycles across processor-output routes.
        for start in &routes {
            let mut frontier = vec![start.destination_track];
            let mut visited = BTreeSet::new();
            while let Some(track) = frontier.pop() {
                if track == start.source.track_id {
                    return Err(ProcessorMidiRoutingError::CycleDetected);
                }
                if !visited.insert(track) {
                    continue;
                }
                for next in &routes {
                    if next.source.track_id == track {
                        frontier.push(next.destination_track);
                    }
                }
            }
        }
        let mut sorted = routes;
        sorted.sort_by_key(|route| (route.source, route.destination_track));
        Ok(Self { routes: sorted })
    }

    pub fn routes(&self) -> &[ProcessorMidiRoute] {
        &self.routes
    }

    pub fn with_route(
        &self,
        source: ProcessorMidiSource,
        destination_track: u64,
        enabled: bool,
    ) -> Result<Self, ProcessorMidiRoutingError> {
        let mut next = self.routes.clone();
        next.retain(|route| {
            !(route.source == source && route.destination_track == destination_track)
        });
        next.push(ProcessorMidiRoute {
            source,
            destination_track,
            enabled,
        });
        Self::new(next)
    }

    pub fn without_route(&self, source: ProcessorMidiSource, destination_track: u64) -> Self {
        let mut next = self.routes.clone();
        next.retain(|route| {
            !(route.source == source && route.destination_track == destination_track)
        });
        Self { routes: next }
    }

    /// Deterministic binary snapshot encoding (`HXPM` v1) independent of authored
    /// `Command::SetMidiSource` routes so existing authored routes stay intact on reopen.
    pub fn encode_snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.routes.len() * 25);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.extend_from_slice(&(self.routes.len() as u16).to_le_bytes());
        for route in &self.routes {
            out.extend_from_slice(&route.source.track_id.to_le_bytes());
            out.extend_from_slice(&route.source.device_id.to_le_bytes());
            out.extend_from_slice(&route.destination_track.to_le_bytes());
            out.push(u8::from(route.enabled));
        }
        out
    }

    /// Strict snapshot decoder rejecting truncated, trailing, self-routing,
    /// cyclic, or duplicate records.
    pub fn decode_snapshot(bytes: &[u8]) -> Result<Self, ProcessorMidiRoutingError> {
        let header = bytes
            .get(..7)
            .ok_or(ProcessorMidiRoutingError::MalformedSnapshot)?;
        if &header[..4] != SNAPSHOT_MAGIC || header[4] != SNAPSHOT_VERSION {
            return Err(ProcessorMidiRoutingError::MalformedSnapshot);
        }
        let count = usize::from(u16::from_le_bytes([header[5], header[6]]));
        let expected_len = 7usize
            .checked_add(
                count
                    .checked_mul(25)
                    .ok_or(ProcessorMidiRoutingError::MalformedSnapshot)?,
            )
            .ok_or(ProcessorMidiRoutingError::MalformedSnapshot)?;
        if bytes.len() != expected_len {
            return Err(ProcessorMidiRoutingError::MalformedSnapshot);
        }
        let mut routes = Vec::with_capacity(count);
        for index in 0..count {
            let start = 7 + index * 25;
            let rec = &bytes[start..start + 25];
            let track_id = u64::from_le_bytes(
                rec[0..8]
                    .try_into()
                    .map_err(|_| ProcessorMidiRoutingError::MalformedSnapshot)?,
            );
            let device_id = u64::from_le_bytes(
                rec[8..16]
                    .try_into()
                    .map_err(|_| ProcessorMidiRoutingError::MalformedSnapshot)?,
            );
            let destination_track = u64::from_le_bytes(
                rec[16..24]
                    .try_into()
                    .map_err(|_| ProcessorMidiRoutingError::MalformedSnapshot)?,
            );
            let enabled = match rec[24] {
                0 => false,
                1 => true,
                _ => return Err(ProcessorMidiRoutingError::MalformedSnapshot),
            };
            routes.push(ProcessorMidiRoute {
                source: ProcessorMidiSource {
                    track_id,
                    device_id,
                },
                destination_track,
                enabled,
            });
        }
        Self::new(routes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeldNote {
    channel: u8,
    pitch: u8,
    note_id: Option<NonZeroU32>,
}

/// Fixed-capacity per-route ledger implementing the explicit orphan, overlap,
/// and release-flush policy with zero heap allocation on the callback lane.
#[derive(Clone)]
struct RouteSlot {
    route: ProcessorMidiRoute,
    held: [Option<HeldNote>; MAX_ACTIVE_ROUTED_NOTES],
    held_count: usize,
    pending: ForeignOutputEvents,
}

impl RouteSlot {
    fn new(route: ProcessorMidiRoute) -> Self {
        Self {
            route,
            held: [None; MAX_ACTIVE_ROUTED_NOTES],
            held_count: 0,
            pending: ForeignOutputEvents::new(),
        }
    }

    fn find_held_index(
        &self,
        channel: u8,
        pitch: u8,
        note_id: Option<NonZeroU32>,
    ) -> Option<usize> {
        if let Some(id) = note_id
            && let Some(pos) = self
                .held
                .iter()
                .position(|slot| slot.is_some_and(|h| h.note_id == Some(id)))
        {
            return Some(pos);
        }
        self.held
            .iter()
            .position(|slot| slot.is_some_and(|h| h.channel == channel && h.pitch == pitch))
    }

    fn insert_held(&mut self, note: HeldNote) -> bool {
        for slot in &mut self.held {
            if slot.is_none() {
                *slot = Some(note);
                self.held_count += 1;
                return true;
            }
        }
        false
    }

    fn remove_held(&mut self, index: usize) -> Option<HeldNote> {
        let taken = self.held.get_mut(index)?.take()?;
        self.held_count = self.held_count.saturating_sub(1);
        Some(taken)
    }

    fn ingest_event(&mut self, note: ForeignNote) -> Result<(), ProcessorMidiRoutingError> {
        match note.kind {
            ForeignNoteKind::On => {
                // Overlap policy: if the same (channel, pitch) is already held
                // without a distinct note_id, emit a deterministic Note-Off
                // before re-triggering so receivers never stick voices.
                let same_pitch_idx = self.held.iter().position(|slot| {
                    slot.is_some_and(|h| {
                        h.channel == note.channel
                            && h.pitch == note.pitch
                            && (note.note_id.is_none() || h.note_id == note.note_id)
                    })
                });
                if let Some(idx) = same_pitch_idx
                    && let Some(prev) = self.remove_held(idx)
                {
                    let release = ForeignNote {
                        frame: note.frame,
                        channel: prev.channel,
                        pitch: prev.pitch,
                        kind: ForeignNoteKind::Off,
                        value: 0,
                        note_id: prev.note_id,
                        is_live: note.is_live,
                    };
                    if !self.pending.push(release) {
                        return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                    }
                }
                if !self.insert_held(HeldNote {
                    channel: note.channel,
                    pitch: note.pitch,
                    note_id: note.note_id,
                }) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
                if !self.pending.push(note) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
            }
            ForeignNoteKind::Off => {
                // Orphan policy: drop Note-Off for notes not currently held on this route.
                let Some(idx) = self.find_held_index(note.channel, note.pitch, note.note_id) else {
                    return Ok(());
                };
                self.remove_held(idx);
                if !self.pending.push(note) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
            }
            ForeignNoteKind::PolyPressure => {
                // Orphan policy: drop per-note pressure when no matching note is held.
                if self
                    .find_held_index(note.channel, note.pitch, note.note_id)
                    .is_none()
                {
                    return Ok(());
                }
                if !self.pending.push(note) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
            }
            ForeignNoteKind::ControlChange
            | ForeignNoteKind::PitchBend
            | ForeignNoteKind::ChannelPressure => {
                if !self.pending.push(note) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
            }
        }
        Ok(())
    }

    fn flush_releases_into(
        &mut self,
        out: &mut ForeignOutputEvents,
    ) -> Result<usize, ProcessorMidiRoutingError> {
        let mut flushed = 0;
        for slot in &mut self.held {
            if let Some(held) = slot.take() {
                self.held_count = self.held_count.saturating_sub(1);
                let release = ForeignNote {
                    frame: 0,
                    channel: held.channel,
                    pitch: held.pitch,
                    kind: ForeignNoteKind::Off,
                    value: 0,
                    note_id: held.note_id,
                    is_live: false,
                };
                if !out.push(release) {
                    return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
                }
                flushed += 1;
            }
        }
        Ok(flushed)
    }
}

/// Control-prepared, allocation-free processor MIDI output router.
pub struct ProcessorMidiRouter {
    slots: Vec<RouteSlot>,
    scratch: ForeignOutputEvents,
}

impl ProcessorMidiRouter {
    /// Prepare fixed storage on control from a validated route table.
    pub fn prepare(table: &ProcessorMidiRouteTable) -> Self {
        let slots = table.routes().iter().copied().map(RouteSlot::new).collect();
        Self {
            slots,
            scratch: ForeignOutputEvents::new(),
        }
    }

    /// Clear per-block pending buffers at the start of each physical callback
    /// while retaining held-note ledgers across blocks.
    pub fn begin_block(&mut self) {
        self.scratch.clear();
        for slot in &mut self.slots {
            slot.pending.clear();
        }
    }

    /// Drain emitted MIDI events directly from a `ForeignProcessors` endpoint
    /// for `source` and route them to subscribed destinations without echoing
    /// track input clips or doubling events.
    pub fn collect_from_foreign(
        &mut self,
        source: ProcessorMidiSource,
        node: ForeignProcessorNode<'_>,
        foreign: &mut dyn ForeignProcessors,
        block_frames: usize,
    ) -> Result<usize, ProcessorMidiRoutingError> {
        self.scratch.clear();
        let count = foreign
            .drain_output_notes(node, &mut self.scratch)
            .map_err(ProcessorMidiRoutingError::Foreign)?;
        if count == 0 {
            return Ok(0);
        }
        let scratch = self.scratch;
        self.ingest_source_events(source, scratch.as_slice(), block_frames)
    }

    /// Validate sample-offset ordering and route committed output events from
    /// `source` to every enabled destination subscribed to that exact source.
    pub fn ingest_source_events(
        &mut self,
        source: ProcessorMidiSource,
        events: &[ForeignNote],
        block_frames: usize,
    ) -> Result<usize, ProcessorMidiRoutingError> {
        let mut last_frame = 0_u16;
        for (index, &event) in events.iter().enumerate() {
            if usize::from(event.frame) >= block_frames {
                return Err(ProcessorMidiRoutingError::InvalidEventOffset);
            }
            if index > 0 && event.frame < last_frame {
                return Err(ProcessorMidiRoutingError::UnorderedEvents);
            }
            last_frame = event.frame;
        }
        let mut routed = 0;
        for slot in &mut self.slots {
            if !slot.route.enabled || slot.route.source != source {
                continue;
            }
            for &event in events {
                slot.ingest_event(event)?;
                routed += 1;
            }
        }
        Ok(routed)
    }

    /// Deliver all pending routed events targeting `destination_track` in
    /// deterministic non-decreasing sample-offset order into `out`.
    pub fn deliver_to_destination(
        &mut self,
        destination_track: u64,
        out: &mut ForeignOutputEvents,
    ) -> Result<usize, ProcessorMidiRoutingError> {
        // Merge in sample-offset order without heap allocation using a bounded
        // inline cursor array (at most MAX_PROCESSOR_MIDI_ROUTES slots).
        let mut cursors = [0_usize; MAX_PROCESSOR_MIDI_ROUTES];
        let mut delivered = 0;
        loop {
            let mut best_slot: Option<usize> = None;
            let mut best_frame = u16::MAX;
            for (slot_index, slot) in self.slots.iter().enumerate() {
                if slot.route.destination_track != destination_track {
                    continue;
                }
                let cursor = cursors[slot_index];
                if let Some(&event) = slot.pending.as_slice().get(cursor)
                    && (best_slot.is_none() || event.frame < best_frame)
                {
                    best_slot = Some(slot_index);
                    best_frame = event.frame;
                }
            }
            let Some(chosen) = best_slot else {
                break;
            };
            let event = self.slots[chosen].pending.as_slice()[cursors[chosen]];
            cursors[chosen] += 1;
            if !out.push(event) {
                return Err(ProcessorMidiRoutingError::OutputBufferOverflow);
            }
            delivered += 1;
        }
        for slot in &mut self.slots {
            if slot.route.destination_track == destination_track {
                slot.pending.clear();
            }
        }
        Ok(delivered)
    }

    /// Emit deterministic frame-0 Note-Off releases for all held notes routed
    /// from `source` to `destination_track` (e.g., on Stop, Seek, route removal,
    /// or source revocation).
    pub fn flush_releases_for_destination(
        &mut self,
        destination_track: u64,
        out: &mut ForeignOutputEvents,
    ) -> Result<usize, ProcessorMidiRoutingError> {
        let mut total = 0;
        for slot in &mut self.slots {
            if slot.route.destination_track == destination_track {
                total += slot.flush_releases_into(out)?;
                slot.pending.clear();
            }
        }
        Ok(total)
    }

    /// Count currently held notes routed to `destination_track`.
    pub fn held_note_count(&self, destination_track: u64) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.route.destination_track == destination_track)
            .map(|slot| slot.held_count)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(
        frame: u16,
        pitch: u8,
        kind: ForeignNoteKind,
        value: u8,
        id: Option<u32>,
    ) -> ForeignNote {
        ForeignNote {
            frame,
            channel: 0,
            pitch,
            kind,
            value,
            note_id: id.and_then(NonZeroU32::new),
            is_live: false,
        }
    }

    #[test]
    fn route_table_validates_cycles_duplicates_self_routes_and_snapshot_round_trip() {
        let src_a = ProcessorMidiSource {
            track_id: 10,
            device_id: 101,
        };
        let src_b = ProcessorMidiSource {
            track_id: 20,
            device_id: 202,
        };
        // Self-route refused.
        assert_eq!(
            ProcessorMidiRouteTable::default().with_route(src_a, 10, true),
            Err(ProcessorMidiRoutingError::SelfRoute)
        );
        // Valid chain 10 -> 20 -> 30 succeeds; closing 30 -> 10 refuses cycle.
        let table = ProcessorMidiRouteTable::default()
            .with_route(src_a, 20, true)
            .unwrap()
            .with_route(src_b, 30, true)
            .unwrap();
        let src_c = ProcessorMidiSource {
            track_id: 30,
            device_id: 303,
        };
        assert_eq!(
            table.with_route(src_c, 10, true),
            Err(ProcessorMidiRoutingError::CycleDetected)
        );
        // Deterministic HXPM v1 snapshot round trip + truncation/tamper refusal.
        let encoded = table.encode_snapshot();
        assert_eq!(
            ProcessorMidiRouteTable::decode_snapshot(&encoded).unwrap(),
            table
        );
        for len in 0..encoded.len() {
            assert!(ProcessorMidiRouteTable::decode_snapshot(&encoded[..len]).is_err());
        }
        let mut bad_magic = encoded.clone();
        bad_magic[0] = b'X';
        assert!(ProcessorMidiRouteTable::decode_snapshot(&bad_magic).is_err());
        // Preserve existing authored Command::SetMidiSource routes independently.
        let removed = table.without_route(src_a, 20);
        assert_eq!(removed.routes().len(), 1);
        assert_eq!(removed.routes()[0].source, src_b);
    }

    #[test]
    fn processor_midi_router_delivers_emitted_gate_without_echoing_input_or_doubling() {
        let arp = ProcessorMidiSource {
            track_id: 1,
            device_id: 42,
        };
        let unrelated_device = ProcessorMidiSource {
            track_id: 1,
            device_id: 99,
        };
        let table = ProcessorMidiRouteTable::default()
            .with_route(arp, 2, true)
            .unwrap();
        let mut router = ProcessorMidiRouter::prepare(&table);
        router.begin_block();

        // An unsubscribed device on the same track must not reach track 2.
        let ignored = router
            .ingest_source_events(
                unrelated_device,
                &[note(4, 60, ForeignNoteKind::On, 100, Some(1))],
                256,
            )
            .unwrap();
        assert_eq!(ignored, 0);

        // Emit an independent gate (pitch 72, offset 17) from `arp`, plus an
        // orphaned NoteOff (pitch 48) and orphaned PolyPressure (pitch 48) which
        // must be dropped without reaching track 2.
        router
            .ingest_source_events(
                arp,
                &[
                    note(5, 48, ForeignNoteKind::Off, 0, Some(99)),
                    note(9, 48, ForeignNoteKind::PolyPressure, 64, Some(99)),
                    note(17, 72, ForeignNoteKind::On, 110, Some(7)),
                    note(40, 72, ForeignNoteKind::PolyPressure, 80, Some(7)),
                ],
                256,
            )
            .unwrap();

        let mut delivered = ForeignOutputEvents::new();
        assert_eq!(router.deliver_to_destination(2, &mut delivered).unwrap(), 2);
        assert_eq!(delivered.as_slice()[0].frame, 17);
        assert_eq!(delivered.as_slice()[0].pitch, 72);
        assert_eq!(delivered.as_slice()[0].kind, ForeignNoteKind::On);
        assert_eq!(delivered.as_slice()[1].frame, 40);
        assert_eq!(delivered.as_slice()[1].kind, ForeignNoteKind::PolyPressure);
        assert_eq!(router.held_note_count(2), 1);

        // Overlap re-trigger on pitch 72 without distinct note_id emits a
        // deterministic NoteOff before the new NoteOn at the same sample offset.
        router.begin_block();
        router
            .ingest_source_events(arp, &[note(12, 72, ForeignNoteKind::On, 90, None)], 256)
            .unwrap();
        delivered.clear();
        assert_eq!(router.deliver_to_destination(2, &mut delivered).unwrap(), 2);
        assert_eq!(delivered.as_slice()[0].frame, 12);
        assert_eq!(delivered.as_slice()[0].kind, ForeignNoteKind::Off);
        assert_eq!(delivered.as_slice()[1].frame, 12);
        assert_eq!(delivered.as_slice()[1].kind, ForeignNoteKind::On);
        assert_eq!(router.held_note_count(2), 1);

        // Stop/Seek/removal flushes deterministic NoteOff at frame 0.
        delivered.clear();
        assert_eq!(
            router
                .flush_releases_for_destination(2, &mut delivered)
                .unwrap(),
            1
        );
        assert_eq!(delivered.as_slice()[0].frame, 0);
        assert_eq!(delivered.as_slice()[0].pitch, 72);
        assert_eq!(delivered.as_slice()[0].kind, ForeignNoteKind::Off);
        assert_eq!(router.held_note_count(2), 0);

        // Reject out-of-range and unordered events.
        assert_eq!(
            router
                .ingest_source_events(arp, &[note(256, 60, ForeignNoteKind::On, 100, None)], 256,),
            Err(ProcessorMidiRoutingError::InvalidEventOffset)
        );
        assert_eq!(
            router.ingest_source_events(
                arp,
                &[
                    note(20, 60, ForeignNoteKind::On, 100, None),
                    note(10, 60, ForeignNoteKind::Off, 0, None),
                ],
                256,
            ),
            Err(ProcessorMidiRoutingError::UnorderedEvents)
        );
        const { assert!(hex_lattice::MAX_FOREIGN_OUTPUT_NOTES >= 64) };
    }
}
