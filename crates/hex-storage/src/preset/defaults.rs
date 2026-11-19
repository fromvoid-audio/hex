//! App-owned per-class/provider defaults using the existing preset envelope.
//!
//! Call only on a control worker. The explicit root belongs to the application's
//! configured data directory; this API never replaces arbitrary external paths.
//! Native provider validation remains the caller's responsibility, including both
//! A/B states. The historical state-only API refuses host trims and complete
//! units. The typed API preserves the complete authored tree and its media manifest.
use super::{
    DeviceClass, Error, Preset, PresetContents, encode, encode_unit, load as load_preset,
    load_contents as load_preset_contents, stage,
};
use hex_lattice_contract::DeviceTypeId;
use hex_project::PortableDeviceUnit;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Validated IDs contain only bounded lowercase ASCII letters, digits, dots,
/// underscores and hyphens. A constant prefix avoids reserved device filenames;
/// the complete filename stays below common255-byte component limits.
pub fn path(root: &Path, class: DeviceClass, provider: DeviceTypeId) -> Result<PathBuf, Error> {
    provider
        .validate()
        .map_err(|_| Error::Invalid("invalid device default provider"))?;
    let name = provider
        .as_str()
        .map_err(|_| Error::Invalid("invalid device default provider"))?;
    Ok(root
        .join("device-defaults")
        .join("v1")
        .join(match class {
            DeviceClass::Instrument => "instrument",
            DeviceClass::Effect => "effect",
        })
        .join(format!("device-{name}.{}", super::EXTENSION)))
}

/// Missing means no user default. Malformed/newer envelopes and identity
/// mismatches are errors; callers must not silently substitute a factory state.
pub fn load(
    root: &Path,
    class: DeviceClass,
    provider: DeviceTypeId,
) -> Result<Option<Preset>, Error> {
    let path = path(root, class, provider)?;
    let preset = match load_preset(&path) {
        Ok(preset) => preset,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if preset.class != class || preset.state.type_id() != provider {
        return Err(Error::Invalid(
            "device default class or provider does not match",
        ));
    }
    Ok(Some(preset))
}

/// Read a historical state default or a complete unit for this exact slot.
pub fn load_contents(
    root: &Path,
    class: DeviceClass,
    provider: DeviceTypeId,
) -> Result<Option<PresetContents>, Error> {
    let path = path(root, class, provider)?;
    let contents = match load_preset_contents(&path) {
        Ok(contents) => contents,
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if contents.identity() != (class, provider) {
        return Err(Error::Invalid(
            "device default class or provider does not match",
        ));
    }
    Ok(Some(contents))
}

/// Publish an authored whole-unit default at its root's class/provider key.
/// Existing malformed or unsupported defaults are preserved.
pub fn save_unit(root: &Path, unit: &PortableDeviceUnit) -> Result<(), Error> {
    let class = DeviceClass::from(unit.class());
    let provider = unit.nodes()[0].state.type_id();
    let path = path(root, class, provider)?;
    let bytes = encode_unit(unit)?;
    load_contents(root, class, provider)?;
    publish_checked(
        path,
        &bytes,
        |from, to| fs::rename(from, to),
        sync_directory,
    )
}

/// Replace this app-owned default with a complete, synced preset using a
/// same-directory rename. Existing malformed/newer envelopes are preserved.
/// Successful concurrent saves use last-publication-wins settings semantics.
///
/// Validation, staging and rename failures leave the previous destination intact.
/// A directory-sync failure after rename returns `PublishedButNotDurable`: the
/// complete new default is already visible and cannot honestly be called rolled back.
pub fn save(root: &Path, preset: &Preset) -> Result<(), Error> {
    save_using(
        root,
        preset,
        |from, to| fs::rename(from, to),
        sync_directory,
    )
}

fn save_using(
    root: &Path,
    preset: &Preset,
    publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), Error> {
    let path = path(root, preset.class, preset.state.type_id())?;
    if preset.state.input_gain_millidb() != 0 || preset.state.output_gain_millidb() != 0 {
        return Err(Error::Invalid(
            "device default format does not support host trims",
        ));
    }
    let bytes = encode(preset)?;
    // Preserve unsupported files rather than destroying a default from another
    // app version or accepting an identity collision as this provider's state.
    load(root, preset.class, preset.state.type_id())?;
    publish_checked(path, &bytes, publish, sync)
}

fn publish_checked(
    path: PathBuf,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or(Error::Invalid("device default folder is missing"))?;
    fs::create_dir_all(parent)?;
    let temporary = stage(parent, bytes)?;
    publish(&temporary.0, &path)?;
    if let Err(source) = sync(parent) {
        return Err(Error::PublishedButNotDurable { path, source });
    }
    Ok(())
}

fn sync_directory(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
