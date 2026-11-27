//! Typed audio, sidechain, MIDI, and auxiliary-output port declarations for
//! native devices (`#2680`).
//!
//! Port declarations stay pure, bounded, and allocation-free so host compilers,
//! routing validators, and UI surfaces can query capabilities without opening
//! DSP runtimes or inferring behavior from device type names.
use crate::DeviceContractError;

/// Maximum number of typed ports a single native device may declare.
pub const MAX_DEVICE_PORTS: usize = 32;
/// Maximum channel width of a single audio port.
pub const MAX_DEVICE_PORT_CHANNELS: u8 = 8;
/// Maximum UTF-8 byte length of a lowercase port display name.
pub const MAX_DEVICE_PORT_NAME_BYTES: usize = 64;

/// Stable non-zero identifier for one port within a device definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DevicePortId(u16);

impl DevicePortId {
    pub const fn from_static(id: u16) -> Self {
        Self(id)
    }

    pub const fn new(id: u16) -> Result<Self, DeviceContractError> {
        if id == 0 {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(Self(id))
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    pub const fn validate(self) -> Result<(), DeviceContractError> {
        if self.0 == 0 {
            Err(DeviceContractError::InvalidId)
        } else {
            Ok(())
        }
    }
}

/// Signal flow direction for a declared device port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevicePortDirection {
    Input,
    Output,
}

/// Signal domain and routing role of a declared device port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevicePortKind {
    MainAudio,
    SidechainAudio,
    AuxiliaryAudio,
    Midi,
}

/// Static typed declaration of one device port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevicePortDescriptor {
    pub id: DevicePortId,
    pub name: &'static str,
    pub direction: DevicePortDirection,
    pub kind: DevicePortKind,
    /// Channel count (`0` for MIDI ports, `1..=MAX_DEVICE_PORT_CHANNELS` for audio).
    pub channels: u8,
    /// Whether the device can operate cleanly when this port is unconnected.
    pub optional: bool,
}

/// Standard port IDs used by built-in device capability presets.
pub const PORT_MAIN_AUDIO_IN: DevicePortId = DevicePortId::from_static(1);
pub const PORT_MAIN_AUDIO_OUT: DevicePortId = DevicePortId::from_static(2);
pub const PORT_SIDECHAIN_AUDIO_IN: DevicePortId = DevicePortId::from_static(3);
pub const PORT_MIDI_IN: DevicePortId = DevicePortId::from_static(4);
pub const PORT_MIDI_OUT: DevicePortId = DevicePortId::from_static(5);

/// Main stereo audio input + output (standard audio effects).
pub const STEREO_AUDIO_EFFECT_PORTS: &[DevicePortDescriptor] = &[
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_IN,
        name: "main in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_OUT,
        name: "main out",
        direction: DevicePortDirection::Output,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
];

/// Main stereo audio I/O + optional stereo sidechain input (Compressor, Gate).
pub const SIDECHAIN_STEREO_EFFECT_PORTS: &[DevicePortDescriptor] = &[
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_IN,
        name: "main in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_OUT,
        name: "main out",
        direction: DevicePortDirection::Output,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_SIDECHAIN_AUDIO_IN,
        name: "sidechain in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::SidechainAudio,
        channels: 2,
        optional: true,
    },
];

/// Main stereo audio I/O + optional MIDI control input (MIDI-controlled effects such as Shifter).
pub const MIDI_CONTROLLED_STEREO_EFFECT_PORTS: &[DevicePortDescriptor] = &[
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_IN,
        name: "main in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_OUT,
        name: "main out",
        direction: DevicePortDirection::Output,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MIDI_IN,
        name: "midi in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::Midi,
        channels: 0,
        optional: true,
    },
];

/// MIDI input + MIDI output (MIDI effects / processors).
pub const MIDI_EFFECT_PORTS: &[DevicePortDescriptor] = &[
    DevicePortDescriptor {
        id: PORT_MIDI_IN,
        name: "midi in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::Midi,
        channels: 0,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MIDI_OUT,
        name: "midi out",
        direction: DevicePortDirection::Output,
        kind: DevicePortKind::Midi,
        channels: 0,
        optional: false,
    },
];

/// MIDI input + main stereo audio output (standard stereo instruments).
pub const STEREO_INSTRUMENT_PORTS: &[DevicePortDescriptor] = &[
    DevicePortDescriptor {
        id: PORT_MIDI_IN,
        name: "midi in",
        direction: DevicePortDirection::Input,
        kind: DevicePortKind::Midi,
        channels: 0,
        optional: false,
    },
    DevicePortDescriptor {
        id: PORT_MAIN_AUDIO_OUT,
        name: "main out",
        direction: DevicePortDirection::Output,
        kind: DevicePortKind::MainAudio,
        channels: 2,
        optional: false,
    },
];

/// Summarized typed port capabilities derived from validated `DevicePortDescriptor`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DevicePortCapabilities {
    pub main_audio_input_channels: u8,
    pub main_audio_output_channels: u8,
    pub sidechain_input_channels: u8,
    pub auxiliary_audio_outputs: u8,
    pub accepts_midi: bool,
    pub emits_midi: bool,
}

impl DevicePortCapabilities {
    pub fn from_ports(ports: &[DevicePortDescriptor]) -> Result<Self, DeviceContractError> {
        validate_device_ports(ports)?;
        let mut caps = Self::default();
        for port in ports {
            match (port.direction, port.kind) {
                (DevicePortDirection::Input, DevicePortKind::MainAudio) => {
                    caps.main_audio_input_channels = port.channels;
                }
                (DevicePortDirection::Output, DevicePortKind::MainAudio) => {
                    caps.main_audio_output_channels = port.channels;
                }
                (DevicePortDirection::Input, DevicePortKind::SidechainAudio) => {
                    caps.sidechain_input_channels = port.channels;
                }
                (DevicePortDirection::Output, DevicePortKind::AuxiliaryAudio) => {
                    caps.auxiliary_audio_outputs = caps.auxiliary_audio_outputs.saturating_add(1);
                }
                (DevicePortDirection::Input, DevicePortKind::Midi) => {
                    caps.accepts_midi = true;
                }
                (DevicePortDirection::Output, DevicePortKind::Midi) => {
                    caps.emits_midi = true;
                }
                _ => {}
            }
        }
        Ok(caps)
    }

    pub const fn has_sidechain_input(self) -> bool {
        self.sidechain_input_channels > 0
    }

    pub const fn is_midi_controlled_audio_effect(self) -> bool {
        self.main_audio_input_channels > 0
            && self.main_audio_output_channels > 0
            && self.accepts_midi
    }
}

/// Validate a static port list for uniqueness, bounds, lowercase names, and
/// direction/kind/channel invariants.
pub fn validate_device_ports(ports: &[DevicePortDescriptor]) -> Result<(), DeviceContractError> {
    if ports.is_empty() || ports.len() > MAX_DEVICE_PORTS {
        return Err(DeviceContractError::InvalidDescriptor);
    }
    let mut main_in = false;
    let mut main_out = false;
    let mut sidechain_in = false;
    for (index, port) in ports.iter().enumerate() {
        port.id.validate()?;
        if ports[..index].iter().any(|prior| prior.id == port.id) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        if port.name.is_empty()
            || port.name.len() > MAX_DEVICE_PORT_NAME_BYTES
            || port
                .name
                .bytes()
                .any(|b| b.is_ascii_uppercase() || b.is_ascii_control())
        {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        match (port.direction, port.kind) {
            (DevicePortDirection::Input, DevicePortKind::MainAudio) => {
                if main_in || port.channels == 0 || port.channels > MAX_DEVICE_PORT_CHANNELS {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                main_in = true;
            }
            (DevicePortDirection::Output, DevicePortKind::MainAudio) => {
                if main_out || port.channels == 0 || port.channels > MAX_DEVICE_PORT_CHANNELS {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                main_out = true;
            }
            (DevicePortDirection::Input, DevicePortKind::SidechainAudio) => {
                if sidechain_in || port.channels == 0 || port.channels > MAX_DEVICE_PORT_CHANNELS {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
                sidechain_in = true;
            }
            (DevicePortDirection::Output, DevicePortKind::AuxiliaryAudio) => {
                if port.channels == 0 || port.channels > MAX_DEVICE_PORT_CHANNELS {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
            }
            (DevicePortDirection::Input | DevicePortDirection::Output, DevicePortKind::Midi) => {
                if port.channels != 0 {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
            }
            // SidechainAudio is Input-only; AuxiliaryAudio is Output-only.
            (DevicePortDirection::Output, DevicePortKind::SidechainAudio)
            | (DevicePortDirection::Input, DevicePortKind::AuxiliaryAudio) => {
                return Err(DeviceContractError::InvalidDescriptor);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_port_presets_validate_and_expose_expected_capabilities() {
        let stereo = DevicePortCapabilities::from_ports(STEREO_AUDIO_EFFECT_PORTS).unwrap();
        assert_eq!(stereo.main_audio_input_channels, 2);
        assert_eq!(stereo.main_audio_output_channels, 2);
        assert!(!stereo.has_sidechain_input());
        assert!(!stereo.accepts_midi);

        let sidechain = DevicePortCapabilities::from_ports(SIDECHAIN_STEREO_EFFECT_PORTS).unwrap();
        assert!(sidechain.has_sidechain_input());
        assert_eq!(sidechain.sidechain_input_channels, 2);

        let midi_fx =
            DevicePortCapabilities::from_ports(MIDI_CONTROLLED_STEREO_EFFECT_PORTS).unwrap();
        assert!(midi_fx.is_midi_controlled_audio_effect());
        assert!(!midi_fx.emits_midi);

        let midi_proc = DevicePortCapabilities::from_ports(MIDI_EFFECT_PORTS).unwrap();
        assert!(midi_proc.accepts_midi && midi_proc.emits_midi);
        assert_eq!(midi_proc.main_audio_input_channels, 0);

        let inst = DevicePortCapabilities::from_ports(STEREO_INSTRUMENT_PORTS).unwrap();
        assert!(inst.accepts_midi);
        assert_eq!(inst.main_audio_output_channels, 2);
    }

    #[test]
    fn port_validation_rejects_malformed_ids_duplicates_directions_and_channels() {
        assert!(validate_device_ports(&[]).is_err());

        // Zero ID refused.
        assert!(
            validate_device_ports(&[DevicePortDescriptor {
                id: DevicePortId::from_static(0),
                name: "main out",
                direction: DevicePortDirection::Output,
                kind: DevicePortKind::MainAudio,
                channels: 2,
                optional: false,
            }])
            .is_err()
        );

        // Duplicate ID refused.
        assert!(
            validate_device_ports(&[
                STEREO_AUDIO_EFFECT_PORTS[0],
                DevicePortDescriptor {
                    id: PORT_MAIN_AUDIO_IN,
                    name: "main out",
                    direction: DevicePortDirection::Output,
                    kind: DevicePortKind::MainAudio,
                    channels: 2,
                    optional: false,
                },
            ])
            .is_err()
        );

        // Uppercase port name refused (Hex lowercase policy).
        assert!(
            validate_device_ports(&[DevicePortDescriptor {
                id: PORT_MAIN_AUDIO_OUT,
                name: "Main Out",
                direction: DevicePortDirection::Output,
                kind: DevicePortKind::MainAudio,
                channels: 2,
                optional: false,
            }])
            .is_err()
        );

        // MIDI port with nonzero channels refused.
        assert!(
            validate_device_ports(&[DevicePortDescriptor {
                id: PORT_MIDI_IN,
                name: "midi in",
                direction: DevicePortDirection::Input,
                kind: DevicePortKind::Midi,
                channels: 2,
                optional: true,
            }])
            .is_err()
        );

        // Wrong direction for SidechainAudio or AuxiliaryAudio refused.
        assert!(
            validate_device_ports(&[DevicePortDescriptor {
                id: PORT_SIDECHAIN_AUDIO_IN,
                name: "sidechain out",
                direction: DevicePortDirection::Output,
                kind: DevicePortKind::SidechainAudio,
                channels: 2,
                optional: true,
            }])
            .is_err()
        );
        assert!(
            validate_device_ports(&[DevicePortDescriptor {
                id: DevicePortId::from_static(10),
                name: "aux in",
                direction: DevicePortDirection::Input,
                kind: DevicePortKind::AuxiliaryAudio,
                channels: 2,
                optional: true,
            }])
            .is_err()
        );
    }
}
