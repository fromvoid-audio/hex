//! Independent, versioned preferences for live MIDI input selection.
//!
//! Preferences are read and written on the control side through the shared
//! bounded document reader and atomic publisher. Missing files select no port
//! and channel zero. Existing v1 single-port/single-channel and v2
//! Omni/multi-port documents load losslessly; configured MPE zones persist as
//! v3 documents. Malformed, unknown, or future documents remain untouched.

use super::{PreferencesError, atomic_save, read_document};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_PORT_NAME_BYTES: usize = 1024;
/// Maximum number of simultaneously aggregated MIDI input ports persisted in preferences.
pub const MAX_MIDI_INPUT_PORTS: usize = 16;
const MIDI_INPUT_DOCUMENT_VERSION_V1: u32 = 1;
const MIDI_INPUT_DOCUMENT_VERSION_V2: u32 = 2;
const MIDI_INPUT_DOCUMENT_VERSION_V3: u32 = 3;

/// Default MPE per-note member pitch bend range in semitones (`1..=96`).
pub const DEFAULT_MPE_MEMBER_PITCH_BEND_SEMITONES: u8 = 48;
/// Maximum supported MPE per-note member pitch bend range in semitones.
pub const MAX_MPE_MEMBER_PITCH_BEND_SEMITONES: u8 = 96;
/// Default MPE master-channel pitch bend range in semitones (`1..=48`).
pub const DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES: u8 = 2;
/// Maximum supported MPE master-channel pitch bend range in semitones.
pub const MAX_MPE_MASTER_PITCH_BEND_SEMITONES: u8 = 48;

const fn default_member_pitch_bend_semitones() -> u8 {
    DEFAULT_MPE_MEMBER_PITCH_BEND_SEMITONES
}

const fn default_master_pitch_bend_semitones() -> u8 {
    DEFAULT_MPE_MASTER_PITCH_BEND_SEMITONES
}

/// Persisted MPE zone side (`lower` master ch 1, `upper` master ch 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiInputMpeZoneKind {
    Lower,
    Upper,
}

/// Persisted bounded MPE zone selection (`member_channels` in `1..=15`,
/// `member_pitch_bend_semitones` in `1..=96`, `master_pitch_bend_semitones` in `1..=48`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiInputMpeZone {
    pub kind: MidiInputMpeZoneKind,
    pub member_channels: u8,
    #[serde(default = "default_member_pitch_bend_semitones")]
    pub member_pitch_bend_semitones: u8,
    #[serde(default = "default_master_pitch_bend_semitones")]
    pub master_pitch_bend_semitones: u8,
}

/// A native MIDI input port selection. The operating system's exact port name
/// is retained without normalization; `id` remains opaque to preferences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiInputPort {
    pub id: u32,
    pub name: String,
}

/// Persisted live MIDI input selection. Channel is zero based (`0..=15`) when
/// `omni` is `false`; when `omni` is `true` all 16 MIDI channels are accepted.
/// Single-port callers may populate `port`; multi-port selections populate
/// `ports` (up to [`MAX_MIDI_INPUT_PORTS`]) or set `all_ports = true`.
/// When `mpe_zone` is `Some`, input uses the configured bounded MPE zone instead
/// of ordinary single-channel or Omni mode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiInputPreferences {
    pub port: Option<MidiInputPort>,
    pub ports: Vec<MidiInputPort>,
    pub all_ports: bool,
    pub channel: u8,
    pub omni: bool,
    pub mpe_zone: Option<MidiInputMpeZone>,
}

impl MidiInputPreferences {
    /// Returns the deduplicated list of explicitly selected ports, falling back
    /// to `port` when `ports` is empty.
    pub fn selected_ports(&self) -> Vec<MidiInputPort> {
        if !self.ports.is_empty() {
            self.ports.clone()
        } else {
            self.port.iter().cloned().collect()
        }
    }

    /// True when this configuration can be represented without loss in the v1
    /// single-port/single-channel schema.
    pub fn is_v1_compatible(&self) -> bool {
        !self.omni
            && !self.all_ports
            && self.mpe_zone.is_none()
            && (self.ports.is_empty()
                || (self.ports.len() == 1 && self.ports.first() == self.port.as_ref()))
    }

    /// True when this configuration can be represented without loss in the v2
    /// Omni/multi-port schema without MPE zone settings.
    pub fn is_v2_compatible(&self) -> bool {
        self.mpe_zone.is_none()
    }
}

#[derive(Deserialize)]
struct VersionHeader {
    version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferencesV1 {
    port: Option<MidiInputPort>,
    channel: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV1 {
    version: u32,
    preferences: PreferencesV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferencesV2 {
    port: Option<MidiInputPort>,
    ports: Vec<MidiInputPort>,
    all_ports: bool,
    channel: u8,
    omni: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV2 {
    version: u32,
    preferences: PreferencesV2,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferencesV3 {
    port: Option<MidiInputPort>,
    ports: Vec<MidiInputPort>,
    all_ports: bool,
    channel: u8,
    omni: bool,
    mpe_zone: Option<MidiInputMpeZone>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentV3 {
    version: u32,
    preferences: PreferencesV3,
}

/// Stable app-support path for selected MIDI input preferences.
pub fn midi_input_preferences_path(home: &Path) -> PathBuf {
    crate::app_support_dir_from(home).join("midi-input.json")
}

/// Missing preferences leave hardware input disabled and select channel zero.
pub fn load_midi_input_preferences(path: &Path) -> Result<MidiInputPreferences, PreferencesError> {
    let Some(bytes) = read_document(path)? else {
        return Ok(MidiInputPreferences::default());
    };
    let header: VersionHeader = serde_json::from_slice(&bytes)
        .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    let preferences = match header.version {
        MIDI_INPUT_DOCUMENT_VERSION_V1 => {
            let document: DocumentV1 = serde_json::from_slice(&bytes)
                .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
            let ports = document.preferences.port.iter().cloned().collect();
            MidiInputPreferences {
                port: document.preferences.port,
                ports,
                all_ports: false,
                channel: document.preferences.channel,
                omni: false,
                mpe_zone: None,
            }
        }
        MIDI_INPUT_DOCUMENT_VERSION_V2 => {
            let document: DocumentV2 = serde_json::from_slice(&bytes)
                .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
            let p = document.preferences;
            let ports = if p.ports.is_empty() {
                p.port.iter().cloned().collect()
            } else {
                p.ports
            };
            let port = p.port.or_else(|| ports.first().cloned());
            MidiInputPreferences {
                port,
                ports,
                all_ports: p.all_ports,
                channel: p.channel,
                omni: p.omni,
                mpe_zone: None,
            }
        }
        MIDI_INPUT_DOCUMENT_VERSION_V3 => {
            let document: DocumentV3 = serde_json::from_slice(&bytes)
                .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
            let p = document.preferences;
            let ports = if p.ports.is_empty() {
                p.port.iter().cloned().collect()
            } else {
                p.ports
            };
            let port = p.port.or_else(|| ports.first().cloned());
            MidiInputPreferences {
                port,
                ports,
                all_ports: p.all_ports,
                channel: p.channel,
                omni: p.omni,
                mpe_zone: p.mpe_zone,
            }
        }
        other => {
            return Err(PreferencesError::InvalidDocument(format!(
                "unsupported version {other}"
            )));
        }
    };
    validate(&preferences)?;
    Ok(preferences)
}

/// Atomically save preferences, refusing to overwrite malformed or unsupported
/// existing data so the user can recover it.
pub fn save_midi_input_preferences(
    path: &Path,
    preferences: &MidiInputPreferences,
) -> Result<(), PreferencesError> {
    validate(preferences)?;
    let _ = load_midi_input_preferences(path)?;
    let bytes = if preferences.is_v1_compatible() {
        serde_json::to_vec(&DocumentV1 {
            version: MIDI_INPUT_DOCUMENT_VERSION_V1,
            preferences: PreferencesV1 {
                port: preferences.port.clone(),
                channel: preferences.channel,
            },
        })
    } else if preferences.is_v2_compatible() {
        let ports = preferences.selected_ports();
        let port = preferences.port.clone().or_else(|| ports.first().cloned());
        serde_json::to_vec(&DocumentV2 {
            version: MIDI_INPUT_DOCUMENT_VERSION_V2,
            preferences: PreferencesV2 {
                port,
                ports,
                all_ports: preferences.all_ports,
                channel: preferences.channel,
                omni: preferences.omni,
            },
        })
    } else {
        let ports = preferences.selected_ports();
        let port = preferences.port.clone().or_else(|| ports.first().cloned());
        serde_json::to_vec(&DocumentV3 {
            version: MIDI_INPUT_DOCUMENT_VERSION_V3,
            preferences: PreferencesV3 {
                port,
                ports,
                all_ports: preferences.all_ports,
                channel: preferences.channel,
                omni: preferences.omni,
                mpe_zone: preferences.mpe_zone,
            },
        })
    }
    .map_err(|error| PreferencesError::InvalidDocument(error.to_string()))?;
    if bytes.len() as u64 > super::MAX_DOCUMENT_BYTES {
        return Err(PreferencesError::DocumentTooLarge {
            maximum: super::MAX_DOCUMENT_BYTES,
        });
    }
    atomic_save(path, &bytes)
}

fn validate(preferences: &MidiInputPreferences) -> Result<(), PreferencesError> {
    if preferences.channel > 15 {
        return Err(PreferencesError::InvalidDocument(
            "midi channel must be between 0 and 15".into(),
        ));
    }
    if let Some(zone) = preferences.mpe_zone {
        if !(1..=15).contains(&zone.member_channels) {
            return Err(PreferencesError::InvalidDocument(
                "mpe member channels must be between 1 and 15".into(),
            ));
        }
        if !(1..=MAX_MPE_MEMBER_PITCH_BEND_SEMITONES).contains(&zone.member_pitch_bend_semitones) {
            return Err(PreferencesError::InvalidDocument(format!(
                "mpe member pitch bend range must be between 1 and {MAX_MPE_MEMBER_PITCH_BEND_SEMITONES} semitones"
            )));
        }
        if !(1..=MAX_MPE_MASTER_PITCH_BEND_SEMITONES).contains(&zone.master_pitch_bend_semitones) {
            return Err(PreferencesError::InvalidDocument(format!(
                "mpe master pitch bend range must be between 1 and {MAX_MPE_MASTER_PITCH_BEND_SEMITONES} semitones"
            )));
        }
    }
    if preferences.ports.len() > MAX_MIDI_INPUT_PORTS {
        return Err(PreferencesError::InvalidDocument(format!(
            "midi input ports exceed {MAX_MIDI_INPUT_PORTS}"
        )));
    }
    if preferences
        .port
        .as_ref()
        .is_some_and(|port| port.name.len() > MAX_PORT_NAME_BYTES)
    {
        return Err(PreferencesError::InvalidDocument(format!(
            "midi port name exceeds {MAX_PORT_NAME_BYTES} bytes"
        )));
    }
    let mut seen = HashSet::with_capacity(preferences.ports.len());
    for port in &preferences.ports {
        if port.name.len() > MAX_PORT_NAME_BYTES {
            return Err(PreferencesError::InvalidDocument(format!(
                "midi port name exceeds {MAX_PORT_NAME_BYTES} bytes"
            )));
        }
        if !seen.insert(port.id) {
            return Err(PreferencesError::InvalidDocument(format!(
                "duplicate midi port id {}",
                port.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hex-midi-input-prefs-{}-{}",
                std::process::id(),
                super::super::NEXT_TEMPORARY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_preferences_disable_input_and_default_to_channel_zero() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        assert_eq!(
            load_midi_input_preferences(&path).unwrap(),
            MidiInputPreferences {
                port: None,
                ports: Vec::new(),
                all_ports: false,
                channel: 0,
                omni: false,
                mpe_zone: None,
            }
        );
        assert!(!path.exists());
    }

    #[test]
    fn round_trip_preserves_port_identity_name_case_and_channel() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let port = MidiInputPort {
            id: u32::MAX,
            name: "My MIDI Interface (Port A)".into(),
        };
        let preferences = MidiInputPreferences {
            port: Some(port.clone()),
            ports: vec![port],
            all_ports: false,
            channel: 15,
            omni: false,
            mpe_zone: None,
        };
        save_midi_input_preferences(&path, &preferences).unwrap();
        // Single-port/single-channel saves stay v1-compatible.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""version":1"#));
        assert_eq!(load_midi_input_preferences(&path).unwrap(), preferences);
    }

    #[test]
    fn historical_v1_single_port_document_loads_without_breaking_and_migrates_in_memory() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let v1 =
            br#"{"version":1,"preferences":{"port":{"id":42,"name":"KeyStep Pro"},"channel":7}}"#;
        std::fs::write(&path, v1).unwrap();
        let loaded = load_midi_input_preferences(&path).unwrap();
        assert_eq!(
            loaded,
            MidiInputPreferences {
                port: Some(MidiInputPort {
                    id: 42,
                    name: "KeyStep Pro".into(),
                }),
                ports: vec![MidiInputPort {
                    id: 42,
                    name: "KeyStep Pro".into(),
                }],
                all_ports: false,
                channel: 7,
                omni: false,
                mpe_zone: None,
            }
        );
        // Opening never rewrites existing v1 bytes on disk.
        assert_eq!(std::fs::read(&path).unwrap(), v1);
    }

    #[test]
    fn v2_round_trip_preserves_omni_all_ports_and_multi_port_selection() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let port_a = MidiInputPort {
            id: 10,
            name: "Launchkey MK4".into(),
        };
        let port_b = MidiInputPort {
            id: 20,
            name: "Hydrasynth Keyboard".into(),
        };
        let multi = MidiInputPreferences {
            port: Some(port_a.clone()),
            ports: vec![port_a.clone(), port_b.clone()],
            all_ports: false,
            channel: 2,
            omni: true,
            mpe_zone: None,
        };
        save_midi_input_preferences(&path, &multi).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""version":2"#));
        assert_eq!(load_midi_input_preferences(&path).unwrap(), multi);

        let all = MidiInputPreferences {
            port: None,
            ports: Vec::new(),
            all_ports: true,
            channel: 0,
            omni: true,
            mpe_zone: None,
        };
        save_midi_input_preferences(&path, &all).unwrap();
        assert_eq!(load_midi_input_preferences(&path).unwrap(), all);
    }

    #[test]
    fn v3_round_trip_persists_bounded_mpe_lower_and_upper_zones_and_rejects_invalid_member_counts()
    {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let port = MidiInputPort {
            id: 77,
            name: "LinnStrument".into(),
        };
        for zone in [
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 15,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 2,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Upper,
                member_channels: 7,
                member_pitch_bend_semitones: 24,
                master_pitch_bend_semitones: 12,
            },
        ] {
            let prefs = MidiInputPreferences {
                port: Some(port.clone()),
                ports: vec![port.clone()],
                all_ports: false,
                channel: 0,
                omni: false,
                mpe_zone: Some(zone),
            };
            save_midi_input_preferences(&path, &prefs).unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(raw.contains(r#""version":3"#));
            assert_eq!(load_midi_input_preferences(&path).unwrap(), prefs);
        }

        // Earlier v3 documents without explicit bend-range fields load cleanly with MPE defaults (48 / 2).
        let legacy_v3 = br#"{"version":3,"preferences":{"port":null,"ports":[],"all_ports":false,"channel":0,"omni":false,"mpe_zone":{"kind":"lower","member_channels":7}}}"#;
        let legacy_path = dir.0.join("legacy-v3.json");
        std::fs::write(&legacy_path, legacy_v3).unwrap();
        assert_eq!(
            load_midi_input_preferences(&legacy_path).unwrap().mpe_zone,
            Some(MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 7,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 2,
            })
        );

        // Turning MPE back off stays v1-compatible for single-port/single-channel.
        let ordinary = MidiInputPreferences {
            port: Some(port.clone()),
            ports: vec![port],
            all_ports: false,
            channel: 3,
            omni: false,
            mpe_zone: None,
        };
        save_midi_input_preferences(&path, &ordinary).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#""version":1"#));
        assert_eq!(load_midi_input_preferences(&path).unwrap(), ordinary);

        // Invalid member counts (0, 16) and invalid bend ranges are rejected on save and load.
        let invalid_path = dir.0.join("invalid-mpe.json");
        for bad_zone in [
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 0,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 2,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 16,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 2,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 7,
                member_pitch_bend_semitones: 0,
                master_pitch_bend_semitones: 2,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 7,
                member_pitch_bend_semitones: 97,
                master_pitch_bend_semitones: 2,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 7,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 0,
            },
            MidiInputMpeZone {
                kind: MidiInputMpeZoneKind::Lower,
                member_channels: 7,
                member_pitch_bend_semitones: 48,
                master_pitch_bend_semitones: 49,
            },
        ] {
            let bad = MidiInputPreferences {
                mpe_zone: Some(bad_zone),
                ..MidiInputPreferences::default()
            };
            assert!(save_midi_input_preferences(&invalid_path, &bad).is_err());
            assert!(!invalid_path.exists());
        }
        let malformed_v3 = br#"{"version":3,"preferences":{"port":null,"ports":[],"all_ports":false,"channel":0,"omni":false,"mpe_zone":{"kind":"upper","member_channels":0}}}"#;
        std::fs::write(&invalid_path, malformed_v3).unwrap();
        assert!(load_midi_input_preferences(&invalid_path).is_err());
        assert!(
            save_midi_input_preferences(&invalid_path, &MidiInputPreferences::default()).is_err()
        );
        assert_eq!(std::fs::read(&invalid_path).unwrap(), malformed_v3);
    }

    #[test]
    fn invalid_channel_oversized_name_duplicate_or_excess_ports_are_rejected() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let too_many: Vec<MidiInputPort> = (0..=MAX_MIDI_INPUT_PORTS as u32)
            .map(|id| MidiInputPort {
                id,
                name: format!("Port {id}"),
            })
            .collect();
        for preferences in [
            MidiInputPreferences {
                channel: 16,
                ..MidiInputPreferences::default()
            },
            MidiInputPreferences {
                port: Some(MidiInputPort {
                    id: 1,
                    name: "x".repeat(MAX_PORT_NAME_BYTES + 1),
                }),
                ..MidiInputPreferences::default()
            },
            MidiInputPreferences {
                port: Some(MidiInputPort {
                    id: 2,
                    name: "é".repeat((MAX_PORT_NAME_BYTES / 2) + 1),
                }),
                ..MidiInputPreferences::default()
            },
            MidiInputPreferences {
                ports: vec![
                    MidiInputPort {
                        id: 5,
                        name: "A".into(),
                    },
                    MidiInputPort {
                        id: 5,
                        name: "B".into(),
                    },
                ],
                ..MidiInputPreferences::default()
            },
            MidiInputPreferences {
                ports: too_many,
                ..MidiInputPreferences::default()
            },
        ] {
            assert!(save_midi_input_preferences(&path, &preferences).is_err());
            assert!(!path.exists());
        }
    }

    #[test]
    fn malformed_future_unknown_and_oversized_name_documents_are_preserved() {
        let dir = Directory::new();
        let path = dir.0.join("midi-input.json");
        let oversized = format!(
            r#"{{"version":1,"preferences":{{"port":{{"id":7,"name":"{}"}},"channel":0}}}}"#,
            "x".repeat(MAX_PORT_NAME_BYTES + 1)
        );
        let documents = [
            b"bad json".to_vec(),
            br#"{"version":99,"preferences":{"port":null,"channel":0}}"#.to_vec(),
            br#"{"version":1,"preferences":{"port":null,"channel":0,"unknown":true}}"#.to_vec(),
            oversized.into_bytes(),
        ];
        for bytes in documents {
            std::fs::write(&path, &bytes).unwrap();
            assert!(load_midi_input_preferences(&path).is_err());
            assert!(save_midi_input_preferences(&path, &MidiInputPreferences::default()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn write_failure_preserves_existing_path_contents() {
        let dir = Directory::new();
        let path = dir.0.join("not-a-directory");
        std::fs::write(&path, b"preserve me").unwrap();
        assert!(
            save_midi_input_preferences(
                &path.join("midi-input.json"),
                &MidiInputPreferences::default()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"preserve me");
    }
}
