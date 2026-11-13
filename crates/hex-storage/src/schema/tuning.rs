//! Current schema37 requires tuning, original/device assets and every MIDI clip-volume row.
use super::{Project, StorageError};
use hex_tuning::{Audition, ProjectTuning, Scale, Waveform};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct DocumentV37<'a> {
    #[serde(flatten)]
    base: super::fade_shapes::DocumentV29<'a>,
    tuning: Tuning,
    midi_clip_volumes: super::midi_clip_volume::Manifest,
    asset_sources: super::original_sources::Manifest,
    device_assets: super::device_assets::Manifest,
    main_devices: Vec<super::DeviceDtoV14>,
    arrangement_sections: super::arrangement_sections::Manifest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Tuning {
    enabled: bool,
    scale: ScaleDto,
    audition: AuditionDto,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleDto {
    count: usize,
    #[serde(deserialize_with = "bounded_cents")]
    cents: Vec<f32>,
    root_key: u8,
    root_hz: f64,
    root_degree: usize,
    period_cents: f32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditionDto {
    waveform: WaveformDto,
    level: f32,
    attack_ms: f32,
    release_ms: f32,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaveformDto {
    Triangle,
    Square,
    Saw,
}
fn bounded_cents<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
    struct Cents;
    impl<'de> serde::de::Visitor<'de> for Cents {
        type Value = Vec<f32>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("exactly 48 tuning degree values")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == 48 {
                    return Err(serde::de::Error::custom("tuning degree limit reached"));
                }
                values.push(value);
            }
            if values.len() != 48 {
                return Err(serde::de::Error::custom("tuning requires 48 degree values"));
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Cents)
}
impl From<ProjectTuning> for Tuning {
    fn from(tuning: ProjectTuning) -> Self {
        let scale = tuning.scale();
        let audition = tuning.audition();
        Self {
            enabled: tuning.enabled(),
            scale: ScaleDto {
                count: scale.count(),
                cents: scale.cents().to_vec(),
                root_key: scale.root_key(),
                root_hz: scale.root_hz(),
                root_degree: scale.root_degree(),
                period_cents: scale.period_cents(),
            },
            audition: AuditionDto {
                waveform: match audition.waveform() {
                    Waveform::Triangle => WaveformDto::Triangle,
                    Waveform::Square => WaveformDto::Square,
                    Waveform::Saw => WaveformDto::Saw,
                },
                level: audition.level(),
                attack_ms: audition.attack_ms(),
                release_ms: audition.release_ms(),
            },
        }
    }
}
impl Tuning {
    fn into_tuning(self) -> Result<ProjectTuning, StorageError> {
        let cents = self
            .scale
            .cents
            .try_into()
            .map_err(|_| StorageError::Invalid("tuning requires 48 degree values"))?;
        let scale = Scale::new(
            self.scale.count,
            cents,
            self.scale.root_key,
            self.scale.root_hz,
            self.scale.root_degree,
            self.scale.period_cents,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        let waveform = match self.audition.waveform {
            WaveformDto::Triangle => Waveform::Triangle,
            WaveformDto::Square => Waveform::Square,
            WaveformDto::Saw => Waveform::Saw,
        };
        let audition = Audition::new(
            waveform,
            self.audition.level,
            self.audition.attack_ms,
            self.audition.release_ms,
        )
        .map_err(|e| StorageError::Invalid(e.0))?;
        ProjectTuning::new(self.enabled, scale, audition).map_err(|e| StorageError::Invalid(e.0))
    }
}
impl<'a> DocumentV37<'a> {
    pub(crate) fn from_project(project: &'a Project) -> Result<Self, StorageError> {
        let base = super::fade_shapes::DocumentV29::for_current_envelope(project)?;
        Ok(Self {
            base,
            arrangement_sections: super::arrangement_sections::Manifest::from_project(project),
            tuning: project.tuning().into(),
            midi_clip_volumes: super::midi_clip_volume::Manifest::from_project(project)?,
            asset_sources: super::original_sources::Manifest::from_project(project),
            device_assets: super::device_assets::Manifest::from_project(project)?,
            main_devices: project
                .main_devices()
                .iter()
                .map(|device| {
                    super::device_to_dto(device.id().value(), device.bypassed(), device.state())
                })
                .collect::<Result<_, _>>()?,
        })
    }
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    decode_version(bytes, 37)
}

/// Frozen upgrades backed by original schema34–36 encoder fixtures.
/// Fields absent by design default only for the precise historical version;
/// present inventories always pass their original strict parser.
pub(crate) fn decode_version(bytes: &[u8], version: u32) -> Result<Project, StorageError> {
    // Preserve duplicate-field refusal before adapting the older strict envelope.
    decode_version_value(super::audio_events::parse_unique(bytes)?, version)
}

/// [`decode_version`] for an envelope an upper layer already parsed.
pub(crate) fn decode_version_value(
    mut value: serde_json::Value,
    version: u32,
) -> Result<Project, StorageError> {
    if (34..=36).contains(&version) {
        super::migrations::upgrade_nested(&mut value, version)?;
    }
    require_device_host_gains(&value)?;
    let object = value
        .as_object_mut()
        .ok_or(StorageError::Invalid("invalid tuning envelope"))?;
    if object.get("version").and_then(|v| v.as_u64()) != Some(u64::from(version))
        || !(34..=37).contains(&version)
    {
        return Err(StorageError::Invalid("invalid tuning envelope version"));
    }
    // Two historical branches reused36. A wholly absent inventory selects the
    // valid tempo branch; deletion of that whole field is indistinguishable.
    // Present null/partial inventories never select a fallback.
    let sections = if version <= 35
        || (version == 36 && !object.contains_key("arrangement_sections"))
    {
        None
    } else {
        Some(serde_json::from_value::<
            super::arrangement_sections::Manifest,
        >(object.remove("arrangement_sections").ok_or(
            StorageError::Invalid("arrangement section inventory is missing"),
        )?)?)
    };
    let volumes = if version == 34 {
        None
    } else {
        Some(serde_json::from_value::<super::midi_clip_volume::Manifest>(
            object
                .remove("midi_clip_volumes")
                .ok_or(StorageError::Invalid(
                    "midi clip volume inventory is missing",
                ))?,
        )?)
    };
    let tuning: Tuning = serde_json::from_value(
        object
            .remove("tuning")
            .ok_or(StorageError::Invalid("project tuning is missing"))?,
    )?;
    let tuning = tuning.into_tuning()?;
    let manifest: super::device_assets::Manifest = serde_json::from_value(
        object
            .remove("device_assets")
            .ok_or(StorageError::Invalid("device asset manifest is missing"))?,
    )?;
    let main: BoundedMain = serde_json::from_value(serde_json::json!({
        "devices": object.remove("main_devices").ok_or(StorageError::Invalid("main device inventory is missing"))?
    }))?;
    let original_sources: super::original_sources::Manifest = serde_json::from_value(
        object
            .remove("asset_sources")
            .ok_or(StorageError::Invalid("asset source inventory is missing"))?,
    )?;
    object.insert("version".into(), 29.into());
    let base = super::fade_shapes::DocumentV29::decode_value(value)?;
    let mut snapshot = base.snapshot();
    snapshot.tuning = tuning;
    if let Some(sections) = sections {
        snapshot.arrangement_sections = sections.into_sections()?;
    }
    snapshot.main_devices = main
        .devices
        .into_iter()
        .map(super::device_dto_into_snapshot)
        .collect::<Result<_, _>>()?;
    manifest.apply(&mut snapshot)?;
    original_sources.apply(&mut snapshot)?;
    if let Some(volumes) = volumes {
        volumes.apply(&mut snapshot)?;
    }
    Project::from_legacy_snapshot(snapshot).map_err(|e| StorageError::Invalid(e.0))
}

/// The private staging DTOs also serve old-shaped internal envelopes and default
/// absent host trims to unity. Current files must carry both saved values rather
/// than silently losing a trim. Inspect only actual device owners; asset and
/// comparison rows have different contracts and never carry these fields.
fn require_device_host_gains(value: &serde_json::Value) -> Result<(), StorageError> {
    fn require(device: &serde_json::Value) -> Result<(), StorageError> {
        if device.get("input_gain_millidb").is_none() || device.get("output_gain_millidb").is_none()
        {
            return Err(StorageError::Invalid("device host gain fields are missing"));
        }
        Ok(())
    }
    for owner in ["tracks", "return_tracks"] {
        for track in value
            .get(owner)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            for device in track
                .get("devices")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                require(device)?;
            }
            if owner == "tracks"
                && let Some(instrument) = track.get("instrument").filter(|value| !value.is_null())
            {
                require(instrument)?;
            }
        }
    }
    for device in value
        .get("main_devices")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        require(device)?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedMain {
    #[serde(deserialize_with = "bounded_main")]
    devices: Vec<super::DeviceDtoV14>,
}
fn bounded_main<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<super::DeviceDtoV14>, D::Error> {
    struct Main;
    impl<'de> serde::de::Visitor<'de> for Main {
        type Value = Vec<super::DeviceDtoV14>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded main device inventory")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut devices = Vec::new();
            while let Some(device) = seq.next_element()? {
                if devices.len() == hex_project::MAX_DEVICES_PER_TRACK {
                    return Err(serde::de::Error::custom("main device limit reached"));
                }
                devices.push(device);
            }
            Ok(devices)
        }
    }
    d.deserialize_seq(Main)
}
