//! Bounded multi-port MIDI input aggregation with shared monotonic held-note
//! tokens and isolated per-port framing and overflow recovery.

use crate::framing::{ChannelFilter, Controllers, Decoder, for_each_bounded_packet};
use crate::held_notes::{HeldNoteInput, HeldNotes};
use crate::mpe::{
    MAX_MPE_MASTER_PITCH_BEND_SEMITONES, MAX_MPE_PITCH_BEND_SEMITONES, MpeConfig, MpeDecoder,
    MpeExpressionEvent,
};
use crate::note_events::NoteEvent;
use crate::timebase::{MachTimebase, MonotonicTimestampClock, TimedPacket, TimestampedNoteEvent};
use std::sync::Arc;

/// Maximum number of MIDI input ports aggregated into one logical live input stream.
pub const MAX_AGGREGATED_PORTS: usize = 16;

struct AggregatedState {
    decoders: Vec<Decoder>,
    mpe_decoders: Vec<MpeDecoder>,
    current: Controllers,
    clock: MonotonicTimestampClock,
    merged_through: u64,
    publish: Box<dyn Fn(Controllers) + Send>,
    note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
}

/// Bounded multi-port MIDI input aggregator (up to [`MAX_AGGREGATED_PORTS`] ports).
///
/// Each port retains its own isolated [`Decoder`] (and optional [`MpeDecoder`])
/// so running status, SysEx framing, MPE per-channel note ownership, and
/// per-port burst overflow never bleed across physical inputs. One control-side
/// mutex serializes merged controller snapshots, monotonic cross-port timestamps,
/// a single monotonic held-note token counter, ordered note/CC/per-note expression
/// delivery, and sole-writer publication into [`HeldNoteInput`].
pub struct MultiPortAggregator {
    filter: ChannelFilter,
    mpe: Option<MpeConfig>,
    timebase: MachTimebase,
    notes: Arc<HeldNoteInput>,
    state: std::sync::Mutex<AggregatedState>,
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
        for _ in 0..port_count {
            decoders.push(Decoder::default());
        }
        Ok(Self {
            filter,
            mpe: None,
            timebase,
            notes,
            state: std::sync::Mutex::new(AggregatedState {
                decoders,
                mpe_decoders: Vec::new(),
                current: Controllers::default(),
                clock: MonotonicTimestampClock::default(),
                merged_through: 0,
                publish,
                note,
            }),
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
        for _ in 0..port_count {
            decoders.push(Decoder::default());
            mpe_decoders.push(MpeDecoder::new(config));
        }
        Ok(Self {
            filter: ChannelFilter::Omni,
            mpe: Some(config),
            timebase,
            notes,
            state: std::sync::Mutex::new(AggregatedState {
                decoders,
                mpe_decoders,
                current: Controllers::default(),
                clock: MonotonicTimestampClock::default(),
                merged_through: 0,
                publish,
                note,
            }),
        })
    }

    /// Deliver one batch of timestamped packets from `port_index` into its isolated
    /// decoder and publish merged held-note / controller / ordered-event updates.
    pub fn deliver_timed_packets<'a>(
        &self,
        port_index: usize,
        packets: impl IntoIterator<Item = TimedPacket<'a>>,
    ) {
        if self.notes.is_cancelled() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if self.notes.is_cancelled() || port_index >= state.decoders.len() {
            return;
        }
        let port_count = state.decoders.len();
        let AggregatedState {
            decoders,
            mpe_decoders,
            current,
            clock,
            merged_through,
            publish,
            note,
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
            let emit = |event| {
                note(TimestampedNoteEvent {
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
                        publish(*current);
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
                        publish(*current);
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
                        publish(*current);
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
        if faulted {
            return;
        }
        if let Some(fault) = decoder.held_note_fault() {
            self.notes.fail(fault);
            return;
        }
        if let Some(port_snapshot) = decoder.take_held_notes() {
            *merged_through = (*merged_through).max(port_snapshot.through());
            let merged =
                HeldNotes::merge_from(decoders.iter().map(Decoder::held_notes), *merged_through);
            self.notes.publish(merged);
        }
    }

    pub fn controllers(&self) -> Controllers {
        self.state
            .lock()
            .map(|state| state.current)
            .unwrap_or_default()
    }

    /// Update member (`1..=96` st) and master (`1..=48` st) MPE pitch-bend ranges
    /// live across every aggregated port without resetting or retuning active notes.
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
        let Ok(mut state) = self.state.lock() else {
            return Err("controller aggregator state is unavailable");
        };
        for decoder in &mut state.mpe_decoders {
            let _ = decoder.set_pitch_bend_ranges(member_semitones, master_semitones);
        }
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
}
