//! Independent native device preset envelope. Payload bytes are never interpreted
//! here; native provider validation and prepared admission belong to the session.
use hex_lattice_contract::{
    DeviceTypeId, MAX_DEVICE_TYPE_ID_BYTES, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceState,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const EXTENSION: &str = "hex-device-preset.json";
// Compact JSON needs at most four bytes per payload byte (255 plus comma),
// plus the bounded type id and a fixed envelope allowance. No state cap changes.
pub const MAX_PRESET_BYTES: usize =
    8 * MAX_NATIVE_DEVICE_STATE_BYTES + 2 * MAX_DEVICE_TYPE_ID_BYTES + 512;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceClass {
    Instrument,
    Effect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub class: DeviceClass,
    pub state: NativeDeviceState,
    pub comparison: Option<hex_project::DeviceComparison>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    schema: u32,
    class: DeviceClass,
    type_id: String,
    state_version: u32,
    payload: Vec<u8>,
    #[serde(deserialize_with = "crate::schema::device_assets::required_comparison")]
    comparison: Option<crate::schema::device_assets::ComparisonDto>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
    PublishedButNotDurable {
        path: PathBuf,
        source: std::io::Error,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => f.write_str(match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "device preset destination exists · choose a new filename"
                }
                std::io::ErrorKind::NotFound => "device preset file or folder is missing",
                std::io::ErrorKind::PermissionDenied => {
                    "device preset access denied · check file and folder permissions"
                }
                std::io::ErrorKind::ReadOnlyFilesystem => {
                    "device preset location is read-only · choose a writable folder"
                }
                _ => "device preset file operation failed · check the file and folder",
            }),
            Self::Json(_) => {
                f.write_str("invalid device preset json · choose a complete preset file")
            }
            Self::Invalid(reason) => f.write_str(reason),
            Self::PublishedButNotDurable { path, .. } => write!(
                f,
                "complete device preset exists at {}; directory cleanup or durability failed",
                path.display()
            ),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source) | Self::PublishedButNotDurable { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::Invalid(_) => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn encode(preset: &Preset) -> Result<Vec<u8>, Error> {
    if preset
        .comparison
        .as_ref()
        .is_some_and(|comparison| comparison.inactive().type_id() != preset.state.type_id())
    {
        return Err(Error::Invalid(
            "comparison provider does not match device preset",
        ));
    }
    if preset.comparison.as_ref().is_some_and(|comparison| {
        comparison.inactive().input_gain_millidb() != preset.state.input_gain_millidb()
            || comparison.inactive().output_gain_millidb() != preset.state.output_gain_millidb()
    }) {
        return Err(Error::Invalid(
            "comparison host gains must be device-global",
        ));
    }
    if !preset.state.asset_references().is_empty()
        || preset
            .comparison
            .as_ref()
            .is_some_and(|value| !value.inactive().asset_references().is_empty())
    {
        return Err(Error::Invalid(
            "device asset presets require a portable asset manifest",
        ));
    }
    let bytes = serde_json::to_vec(&Envelope {
        format: "hex-device-preset".into(),
        schema: 2,
        class: preset.class,
        type_id: preset
            .state
            .type_id()
            .as_str()
            .map_err(|_| Error::Invalid("invalid device preset type id"))?
            .into(),
        state_version: preset.state.state_version(),
        payload: preset.state.payload().to_vec(),
        comparison: preset
            .comparison
            .as_ref()
            .map(crate::schema::device_assets::ComparisonDto::from_comparison)
            .transpose()
            .map_err(|_| Error::Invalid("invalid comparison state"))?,
    })?;
    if bytes.len() > MAX_PRESET_BYTES {
        return Err(Error::Invalid("device preset exceeds byte limit"));
    }
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Preset, Error> {
    if bytes.len() > MAX_PRESET_BYTES {
        return Err(Error::Invalid("device preset exceeds byte limit"));
    }
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    if envelope.format != "hex-device-preset" || envelope.schema != 2 {
        return Err(Error::Invalid(
            "unsupported device preset format or schema · use schema 2",
        ));
    }
    let type_id = DeviceTypeId::new(&envelope.type_id)
        .map_err(|_| Error::Invalid("invalid device preset type id"))?;
    let state = NativeDeviceState::new(type_id, envelope.state_version, envelope.payload)
        .map_err(|_| Error::Invalid("invalid device preset version or payload size"))?;
    let comparison = envelope
        .comparison
        .map(|value| value.into_comparison(&state))
        .transpose()
        .map_err(|_| Error::Invalid("invalid comparison state"))?;
    if comparison
        .as_ref()
        .is_some_and(|value| !value.inactive().asset_references().is_empty())
    {
        return Err(Error::Invalid(
            "device asset presets require a portable asset manifest",
        ));
    }
    Ok(Preset {
        comparison,
        class: envelope.class,
        state,
    })
}

pub fn load(path: &Path) -> Result<Preset, Error> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_PRESET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    decode(&bytes)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Atomically publish a complete create-new preset. Existing destinations are
/// never overwritten, including collisions appearing during serialization.
pub fn save_new(path: &Path, preset: &Preset) -> Result<(), Error> {
    if path.file_name().is_none() {
        return Err(Error::Invalid("device preset target needs a filename"));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "device preset destination exists · choose a new filename",
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = encode(preset)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut reserved = None;
    for _ in 0..32 {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".hex-preset-pending-{}-{id}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                reserved = Some((Temporary(candidate), file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary, mut file) = reserved.ok_or(Error::Invalid(
        "could not reserve device preset temporary file",
    ))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::hard_link(&temporary.0, path)?;
    let cleanup_error = fs::remove_file(&temporary.0).err();
    #[cfg(unix)]
    let sync_error = File::open(parent).and_then(|file| file.sync_all()).err();
    #[cfg(not(unix))]
    let sync_error = None;
    if let Some(source) = cleanup_error.or(sync_error) {
        return Err(Error::PublishedButNotDurable {
            path: path.into(),
            source,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Preset {
        Preset {
            comparison: None,
            class: DeviceClass::Instrument,
            state: NativeDeviceState::new(
                DeviceTypeId::from_static("hex.instrument.operator"),
                8,
                (0..=255)
                    .cycle()
                    .take(MAX_NATIVE_DEVICE_STATE_BYTES)
                    .collect(),
            )
            .unwrap(),
        }
    }
    #[test]
    fn complete_maximum_opaque_payload_roundtrips_without_version_rewriting() {
        let preset = fixture();
        let bytes = encode(&preset).unwrap();
        assert!(bytes.len() <= MAX_PRESET_BYTES);
        assert_eq!(decode(&bytes).unwrap(), preset);
    }
    #[test]
    fn complete_maximum_pair_roundtrips_and_wrong_provider_never_publishes() {
        let mut preset = fixture();
        let inactive = NativeDeviceState::new(
            preset.state.type_id(),
            9,
            vec![255; MAX_NATIVE_DEVICE_STATE_BYTES],
        )
        .unwrap();
        preset.comparison = Some(hex_project::DeviceComparison::new(
            hex_project::DeviceVariant::B,
            inactive,
        ));
        let bytes = encode(&preset).unwrap();
        assert!(bytes.len() <= MAX_PRESET_BYTES);
        assert_eq!(decode(&bytes).unwrap(), preset);
        let directory = std::env::temp_dir().join(format!(
            "hex-invalid-pair-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("invalid.hex-device-preset.json");
        preset.comparison = Some(hex_project::DeviceComparison::new(
            hex_project::DeviceVariant::A,
            NativeDeviceState::new(DeviceTypeId::from_static("other.provider"), 1, vec![1])
                .unwrap(),
        ));
        assert!(save_new(&path, &preset).is_err());
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir(&directory).unwrap();
    }

    #[test]
    fn preset_pair_refuses_mismatched_global_trims_before_file_publication() {
        for input in [true, false] {
            let mut preset = fixture();
            let inactive = if input {
                preset.state.clone().with_input_gain_millidb(-1200).unwrap()
            } else {
                preset.state.clone().with_output_gain_millidb(2300).unwrap()
            };
            preset.comparison = Some(hex_project::DeviceComparison::new(
                hex_project::DeviceVariant::A,
                inactive,
            ));
            assert!(
                encode(&preset).is_err(),
                "mismatched global trims must refuse"
            );
            let directory = std::env::temp_dir().join(format!(
                "hex-preset-trim-refusal-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let path = directory.join("refused.hex-device-preset.json");
            assert!(save_new(&path, &preset).is_err());
            assert!(!path.exists());
            fs::remove_dir(&directory).unwrap();
        }
    }

    #[test]
    fn malformed_unknown_newer_and_unbounded_envelopes_refuse() {
        let bytes = encode(&fixture()).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (key, value) in [
            ("schema", serde_json::json!(3)),
            ("format", serde_json::json!("project")),
            ("class", serde_json::json!("plugin")),
            ("type_id", serde_json::json!("INVALID")),
            ("state_version", serde_json::json!(0)),
            ("unknown", serde_json::json!(true)),
            (
                "payload",
                serde_json::json!(vec![0u8; MAX_NATIVE_DEVICE_STATE_BYTES + 1]),
            ),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            assert!(
                decode(&serde_json::to_vec(&changed).unwrap()).is_err(),
                "{key}"
            );
        }
        assert!(decode(b"{broken").is_err());
        assert!(decode(&vec![b' '; MAX_PRESET_BYTES + 1]).is_err());
    }
    #[test]
    fn create_new_collision_missing_and_oversized_files_are_safe() {
        let directory = std::env::temp_dir().join(format!(
            "hex-preset-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("ExactCase.hex-device-preset.json");
        assert!(load(&path).is_err());
        save_new(&path, &fixture()).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(load(&path).unwrap(), fixture());
        assert!(
            matches!(save_new(&path, &fixture()), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        let collision = save_new(&path, &fixture()).unwrap_err().to_string();
        let late_collision = Error::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "File exists",
        ))
        .to_string();
        assert_eq!(collision, late_collision);
        assert!(collision.contains("choose a new filename"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let huge = directory.join("huge.json");
        fs::write(&huge, vec![b' '; MAX_PRESET_BYTES + 1]).unwrap();
        assert!(matches!(load(&huge), Err(Error::Invalid(_))));
        assert_eq!(
            fs::read_dir(&directory).unwrap().count(),
            2,
            "no leaked temporary files"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
