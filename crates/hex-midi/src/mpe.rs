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
}

impl Default for MpeConfig {
    fn default() -> Self {
        Self {
            zone: MpeZone::full_lower(),
            pitch_bend_range_semitones: DEFAULT_MPE_PITCH_BEND_SEMITONES,
        }
    }
}

impl MpeConfig {
    pub const fn new(zone: MpeZone, pitch_bend_range_semitones: u8) -> Option<Self> {
        if !zone.is_valid()
            || pitch_bend_range_semitones == 0
            || pitch_bend_range_semitones > MAX_MPE_PITCH_BEND_SEMITONES
        {
            return None;
        }
        Some(Self {
            zone,
            pitch_bend_range_semitones,
        })
    }

    /// Convert a signed 14-bit MIDI pitch bend (`-8192..=8191`) into cents
    /// scaled by `pitch_bend_range_semitones`, clamped to `-4800..=4800`.
    pub fn bend_to_cents(self, bend: i16) -> i16 {
        let range_cents = i64::from(self.pitch_bend_range_semitones) * 100;
        let denom = if bend < 0 { 8192_i64 } else { 8191_i64 };
        let scaled = (i64::from(bend) * range_cents
            + if bend >= 0 { denom / 2 } else { -(denom / 2) })
            / denom;
        scaled.clamp(-4800, 4800) as i16
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

/// Zero-allocation MPE member-channel expression decoder.
#[derive(Clone, Debug)]
pub struct MpeDecoder {
    config: MpeConfig,
    /// Currently sounding pitch on each member channel (`0..=15`).
    active_pitch: [Option<u8>; 16],
}

impl MpeDecoder {
    pub const fn new(config: MpeConfig) -> Self {
        Self {
            config,
            active_pitch: [None; 16],
        }
    }

    pub const fn config(&self) -> MpeConfig {
        self.config
    }

    /// Clear tracked active pitches on all member channels.
    pub fn reset(&mut self) {
        self.active_pitch = [None; 16];
    }

    /// Active pitch currently tracked on `channel` (`0..=15`), if any.
    pub fn active_pitch(&self, channel: u8) -> Option<u8> {
        self.active_pitch
            .get(usize::from(channel))
            .copied()
            .flatten()
    }

    /// Decode one complete MIDI 1.0 channel-voice message (2 or 3 bytes) under
    /// the configured MPE zone. Returns `None` for out-of-zone or malformed input.
    pub fn decode_message(&mut self, message: &[u8]) -> Option<MpeExpressionEvent> {
        if !(2..=3).contains(&message.len()) {
            return None;
        }
        let (&status, data) = message.split_first()?;
        if !(0x80..0xf0).contains(&status) || data.iter().any(|&v| v > 127) {
            return None;
        }
        let channel = status & 0x0f;
        let kind = status & 0xf0;
        let is_master = channel == self.config.zone.master_channel();
        let is_member = self.config.zone.is_member_channel(channel);
        if !is_master && !is_member {
            return None;
        }

        match (kind, data) {
            (0x90, &[pitch, velocity]) if velocity != 0 => {
                if is_member {
                    self.active_pitch[usize::from(channel)] = Some(pitch);
                }
                Some(MpeExpressionEvent::NoteOn {
                    channel,
                    pitch,
                    velocity,
                })
            }
            (0x80 | 0x90, &[pitch, velocity]) => {
                if is_member && self.active_pitch[usize::from(channel)] == Some(pitch) {
                    self.active_pitch[usize::from(channel)] = None;
                }
                Some(MpeExpressionEvent::NoteOff {
                    channel,
                    pitch,
                    velocity,
                })
            }
            (0xa0, &[pitch, value]) => Some(MpeExpressionEvent::NotePressure {
                channel,
                pitch,
                value,
            }),
            (0xd0, &[pressure]) => {
                if is_member {
                    let pitch = self.active_pitch[usize::from(channel)]?;
                    Some(MpeExpressionEvent::NotePressure {
                        channel,
                        pitch,
                        value: pressure,
                    })
                } else {
                    Some(MpeExpressionEvent::MasterPressure {
                        channel,
                        value: pressure,
                    })
                }
            }
            (0xe0, &[low, high]) => {
                let bend = (i16::from(high) * 128 + i16::from(low)) - 8192;
                if is_member {
                    let pitch = self.active_pitch[usize::from(channel)]?;
                    Some(MpeExpressionEvent::NotePitchBend {
                        channel,
                        pitch,
                        cents: self.config.bend_to_cents(bend),
                    })
                } else {
                    Some(MpeExpressionEvent::MasterPitchBend {
                        channel,
                        value: bend,
                    })
                }
            }
            (0xb0, &[cc, value]) => {
                if crate::note_events::is_all_notes_off_cc(cc) {
                    if is_master {
                        self.reset();
                    } else {
                        self.active_pitch[usize::from(channel)] = None;
                    }
                    return Some(MpeExpressionEvent::AllNotesOff);
                }
                if is_member && cc == ControlChange::TIMBRE {
                    let pitch = self.active_pitch[usize::from(channel)]?;
                    return Some(MpeExpressionEvent::NoteSlide {
                        channel,
                        pitch,
                        value,
                    });
                }
                Some(MpeExpressionEvent::MasterControlChange { channel, cc, value })
            }
            _ => None,
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
}
