//! Embedded app-owned vector assets for GPUI's existing asset source.
//!
//! Paths are stable resource identifiers, not user filesystem paths. SVGs are
//! compiled into the application and painted with the caller's theme color.
use std::borrow::Cow;

/// The fixed, bounded vector library used by Hex windows.
pub struct HexAssets;

const ICONS: [(&str, &[u8]); 10] = [
    (
        "icons/browser-place/folder.svg",
        include_bytes!("../../../assets/icons/browser-place/folder.svg"),
    ),
    (
        "icons/browser-place/samples.svg",
        include_bytes!("../../../assets/icons/browser-place/samples.svg"),
    ),
    (
        "icons/browser-place/devices.svg",
        include_bytes!("../../../assets/icons/browser-place/devices.svg"),
    ),
    (
        "icons/browser-place/projects.svg",
        include_bytes!("../../../assets/icons/browser-place/projects.svg"),
    ),
    (
        "icons/browser-place/collection.svg",
        include_bytes!("../../../assets/icons/browser-place/collection.svg"),
    ),
    (
        "icons/browser-place/waveform.svg",
        include_bytes!("../../../assets/icons/browser-place/waveform.svg"),
    ),
    (
        "icons/browser-place/search.svg",
        include_bytes!("../../../assets/icons/browser-place/search.svg"),
    ),
    (
        "icons/transport/play.svg",
        include_bytes!("../../../assets/icons/transport/play.svg"),
    ),
    (
        "icons/transport/pause.svg",
        include_bytes!("../../../assets/icons/transport/pause.svg"),
    ),
    (
        "icons/transport/stop.svg",
        include_bytes!("../../../assets/icons/transport/stop.svg"),
    ),
];

impl gpui::AssetSource for HexAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, data)| Cow::Borrowed(*data)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}

/// Refuse to open an editor whose host did not install the embedded catalog.
pub(crate) fn validate(cx: &gpui::App) -> Result<(), String> {
    for (path, expected) in ICONS {
        let actual = cx
            .asset_source()
            .load(path)
            .map_err(|_| format!("could not load embedded icon: {path}"))?;
        if actual.as_deref() != Some(expected) {
            return Err(format!("embedded icon unavailable or changed: {path}"));
        }
    }
    Ok(())
}
