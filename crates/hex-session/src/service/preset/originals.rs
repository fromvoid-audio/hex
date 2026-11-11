//! App-owned, immutable original-rate audio for portable presets. The preset
//! carries content identity; the worker decides the store root and byte budget.
use hex_engine::Frame;
use hex_engine::native_device_resources::{
    ResourcePreparationError, original_pcm_facts_on_control,
};
use hex_export::{ExportError, FLOAT32_WAV_HEADER_BYTES};
use hex_media::{DecodedAudio, ImportError, Limits};
use hex_project::{PortableDeviceAsset, PortableDeviceUnit};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
pub enum Error {
    Invalid(&'static str),
    File(io::Error),
    Decode(ImportError),
    Write(ExportError),
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::File(error) => write!(f, "preset original file: {error}"),
            Self::Decode(error) => write!(f, "could not read preset original audio: {error}"),
            Self::Write(error) => write!(f, "could not retain preset original audio: {error}"),
            Self::Cancelled => f.write_str("preset original audio operation canceled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Invalid(_) | Self::Cancelled => None,
        }
    }
}

pub struct OriginalStore {
    directory: PathBuf,
    max_frames: u64,
}

impl OriginalStore {
    pub fn new(data_directory: &Path, max_frames: u64) -> Result<Self, Error> {
        if data_directory.as_os_str().is_empty() || data_directory.as_os_str().len() > 4096 {
            return Err(Error::Invalid(
                "preset original storage needs an app data folder",
            ));
        }
        if max_frames == 0 {
            return Err(Error::Invalid(
                "preset original storage needs a frame budget",
            ));
        }
        Ok(Self {
            directory: data_directory.join("device-preset-media").join("v1"),
            max_frames,
        })
    }

    /// Save all active and inactive originals from the actual Session. A
    /// missing resident original refuses; project-rate clip audio cannot stand in.
    pub fn retain_unit(
        &self,
        session: &crate::Session,
        unit: &PortableDeviceUnit,
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        for &expected in unit.assets() {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            let source = session
                .project()
                .assets()
                .filter(|asset| {
                    asset
                        .original_source()
                        .is_none_or(|saved| saved == expected.facts)
                        && asset.source_sample_rate() == expected.source_rate
                        && asset.source_frames() == expected.source_frames
                })
                .find_map(|asset| {
                    let facts = session
                        .native_assets
                        .original_source_facts_on_control(asset.id())
                        .ok()?;
                    if facts != expected.facts {
                        return None;
                    }
                    session.native_source(asset.id())
                })
                .ok_or(Error::Invalid(
                    "preset original is unavailable in this project",
                ))?;
            let frames = source
                .resident_frames()
                .ok_or(Error::Invalid("preset original is not resident audio"))?;
            self.retain(expected, source.sample_rate(), frames, cancel)?;
        }
        Ok(())
    }

    /// Publish verified normalized stereo without overwriting an existing file.
    /// A digest collision at publication reopens and verifies the existing file.
    pub fn retain(
        &self,
        expected: PortableDeviceAsset,
        sample_rate: u32,
        frames: &[Frame],
        cancel: &AtomicBool,
    ) -> Result<PathBuf, Error> {
        self.limits(expected)?;
        self.verify(expected, sample_rate, frames, cancel)?;
        let path = self.path(expected);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                self.load(expected, cancel)?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::File(error)),
        }
        fs::create_dir_all(&self.directory).map_err(Error::File)?;
        let mut remaining = frames;
        let write = hex_export::write_float_wav_new(
            &path,
            sample_rate,
            expected.source_frames,
            cancel,
            |block| {
                let (read, tail) = remaining
                    .split_at_checked(block.len())
                    .ok_or(ExportError::Invalid("preset original audio is incomplete"))?;
                block.copy_from_slice(read);
                remaining = tail;
                Ok(())
            },
        );
        match write {
            Ok(_) => Ok(path),
            Err(ExportError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists =>
            {
                self.load(expected, cancel)?;
                Ok(path)
            }
            Err(ExportError::Cancelled) => Err(Error::Cancelled),
            Err(error) => Err(Error::Write(error)),
        }
    }

    /// Reopen the exact managed WAV, then prove its decoded rate, extent and
    /// original PCM content before returning anything to the prepared importer.
    pub fn load(
        &self,
        expected: PortableDeviceAsset,
        cancel: &AtomicBool,
    ) -> Result<DecodedAudio, Error> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let limits = self.limits(expected)?;
        let path = self.path(expected);
        let metadata = fs::symlink_metadata(&path).map_err(Error::File)?;
        if !metadata.is_file() || metadata.len() != limits.max_file_bytes as u64 {
            return Err(Error::Invalid(
                "preset original file has the wrong format or size",
            ));
        }
        let decoded =
            hex_media::decode_wav_file_cancellable(&path, limits, cancel).map_err(|error| {
                match error {
                    ImportError::Cancelled => Error::Cancelled,
                    error => Error::Decode(error),
                }
            })?;
        self.verify(
            expected,
            decoded.sample_rate(),
            decoded.audio().frames(),
            cancel,
        )?;
        Ok(decoded)
    }

    fn limits(&self, expected: PortableDeviceAsset) -> Result<Limits, Error> {
        if expected.source_frames == 0 || expected.source_frames > self.max_frames {
            return Err(Error::Invalid(
                "preset original audio exceeds the frame budget",
            ));
        }
        let frames = usize::try_from(expected.source_frames)
            .map_err(|_| Error::Invalid("preset original audio exceeds the frame range"))?;
        let bytes = expected
            .source_frames
            .checked_mul(std::mem::size_of::<Frame>() as u64)
            .and_then(|value| value.checked_add(FLOAT32_WAV_HEADER_BYTES))
            .filter(|&value| value.saturating_sub(8) <= u64::from(u32::MAX))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(Error::Invalid(
                "preset original WAV exceeds the file size limit",
            ))?;
        Ok(Limits::new(bytes, frames))
    }

    fn path(&self, expected: PortableDeviceAsset) -> PathBuf {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut filename = String::with_capacity(68);
        for byte in expected.facts.content_digest() {
            filename.push(char::from(HEX[usize::from(byte >> 4)]));
            filename.push(char::from(HEX[usize::from(byte & 15)]));
        }
        filename.push_str(".wav");
        self.directory.join(filename)
    }

    fn verify(
        &self,
        expected: PortableDeviceAsset,
        sample_rate: u32,
        frames: &[Frame],
        cancel: &AtomicBool,
    ) -> Result<(), Error> {
        if sample_rate != expected.source_rate
            || u64::try_from(frames.len()).ok() != Some(expected.source_frames)
        {
            return Err(Error::Invalid(
                "preset original rate or frame count does not match",
            ));
        }
        let actual = original_pcm_facts_on_control(sample_rate, frames, self.max_frames, cancel)
            .map_err(|error| match error {
                ResourcePreparationError::Cancelled => Error::Cancelled,
                _ => Error::Invalid("could not verify preset original audio"),
            })?;
        if actual != expected.facts {
            return Err(Error::Invalid(
                "preset original audio content does not match",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
