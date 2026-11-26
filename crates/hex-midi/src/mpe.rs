//! Bounded MIDI Polyphonic Expression (MPE) zone configuration and
//! member-channel expression decoder.
//!
//! Decodes MIDI 1.0 channel-voice messages on a configured Lower or Upper MPE
//! zone without heap allocation, attributing member-channel Pitch Bend, CC74
//! (Timbre / Slide), and Channel Pressure to the active note sounding on each
//! member channel while routing master-channel messages as global expression.

use crate::ControlChange;

/// Default MPE per-note pitch bend range in semitones per the MPE specification.
pub const DEFAULT_MPE_PITCH_BEND_SEMITONES: u8 = 48;
/// Maximum supported MPE per-note pitch bend range in semitones.
pub const MAX_MPE_PITCH_BEND_SEMITONES: u8 = 96;
/// Maximum per-note pitch bend offset in cents corresponding to [`MAX_MPE_PITCH_BEND_SEMITONES`].
pub const MAX_MPE_PITCH_BEND_CENTS: i16 = (MAX_MPE_PITCH_BEND_SEMITONES as i16) * 100;
/// Default MPE master-channel pitch bend range in semitones per the MPE specification.
pub const DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES: u8 = 2;
/// Maximum supported MPE master-channel pitch bend range in semitones.
pub const MAX_MPE_MASTER_PITCH_BEND_SEMITONES: u8 = 48;

/// Configured MPE zone (zero-based MIDI channels `0..=15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpeZone {
    /// Master channel `0` (ch 1); member channels `1..=member_channels`.
    Lower { member_channels: u8 },
    /// Master channel `15` (ch 16); member channels `(15 - member_channels)..=14`.
    Upper { member_channels: u8 },
}

impl MpeZone {
    /// Standard 15-member Lower Zone (master ch 1, members ch 2..=16).
    pub const fn full_lower() -> Self {
        Self::Lower {
            member_channels: 15,
        }
    }

    /// True when `member_channels` is within `1..=15`.
    pub const fn is_valid(self) -> bool {
        match self {
            Self::Lower { member_channels } | Self::Upper { member_channels } => {
                member_channels >= 1 && member_channels <= 15
            }
        }
    }

    /// Zero-based master channel for this zone.
    pub const fn master_channel(self) -> u8 {
        match self {
            Self::Lower { .. } => 0,
            Self::Upper { .. } => 15,
        }
    }

    /// True when `channel` (`0..=15`) is a member channel of this zone.
    pub const fn is_member_channel(self, channel: u8) -> bool {
        if !self.is_valid() || channel >= 16 {
            return false;
        }
        match self {
            Self::Lower { member_channels } => channel >= 1 && channel <= member_channels,
            Self::Upper { member_channels } => channel >= (15 - member_channels) && channel <= 14,
        }
    }
}

/// Validated MPE decoding configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpeConfig {
    pub zone: MpeZone,
    /// Per-note pitch bend range on member channels, in semitones (`1..=96`, default 48).
    pub pitch_bend_range_semitones: u8,
    /// Global pitch bend range on the zone master channel, in semitones (`1..=48`, default 2).
    pub master_pitch_bend_range_semitones: u8,
}

impl Default for MpeConfig {
    fn default() -> Self {
        Self {
            zone: MpeZone::full_lower(),
            pitch_bend_range_semitones: DEFAULT_MPE_PITCH_BEND_SEMITONES,
            master_pitch_bend_range_semitones: DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES,
        }
    }
}

impl MpeConfig {
    pub const fn new(zone: MpeZone, pitch_bend_range_semitones: u8) -> Option<Self> {
        Self::with_ranges(
            zone,
            pitch_bend_range_semitones,
            DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES,
        )
    }

    /// Construct a validated MPE config with explicit bounded member (`1..=96` st)
    /// and master (`1..=48` st) pitch-bend ranges.
    pub const fn with_ranges(
        zone: MpeZone,
        pitch_bend_range_semitones: u8,
        master_pitch_bend_range_semitones: u8,
    ) -> Option<Self> {
        if !zone.is_valid()
            || pitch_bend_range_semitones == 0
            || pitch_bend_range_semitones > MAX_MPE_PITCH_BEND_SEMITONES
            || master_pitch_bend_range_semitones == 0
            || master_pitch_bend_range_semitones > MAX_MPE_MASTER_PITCH_BEND_SEMITONES
        {
            return None;
        }
        Some(Self {
            zone,
            pitch_bend_range_semitones,
            master_pitch_bend_range_semitones,
        })
    }

    /// True when zone, member bend range, and master bend range are all within bounds.
    pub const fn is_valid(self) -> bool {
        self.zone.is_valid()
            && self.pitch_bend_range_semitones >= 1
            && self.pitch_bend_range_semitones <= MAX_MPE_PITCH_BEND_SEMITONES
            && self.master_pitch_bend_range_semitones >= 1
            && self.master_pitch_bend_range_semitones <= MAX_MPE_MASTER_PITCH_BEND_SEMITONES
    }

    /// Convert a signed 14-bit MIDI pitch bend (`-8192..=8191`) into cents
    /// scaled by `pitch_bend_range_semitones`, clamped to
    /// `±MAX_MPE_PITCH_BEND_CENTS` (`-9600..=9600`).
    pub fn bend_to_cents(self, bend: i16) -> i16 {
        MpeDecoder::bend_to_cents_with_range(bend, self.pitch_bend_range_semitones)
    }

    /// Convert a signed 14-bit master-channel pitch bend (`-8192..=8191`) into cents
    /// scaled by `master_pitch_bend_range_semitones`, clamped to
    /// `±MAX_MPE_PITCH_BEND_CENTS` (`-9600..=9600`).
    pub fn master_bend_to_cents(self, bend: i16) -> i16 {
        MpeDecoder::bend_to_cents_with_range(bend, self.master_pitch_bend_range_semitones)
    }
}

/// Decoded MPE note, per-note expression, or master-zone controller event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpeExpressionEvent {
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    /// Per-note pitch offset in cents (`-4800..=4800`) attributed to the active
    /// note on `channel`.
    NotePitchBend {
        channel: u8,
        pitch: u8,
        cents: i16,
    },
    /// Per-note slide / timbre (`0..=127`, CC74) attributed to the active note
    /// on `channel`.
    NoteSlide {
        channel: u8,
        pitch: u8,
        value: u8,
    },
    /// Per-note pressure (`0..=127`, member Channel Pressure or PolyPressure)
    /// attributed to the active note on `channel`.
    NotePressure {
        channel: u8,
        pitch: u8,
        value: u8,
    },
    /// Global Control Change received on the MPE master channel (or non-CC74
    /// member CC forwarded as channel controller).
    MasterControlChange {
        channel: u8,
        cc: u8,
        value: u8,
    },
    /// Global pitch bend (`-8192..=8191`) received on the MPE master channel.
    MasterPitchBend {
        channel: u8,
        value: i16,
    },
    /// Global channel pressure (`0..=127`) received on the MPE master channel.
    MasterPressure {
        channel: u8,
        value: u8,
    },
    AllNotesOff,
}

/// Active note state tracked on one MPE member channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpeActiveNote {
    pub pitch: u8,
    pub sustained: bool,
    /// Per-note pitch bend range captured at NoteOn time so later zone bend-range
    /// changes do not retune already-held notes.
    pub pitch_bend_range_semitones: u8,
    /// Most recent member-channel per-note pitch offset in cents (`-4800..=4800`).
    pub member_bend_cents: i16,
    /// Monotonic occurrence token assigned by the decoder.
    pub occurrence_token: u64,
}

/// Zero-allocation MPE member-channel expression decoder.
///
/// Tracks per-channel note ownership, master/member CC64 sustain, per-note
/// pitch-bend range capture at NoteOn, master-channel global pitch bend,
/// duplicate same-channel retrigger release, channel reuse, and zone/system
/// resets without heap allocation.
#[derive(Clone, Debug)]
pub struct MpeDecoder {
    config: MpeConfig,
    /// Currently sounding note on each member channel (`0..=15`).
    active: [Option<MpeActiveNote>; 16],
    /// Sustain pedal (CC64 >= 64) state per channel (`0..=15`); master channel
    /// sustain holds every member channel in the zone.
    sustain_down: [bool; 16],
    /// Current zone master-channel global pitch bend in cents (`-4800..=4800`).
    master_bend_cents: i16,
    next_token: u64,
}

impl MpeDecoder {
    pub const fn new(config: MpeConfig) -> Self {
        Self {
            config,
            active: [None; 16],
            sustain_down: [false; 16],
            master_bend_cents: 0,
            next_token: 0,
        }
    }

    pub const fn config(&self) -> MpeConfig {
        self.config
    }

    /// Update the per-note pitch bend range (`1..=96` semitones) for future
    /// notes without retuning already-sounding notes on member channels.
    pub fn set_pitch_bend_range(&mut self, semitones: u8) -> bool {
        if semitones == 0 || semitones > MAX_MPE_PITCH_BEND_SEMITONES {
            return false;
        }
        self.config.pitch_bend_range_semitones = semitones;
        true
    }

    /// Update the master-channel pitch bend range (`1..=48` semitones) for
    /// future master bend messages without retuning already-sounding notes.
    pub fn set_master_pitch_bend_range(&mut self, semitones: u8) -> bool {
        if semitones == 0 || semitones > MAX_MPE_MASTER_PITCH_BEND_SEMITONES {
            return false;
        }
        self.config.master_pitch_bend_range_semitones = semitones;
        true
    }

    /// Update both member (`1..=96` st) and master (`1..=48` st) pitch bend
    /// ranges atomically without retuning already-sounding notes.
    pub fn set_pitch_bend_ranges(&mut self, member_semitones: u8, master_semitones: u8) -> bool {
        if member_semitones == 0
            || member_semitones > MAX_MPE_PITCH_BEND_SEMITONES
            || master_semitones == 0
            || master_semitones > MAX_MPE_MASTER_PITCH_BEND_SEMITONES
        {
            return false;
        }
        self.config.pitch_bend_range_semitones = member_semitones;
        self.config.master_pitch_bend_range_semitones = master_semitones;
        true
    }

    /// Current zone master-channel pitch bend offset in cents (`-4800..=4800`).
    pub const fn master_bend_cents(&self) -> i16 {
        self.master_bend_cents
    }

    /// Reconfigure the MPE zone, clearing all tracked notes and sustain state.
    pub fn set_config(&mut self, config: MpeConfig) -> bool {
        if !config.is_valid() {
            return false;
        }
        self.config = config;
        self.reset();
        true
    }

    /// Reconfigure the MPE zone without losing note-release semantics.
    ///
    /// Returns `None` on invalid configuration without mutating active notes or
    /// sustain state. On valid reconfiguration, clears tracked state and emits
    /// `MpeExpressionEvent::AllNotesOff` into `out` whenever any note or sustain
    /// pedal was active.
    pub fn reconfigure_into(
        &mut self,
        config: MpeConfig,
        out: &mut [MpeExpressionEvent; 16],
    ) -> Option<usize> {
        if !config.is_valid() {
            return None;
        }
        let had_state = self.has_active_notes() || self.sustain_down.iter().any(|&down| down);
        self.config = config;
        self.reset();
        if had_state {
            out[0] = MpeExpressionEvent::AllNotesOff;
            Some(1)
        } else {
            Some(0)
        }
    }

    /// Clear tracked active notes, sustain state, and master bend on all channels.
    pub fn reset(&mut self) {
        self.active = [None; 16];
        self.sustain_down = [false; 16];
        self.master_bend_cents = 0;
    }

    /// True when any member channel currently owns an active or sustained note.
    pub fn has_active_notes(&self) -> bool {
        self.active.iter().any(Option::is_some)
    }

    /// True when CC64 sustain is currently held on `channel` (`0..=15`).
    pub fn is_sustain_down(&self, channel: u8) -> bool {
        self.sustain_down
            .get(usize::from(channel))
            .copied()
            .unwrap_or(false)
    }

    /// Active pitch currently tracked on `channel` (`0..=15`), if any.
    pub fn active_pitch(&self, channel: u8) -> Option<u8> {
        self.active_note(channel).map(|note| note.pitch)
    }

    /// Full active note state currently tracked on `channel` (`0..=15`), if any.
    pub fn active_note(&self, channel: u8) -> Option<MpeActiveNote> {
        self.active.get(usize::from(channel)).copied().flatten()
    }

    fn channel_sustained(&self, channel: u8) -> bool {
        let master = self.config.zone.master_channel();
        self.sustain_down[usize::from(channel)] || self.sustain_down[usize::from(master)]
    }

    pub(crate) fn bend_to_cents_with_range(bend: i16, range_semitones: u8) -> i16 {
        let clamped_semitones = range_semitones.min(MAX_MPE_PITCH_BEND_SEMITONES);
        let range_cents = i64::from(clamped_semitones) * 100;
        let denom = if bend < 0 { 8192_i64 } else { 8191_i64 };
        let scaled = (i64::from(bend) * range_cents
            + if bend >= 0 { denom / 2 } else { -(denom / 2) })
            / denom;
        let limit = i64::from(MAX_MPE_PITCH_BEND_CENTS);
        scaled.clamp(-limit, limit) as i16
    }

    /// Decode one complete MIDI 1.0 channel-voice message (2 or 3 bytes) into
    /// caller-owned bounded storage (`out`), returning the number of emitted
    /// events (`0..=16`).
    ///
    /// - A duplicate `NoteOn` on an occupied member channel emits `NoteOff` for
    ///   the prior occurrence before `NoteOn` for the new pitch (no stuck notes).
    /// - A `NoteOff` while master or member CC64 sustain is held retains note
    ///   ownership (`sustained = true`) until sustain lifts or CC121 resets.
    /// - Releasing master CC64 sustain emits `NoteOff` for every sustained member
    ///   channel in deterministic channel order.
    /// - Out-of-zone, malformed, or idle-channel expression messages emit `0`.
    pub fn decode_message_into(
        &mut self,
        message: &[u8],
        out: &mut [MpeExpressionEvent; 16],
    ) -> usize {
        if !(2..=3).contains(&message.len()) {
            return 0;
        }
        let Some((&status, data)) = message.split_first() else {
            return 0;
        };
        if !(0x80..0xf0).contains(&status) || data.iter().any(|&v| v > 127) {
            return 0;
        }
        let channel = status & 0x0f;
        let kind = status & 0xf0;
        let is_master = channel == self.config.zone.master_channel();
        let is_member = self.config.zone.is_member_channel(channel);
        if !is_master && !is_member {
            return 0;
        }

        let mut count = 0_usize;
        let mut push = |event: MpeExpressionEvent| {
            if count < out.len() {
                out[count] = event;
                count += 1;
            }
        };

        match (kind, data) {
            (0x90, &[pitch, velocity]) if velocity != 0 => {
                if is_member {
                    if let Some(prior) = self.active[usize::from(channel)].take() {
                        push(MpeExpressionEvent::NoteOff {
                            channel,
                            pitch: prior.pitch,
                            velocity: 0,
                        });
                    }
                    self.next_token = self.next_token.wrapping_add(1);
                    self.active[usize::from(channel)] = Some(MpeActiveNote {
                        pitch,
                        sustained: false,
                        pitch_bend_range_semitones: self.config.pitch_bend_range_semitones,
                        member_bend_cents: 0,
                        occurrence_token: self.next_token,
                    });
                }
                push(MpeExpressionEvent::NoteOn {
                    channel,
                    pitch,
                    velocity,
                });
                if is_member && self.master_bend_cents != 0 {
                    push(MpeExpressionEvent::NotePitchBend {
                        channel,
                        pitch,
                        cents: self.master_bend_cents,
                    });
                }
            }
            (0x80 | 0x90, &[pitch, velocity]) => {
                if is_member {
                    let sustained = self.channel_sustained(channel);
                    let Some(active) = self.active[usize::from(channel)].as_mut() else {
                        // Refusing stray NoteOff for an idle channel avoids bogus transitions.
                        return 0;
                    };
                    if active.pitch != pitch {
                        return 0;
                    }
                    if sustained {
                        active.sustained = true;
                        return 0;
                    }
                    self.active[usize::from(channel)] = None;
                }
                push(MpeExpressionEvent::NoteOff {
                    channel,
                    pitch,
                    velocity,
                });
            }
            (0xa0, &[pitch, value]) => {
                if is_member && self.active[usize::from(channel)].is_none_or(|n| n.pitch != pitch) {
                    return 0;
                }
                push(MpeExpressionEvent::NotePressure {
                    channel,
                    pitch,
                    value,
                });
            }
            (0xd0, &[pressure]) => {
                if is_member {
                    let Some(note) = self.active[usize::from(channel)] else {
                        return 0;
                    };
                    push(MpeExpressionEvent::NotePressure {
                        channel,
                        pitch: note.pitch,
                        value: pressure,
                    });
                } else {
                    push(MpeExpressionEvent::MasterPressure {
                        channel,
                        value: pressure,
                    });
                }
            }
            (0xe0, &[low, high]) => {
                let bend = (i16::from(high) * 128 + i16::from(low)) - 8192;
                if is_member {
                    let Some(note) = self.active[usize::from(channel)].as_mut() else {
                        return 0;
                    };
                    let member_cents =
                        Self::bend_to_cents_with_range(bend, note.pitch_bend_range_semitones);
                    note.member_bend_cents = member_cents;
                    let limit = i32::from(MAX_MPE_PITCH_BEND_CENTS);
                    let total_cents = (i32::from(member_cents) + i32::from(self.master_bend_cents))
                        .clamp(-limit, limit) as i16;
                    push(MpeExpressionEvent::NotePitchBend {
                        channel,
                        pitch: note.pitch,
                        cents: total_cents,
                    });
                } else {
                    let master_cents = Self::bend_to_cents_with_range(
                        bend,
                        self.config.master_pitch_bend_range_semitones,
                    );
                    self.master_bend_cents = master_cents;
                    let limit = i32::from(MAX_MPE_PITCH_BEND_CENTS);
                    for ch in 0_u8..16 {
                        if self.config.zone.is_member_channel(ch)
                            && let Some(note) = self.active[usize::from(ch)]
                        {
                            let total_cents =
                                (i32::from(note.member_bend_cents) + i32::from(master_cents))
                                    .clamp(-limit, limit) as i16;
                            push(MpeExpressionEvent::NotePitchBend {
                                channel: ch,
                                pitch: note.pitch,
                                cents: total_cents,
                            });
                        }
                    }
                    push(MpeExpressionEvent::MasterPitchBend {
                        channel,
                        value: bend,
                    });
                }
            }
            (0xb0, &[cc, value]) => {
                if crate::note_events::is_all_notes_off_cc(cc) {
                    if is_master {
                        self.reset();
                        push(MpeExpressionEvent::AllNotesOff);
                    } else {
                        self.sustain_down[usize::from(channel)] = false;
                        if let Some(note) = self.active[usize::from(channel)].take() {
                            push(MpeExpressionEvent::NoteOff {
                                channel,
                                pitch: note.pitch,
                                velocity: 0,
                            });
                        }
                    }
                    return count;
                }
                if cc == ControlChange::SUSTAIN {
                    let down = value >= 64;
                    self.sustain_down[usize::from(channel)] = down;
                    if !down {
                        for ch in 0_u8..16 {
                            if !self.config.zone.is_member_channel(ch) {
                                continue;
                            }
                            if !self.channel_sustained(ch)
                                && self.active[usize::from(ch)].is_some_and(|n| n.sustained)
                                && let Some(note) = self.active[usize::from(ch)].take()
                            {
                                push(MpeExpressionEvent::NoteOff {
                                    channel: ch,
                                    pitch: note.pitch,
                                    velocity: 0,
                                });
                            }
                        }
                    }
                    if is_master {
                        push(MpeExpressionEvent::MasterControlChange { channel, cc, value });
                    }
                    return count;
                }
                if cc == ControlChange::RESET_ALL_CONTROLLERS {
                    if is_master {
                        self.sustain_down = [false; 16];
                        self.master_bend_cents = 0;
                        for ch in 0_u8..16 {
                            if self.config.zone.is_member_channel(ch)
                                && self.active[usize::from(ch)].is_some_and(|n| n.sustained)
                                && let Some(note) = self.active[usize::from(ch)].take()
                            {
                                push(MpeExpressionEvent::NoteOff {
                                    channel: ch,
                                    pitch: note.pitch,
                                    velocity: 0,
                                });
                            }
                        }
                    } else {
                        self.sustain_down[usize::from(channel)] = false;
                        if !self.channel_sustained(channel)
                            && self.active[usize::from(channel)].is_some_and(|n| n.sustained)
                            && let Some(note) = self.active[usize::from(channel)].take()
                        {
                            push(MpeExpressionEvent::NoteOff {
                                channel,
                                pitch: note.pitch,
                                velocity: 0,
                            });
                        }
                    }
                    push(MpeExpressionEvent::MasterControlChange { channel, cc, value });
                    return count;
                }
                if is_member && cc == ControlChange::TIMBRE {
                    let Some(note) = self.active[usize::from(channel)] else {
                        return 0;
                    };
                    push(MpeExpressionEvent::NoteSlide {
                        channel,
                        pitch: note.pitch,
                        value,
                    });
                    return count;
                }
                push(MpeExpressionEvent::MasterControlChange { channel, cc, value });
            }
            _ => {}
        }
        count
    }

    /// Convenience single-event decoder returning the primary decoded event.
    /// Callers requiring multi-event transitions (duplicate same-channel retrigger
    /// or master sustain pedal lift) should use [`Self::decode_message_into`].
    pub fn decode_message(&mut self, message: &[u8]) -> Option<MpeExpressionEvent> {
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];
        let count = self.decode_message_into(message, &mut out);
        if count == 0 {
            None
        } else {
            Some(out[count - 1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_zone_decodes_member_pitch_bend_timbre_pressure_and_master_cc() {
        let config = MpeConfig::new(MpeZone::Lower { member_channels: 7 }, 48).unwrap();
        let mut decoder = MpeDecoder::new(config);

        // NoteOn on member channel 1 (pitch 60) and member channel 2 (pitch 64).
        assert_eq!(
            decoder.decode_message(&[0x91, 60, 100]),
            Some(MpeExpressionEvent::NoteOn {
                channel: 1,
                pitch: 60,
                velocity: 100
            })
        );
        assert_eq!(
            decoder.decode_message(&[0x92, 64, 90]),
            Some(MpeExpressionEvent::NoteOn {
                channel: 2,
                pitch: 64,
                velocity: 90
            })
        );

        // Member ch 1 pitch bend full up (+8191) => +4800 cents for pitch 60.
        assert_eq!(
            decoder.decode_message(&[0xe1, 0x7f, 0x7f]),
            Some(MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 4800
            })
        );
        // Member ch 2 CC74 => NoteSlide for pitch 64.
        assert_eq!(
            decoder.decode_message(&[0xb2, 74, 88]),
            Some(MpeExpressionEvent::NoteSlide {
                channel: 2,
                pitch: 64,
                value: 88
            })
        );
        // Member ch 2 Channel Pressure => NotePressure for pitch 64.
        assert_eq!(
            decoder.decode_message(&[0xd2, 112]),
            Some(MpeExpressionEvent::NotePressure {
                channel: 2,
                pitch: 64,
                value: 112
            })
        );
        // Master channel 0 CC1 (mod wheel) => MasterControlChange.
        assert_eq!(
            decoder.decode_message(&[0xb0, 1, 64]),
            Some(MpeExpressionEvent::MasterControlChange {
                channel: 0,
                cc: 1,
                value: 64
            })
        );
        // Channel 10 is outside Lower { member_channels: 7 } => ignored.
        assert_eq!(decoder.decode_message(&[0x9a, 60, 100]), None);

        // NoteOff clears member ch 1 so subsequent ch 1 pitch bend does not attach to stale pitch.
        assert_eq!(
            decoder.decode_message(&[0x81, 60, 0]),
            Some(MpeExpressionEvent::NoteOff {
                channel: 1,
                pitch: 60,
                velocity: 0
            })
        );
        assert_eq!(decoder.decode_message(&[0xe1, 0x00, 0x40]), None);
    }

    #[test]
    fn upper_zone_and_invalid_configs_are_validated() {
        assert!(MpeConfig::new(MpeZone::Lower { member_channels: 0 }, 48).is_none());
        assert!(
            MpeConfig::new(
                MpeZone::Lower {
                    member_channels: 16
                },
                48
            )
            .is_none()
        );
        assert!(MpeConfig::new(MpeZone::full_lower(), 0).is_none());
        assert!(MpeConfig::new(MpeZone::full_lower(), 97).is_none());

        let upper = MpeConfig::new(MpeZone::Upper { member_channels: 3 }, 24).unwrap();
        assert_eq!(upper.zone.master_channel(), 15);
        assert!(upper.zone.is_member_channel(14));
        assert!(upper.zone.is_member_channel(12));
        assert!(!upper.zone.is_member_channel(11));

        let mut decoder = MpeDecoder::new(upper);
        assert_eq!(
            decoder.decode_message(&[0x9e, 72, 110]),
            Some(MpeExpressionEvent::NoteOn {
                channel: 14,
                pitch: 72,
                velocity: 110
            })
        );
        // Full down bend (-8192) at 24 semitones => -2400 cents.
        assert_eq!(
            decoder.decode_message(&[0xee, 0x00, 0x00]),
            Some(MpeExpressionEvent::NotePitchBend {
                channel: 14,
                pitch: 72,
                cents: -2400
            })
        );
        // Master channel 15 pressure => MasterPressure.
        assert_eq!(
            decoder.decode_message(&[0xdf, 77]),
            Some(MpeExpressionEvent::MasterPressure {
                channel: 15,
                value: 77
            })
        );
    }

    #[test]
    fn fragmented_running_status_packets_keep_two_notes_independent_and_clean_up_on_reset_or_overflow()
     {
        use crate::{Controllers, Decoder, deliver_packets_with_mpe};
        use std::cell::RefCell;

        let config = MpeConfig::new(MpeZone::Lower { member_channels: 7 }, 48).unwrap();
        let mut byte_decoder = Decoder::default();
        let mut mpe = MpeDecoder::new(config);
        let events = RefCell::new(Vec::new());

        // Packet 1 splits NoteOn ch1 (60) and NoteOn ch2 (64) across packets with interleaved
        // realtime clock (0xF8) and running status on ch1 pitch bend + ch2 CC74/pressure.
        let p1: &[u8] = &[0x91, 60, 0xf8, 100, 0x92, 64];
        let p2: &[u8] = &[
            90, 0xe1, 0x7f, 0x7f, 0x00, 0x40, // running-status ch1 bend: +4800, then 0
            0xb2, 74, 110, 0xd2, 95, // ch2 slide + pressure
            0x91, 62,
            88, // duplicate same-channel NoteOn on ch1 => releases 60 then triggers 62
        ];
        deliver_packets_with_mpe(
            &mut byte_decoder,
            &mut mpe,
            Controllers::default(),
            [p1, p2],
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );

        assert_eq!(
            *events.borrow(),
            vec![
                MpeExpressionEvent::NoteOn {
                    channel: 1,
                    pitch: 60,
                    velocity: 100
                },
                MpeExpressionEvent::NoteOn {
                    channel: 2,
                    pitch: 64,
                    velocity: 90
                },
                MpeExpressionEvent::NotePitchBend {
                    channel: 1,
                    pitch: 60,
                    cents: 4800
                },
                MpeExpressionEvent::NotePitchBend {
                    channel: 1,
                    pitch: 60,
                    cents: 0
                },
                MpeExpressionEvent::NoteSlide {
                    channel: 2,
                    pitch: 64,
                    value: 110
                },
                MpeExpressionEvent::NotePressure {
                    channel: 2,
                    pitch: 64,
                    value: 95
                },
                MpeExpressionEvent::NoteOff {
                    channel: 1,
                    pitch: 60,
                    velocity: 0
                },
                MpeExpressionEvent::NoteOn {
                    channel: 1,
                    pitch: 62,
                    velocity: 88
                },
            ]
        );
        assert_eq!(mpe.active_pitch(1), Some(62));
        assert_eq!(mpe.active_pitch(2), Some(64));

        // 0xFF System Reset clears all member notes and emits AllNotesOff.
        events.borrow_mut().clear();
        deliver_packets_with_mpe(
            &mut byte_decoder,
            &mut mpe,
            Controllers::default(),
            [&[0xff][..]],
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(*events.borrow(), vec![MpeExpressionEvent::AllNotesOff]);
        assert_eq!(mpe.active_pitch(1), None);
        assert_eq!(mpe.active_pitch(2), None);
        // Idle-channel bend is refused after reset (zero cross-talk).
        assert_eq!(mpe.decode_message(&[0xe1, 0x7f, 0x7f]), None);

        // Burst overflow past CALLBACK_PACKETS resets MPE state and emits AllNotesOff.
        events.borrow_mut().clear();
        let on = [0x91_u8, 60, 100];
        let mut flood: Vec<&[u8]> = Vec::with_capacity(1026);
        flood.push(&on);
        for _ in 0..1025 {
            flood.push(&[0xf8]);
        }
        deliver_packets_with_mpe(
            &mut byte_decoder,
            &mut mpe,
            Controllers::default(),
            flood,
            &|_| {},
            &|event| events.borrow_mut().push(event),
        );
        assert_eq!(
            events.borrow().last().copied(),
            Some(MpeExpressionEvent::AllNotesOff)
        );
        assert_eq!(mpe.active_pitch(1), None);
    }

    #[test]
    fn sustain_preserves_mpe_ownership_and_bend_range_changes_do_not_retune_held_notes() {
        let config = MpeConfig::new(MpeZone::Lower { member_channels: 4 }, 48).unwrap();
        let mut mpe = MpeDecoder::new(config);
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];

        // Start note 60 on ch 1 at 48-semitone range, then change zone bend range to 12 semitones.
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        assert!(mpe.set_pitch_bend_range(12));
        // Start note 64 on ch 2 at the new 12-semitone range.
        assert_eq!(mpe.decode_message_into(&[0x92, 64, 100], &mut out), 1);

        // Full up bend (+8191): ch 1 stays at 4800 cents, ch 2 uses 1200 cents.
        assert_eq!(
            mpe.decode_message(&[0xe1, 0x7f, 0x7f]),
            Some(MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 4800
            })
        );
        assert_eq!(
            mpe.decode_message(&[0xe2, 0x7f, 0x7f]),
            Some(MpeExpressionEvent::NotePitchBend {
                channel: 2,
                pitch: 64,
                cents: 1200
            })
        );

        // Hold master sustain (ch 0 CC64=127), release ch 1 key -> remains owned & bendable.
        assert_eq!(mpe.decode_message_into(&[0xb0, 64, 127], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0x81, 60, 0], &mut out), 0);
        assert!(mpe.active_note(1).unwrap().sustained);
        assert_eq!(
            mpe.decode_message(&[0xb1, 74, 70]),
            Some(MpeExpressionEvent::NoteSlide {
                channel: 1,
                pitch: 60,
                value: 70
            })
        );

        // Lift master sustain (ch 0 CC64=0) -> releases sustained ch 1 while ch 2 remains active.
        let n = mpe.decode_message_into(&[0xb0, 64, 0], &mut out);
        assert_eq!(n, 2);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NoteOff {
                channel: 1,
                pitch: 60,
                velocity: 0
            }
        );
        assert_eq!(mpe.active_pitch(1), None);
        assert_eq!(mpe.active_pitch(2), Some(64));

        // Zone reconfiguration resets active notes cleanly.
        assert!(mpe.set_config(MpeConfig::new(MpeZone::Upper { member_channels: 3 }, 24).unwrap()));
        assert_eq!(mpe.active_pitch(2), None);
    }

    #[test]
    fn member_channel_all_notes_off_and_overlapping_master_member_sustain_preserve_unrelated_notes()
    {
        let config = MpeConfig::new(
            MpeZone::Lower {
                member_channels: 15,
            },
            48,
        )
        .unwrap();
        let mut mpe = MpeDecoder::new(config);
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];

        // Start two notes on ch 1 (60) and ch 2 (64).
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0x92, 64, 95], &mut out), 1);

        // Idle member ch 3 CC123 (All Notes Off) must NOT kill ch 1 or ch 2!
        assert_eq!(mpe.decode_message_into(&[0xb3, 123, 0], &mut out), 0);
        assert_eq!(mpe.active_pitch(1), Some(60));
        assert_eq!(mpe.active_pitch(2), Some(64));

        // Occupied member ch 2 CC123 releases ONLY ch 2, leaving ch 1 owned.
        assert_eq!(mpe.decode_message_into(&[0xb2, 123, 0], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NoteOff {
                channel: 2,
                pitch: 64,
                velocity: 0,
            }
        );
        assert_eq!(mpe.active_pitch(1), Some(60));
        assert_eq!(mpe.active_pitch(2), None);

        // Overlapping master + member CC64 sustain on ch 1:
        assert_eq!(mpe.decode_message_into(&[0xb0, 64, 127], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0xb1, 64, 127], &mut out), 0);
        assert!(mpe.is_sustain_down(0));
        assert!(mpe.is_sustain_down(1));
        // Wrong-pitch NoteOff is ignored; matching NoteOff sustains ch 1.
        assert_eq!(mpe.decode_message_into(&[0x81, 62, 0], &mut out), 0);
        assert_eq!(mpe.decode_message_into(&[0x81, 60, 0], &mut out), 0);
        assert!(mpe.active_note(1).unwrap().sustained);
        // Lifting member CC64 while master CC64 stays down keeps ch 1 sustained!
        assert_eq!(mpe.decode_message_into(&[0xb1, 64, 0], &mut out), 0);
        assert!(mpe.active_note(1).unwrap().sustained);
        // Same-channel reuse while sustained releases old pitch 60 before triggering pitch 67.
        assert_eq!(mpe.decode_message_into(&[0x91, 67, 90], &mut out), 2);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NoteOff {
                channel: 1,
                pitch: 60,
                velocity: 0,
            }
        );
        assert_eq!(
            out[1],
            MpeExpressionEvent::NoteOn {
                channel: 1,
                pitch: 67,
                velocity: 90,
            }
        );

        // Sustain all 15 member channels simultaneously and release master CC64 -> all 15 release deterministically!
        for ch in 1_u8..=15 {
            if ch != 1 {
                let _ = mpe.decode_message_into(&[0x90 | ch, 48 + ch, 100], &mut out);
            }
            let pitch = mpe.active_pitch(ch).unwrap();
            assert_eq!(mpe.decode_message_into(&[0x80 | ch, pitch, 0], &mut out), 0);
        }
        let released = mpe.decode_message_into(&[0xb0, 64, 0], &mut out);
        assert_eq!(released, 16);
        for ch in 1_u8..=15 {
            assert!(matches!(
                out[usize::from(ch - 1)],
                MpeExpressionEvent::NoteOff { channel, .. } if channel == ch
            ));
        }
        assert_eq!(
            out[15],
            MpeExpressionEvent::MasterControlChange {
                channel: 0,
                cc: 64,
                value: 0,
            }
        );
        assert!(!mpe.has_active_notes());

        // reconfigure_into refuses invalid config without disturbing an active note,
        // and emits AllNotesOff on valid zone switch.
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        assert_eq!(
            mpe.reconfigure_into(
                MpeConfig {
                    zone: MpeZone::Lower { member_channels: 0 },
                    pitch_bend_range_semitones: 48,
                    master_pitch_bend_range_semitones: 2,
                },
                &mut out,
            ),
            None
        );
        assert_eq!(mpe.active_pitch(1), Some(60));
        assert_eq!(
            mpe.reconfigure_into(
                MpeConfig::new(MpeZone::Upper { member_channels: 4 }, 48).unwrap(),
                &mut out,
            ),
            Some(1)
        );
        assert_eq!(out[0], MpeExpressionEvent::AllNotesOff);
        assert!(!mpe.has_active_notes());
    }

    #[test]
    fn master_and_member_bend_ranges_combine_per_note_and_live_updates_do_not_retune_held_notes() {
        assert!(MpeConfig::with_ranges(MpeZone::full_lower(), 48, 0).is_none());
        assert!(MpeConfig::with_ranges(MpeZone::full_lower(), 48, 49).is_none());
        assert!(MpeConfig::with_ranges(MpeZone::full_lower(), 0, 2).is_none());
        assert!(MpeConfig::with_ranges(MpeZone::full_lower(), 97, 2).is_none());

        let config = MpeConfig::with_ranges(MpeZone::Lower { member_channels: 4 }, 48, 12).unwrap();
        let mut mpe = MpeDecoder::new(config);
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];

        // Start ch 1 pitch 60 (+1200c member bend -> quarter of +4800 = +2048 -> raw 10240 = 0x50 0x00)
        // and ch 2 pitch 64 (-600c member bend).
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0x92, 64, 100], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0xe1, 0x00, 0x50], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 1200,
            }
        );
        assert_eq!(mpe.decode_message_into(&[0xe2, 0x00, 0x38], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 2,
                pitch: 64,
                cents: -600,
            }
        );

        // Apply master bend full up (+8191) at master range 12 st (+1200c):
        // ch 1 becomes 1200 + 1200 = +2400c, ch 2 becomes -600 + 1200 = +600c!
        let n = mpe.decode_message_into(&[0xe0, 0x7f, 0x7f], &mut out);
        assert_eq!(n, 3);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 2400,
            }
        );
        assert_eq!(
            out[1],
            MpeExpressionEvent::NotePitchBend {
                channel: 2,
                pitch: 64,
                cents: 600,
            }
        );
        assert_eq!(
            out[2],
            MpeExpressionEvent::MasterPitchBend {
                channel: 0,
                value: 8191,
            }
        );

        // Live range change (member 48 -> 12, master 12 -> 2) rejects invalid values and
        // does NOT retune already-held notes or clear their captured member bend ranges!
        assert!(!mpe.set_pitch_bend_ranges(0, 2));
        assert!(!mpe.set_pitch_bend_ranges(12, 49));
        assert!(mpe.set_pitch_bend_ranges(12, 2));
        assert_eq!(mpe.master_bend_cents(), 1200);
        // Existing ch 1 still uses its NoteOn-captured 48 st range when bent to full down (-8192):
        // member = -4800c, master = +1200c => -3600c!
        assert_eq!(mpe.decode_message_into(&[0xe1, 0x00, 0x00], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: -3600,
            }
        );

        // Same-channel reuse on ch 1 (pitch 67) clears old member bend (-4800c),
        // captures new 12 st member range, and starts with active master bend (+1200c).
        let reuse = mpe.decode_message_into(&[0x91, 67, 96], &mut out);
        assert_eq!(reuse, 3);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NoteOff {
                channel: 1,
                pitch: 60,
                velocity: 0,
            }
        );
        assert_eq!(
            out[1],
            MpeExpressionEvent::NoteOn {
                channel: 1,
                pitch: 67,
                velocity: 96,
            }
        );
        assert_eq!(
            out[2],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 67,
                cents: 1200,
            }
        );
        // Full down member bend on new ch 1 note uses the updated 12 st range (-1200c + 1200c master = 0c)!
        assert_eq!(mpe.decode_message_into(&[0xe1, 0x00, 0x00], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 67,
                cents: 0,
            }
        );
    }

    #[test]
    fn bend_to_cents_supports_full_96_semitone_range_without_4800_clamp() {
        let config = MpeConfig::new(MpeZone::Lower { member_channels: 4 }, 96).unwrap();
        assert_eq!(config.bend_to_cents(8191), 9600);
        assert_eq!(config.bend_to_cents(-8192), -9600);
        assert_eq!(config.bend_to_cents(0), 0);

        let mut mpe = MpeDecoder::new(config);
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        assert_eq!(mpe.decode_message_into(&[0xe1, 0x7f, 0x7f], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: 9600,
            }
        );
        assert_eq!(mpe.decode_message_into(&[0xe1, 0x00, 0x00], &mut out), 1);
        assert_eq!(
            out[0],
            MpeExpressionEvent::NotePitchBend {
                channel: 1,
                pitch: 60,
                cents: -9600,
            }
        );
    }

    #[test]
    fn idle_member_cc120_and_cc123_to_127_emit_zero_events_without_disturbing_active_members() {
        let config = MpeConfig::new(MpeZone::Lower { member_channels: 4 }, 48).unwrap();
        let mut mpe = MpeDecoder::new(config);
        let mut out = [MpeExpressionEvent::AllNotesOff; 16];
        assert_eq!(mpe.decode_message_into(&[0x91, 60, 100], &mut out), 1);
        for cc in [120_u8, 123, 124, 125, 126, 127] {
            assert_eq!(
                mpe.decode_message_into(&[0xb2, cc, 0], &mut out),
                0,
                "idle member CC{cc} must emit zero events"
            );
            assert_eq!(mpe.active_pitch(1), Some(60));
        }
    }

    #[test]
    fn mpe_burst_overflow_resets_controllers_and_clears_decoder_held_notes() {
        use crate::{Controllers, Decoder, deliver_packets_with_mpe};
        use std::cell::RefCell;

        let mut decoder = Decoder::default();
        let mut mpe = MpeDecoder::new(MpeConfig::default());
        let published = RefCell::new(Vec::new());
        let events = RefCell::new(Vec::new());

        let initial = deliver_packets_with_mpe(
            &mut decoder,
            &mut mpe,
            Controllers::default(),
            [&[0x91, 60, 100, 0xb0, 1, 96][..]],
            &|ctrl| published.borrow_mut().push(ctrl),
            &|ev| events.borrow_mut().push(ev),
        );
        assert_eq!(initial.wheel, 96);
        assert_eq!(decoder.held_notes().after(0).count(), 1);
        let _ = decoder.take_held_notes();
        published.borrow_mut().clear();
        events.borrow_mut().clear();

        let burst = vec![&[0xf8_u8][..]; 1025];
        let after = deliver_packets_with_mpe(
            &mut decoder,
            &mut mpe,
            initial,
            burst,
            &|ctrl| published.borrow_mut().push(ctrl),
            &|ev| events.borrow_mut().push(ev),
        );
        assert_eq!(after, Controllers::default());
        assert_eq!(*published.borrow(), vec![Controllers::default()]);
        assert_eq!(
            events.borrow().last(),
            Some(&MpeExpressionEvent::AllNotesOff)
        );
        assert!(!mpe.has_active_notes());
        let cleared = decoder
            .take_held_notes()
            .expect("burst overflow must mark decoder held notes dirty");
        assert_eq!(cleared.after(0).count(), 0);
    }
}
