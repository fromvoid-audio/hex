//! Control-side discovery and selection of audio output devices through CPAL.
//!
//! Queries enumerate output devices without opening a stream or modifying hardware
//! properties. Selection matches exact CPAL device IDs (CoreAudio device UIDs on macOS)
//! so pinned interfaces never silently fall back to a different device.

use crate::output_route::{OutputRoute, route_of};
use cpal::traits::{DeviceTrait, HostTrait};

const MAX_OUTPUT_DEVICES: usize = 128;
const STANDARD_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Explicit output device and hardware buffer preference passed to the audio lifecycle lane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputDeviceSelection {
    /// Exact CPAL device identifier (`None` follows the system default output device).
    pub device_uid: Option<String>,
    /// Preferred hardware buffer size in sample frames (`None` keeps the device default).
    pub buffer_size_frames: Option<u32>,
}

/// Control-side snapshot of one audio output device and its reported capabilities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    /// Stable host-qualified CPAL identifier (CoreAudio UID on macOS).
    pub uid: String,
    /// Human-readable device name reported by the OS.
    pub name: String,
    /// Current nominal sample rate in hertz.
    pub nominal_sample_rate_hz: u32,
    /// Output channel count reported for the default config.
    pub channels: u16,
    /// Minimum hardware buffer size in frames when reported by the backend.
    pub min_buffer_frames: Option<u32>,
    /// Maximum hardware buffer size in frames when reported by the backend.
    pub max_buffer_frames: Option<u32>,
    /// Standard sample rates (`44.1..=192 kHz`) reported as supported by this device.
    pub supported_sample_rates: Vec<u32>,
    /// Whether this device was the OS default output at probe time.
    pub is_default: bool,
}

impl OutputDeviceInfo {
    /// Check whether `frames` is admitted by this device's reported hardware buffer range.
    /// Returns `Ok(())` when supported, or a clear lowercase refusal reason.
    pub fn validate_buffer_size(&self, frames: u32) -> Result<(), String> {
        let (Some(min), Some(max)) = (self.min_buffer_frames, self.max_buffer_frames) else {
            return Err("selected device does not expose configurable buffer sizes".into());
        };
        if min == 0 || min > max {
            return Err("selected device reported an invalid buffer size range".into());
        }
        if frames < min || frames > max {
            return Err(format!(
                "buffer size {frames} frames is outside device range {min}..={max}"
            ));
        }
        Ok(())
    }

    /// Check whether `rate_hz` is natively listed by this device's supported configs.
    pub fn supports_native_sample_rate(&self, rate_hz: u32) -> bool {
        self.supported_sample_rates.contains(&rate_hz) || self.nominal_sample_rate_hz == rate_hz
    }
}

/// Bounded snapshot of discovered audio output devices on the current host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputDevicesSnapshot {
    pub devices: Vec<OutputDeviceInfo>,
    pub default_uid: Option<String>,
}

impl OutputDevicesSnapshot {
    /// Resolve an exact pinned device UID in this snapshot.
    pub fn find_by_uid(&self, uid: &str) -> Option<&OutputDeviceInfo> {
        self.devices.iter().find(|device| device.uid == uid)
    }

    /// Resolve the current default device entry when present in this snapshot.
    pub fn default_device(&self) -> Option<&OutputDeviceInfo> {
        self.default_uid
            .as_deref()
            .and_then(|uid| self.find_by_uid(uid))
    }
}

/// Compute one-way hardware buffer latency in milliseconds for `frames` at `sample_rate_hz`.
pub fn buffer_latency_ms(frames: u32, sample_rate_hz: u32) -> Option<f64> {
    if frames == 0 || sample_rate_hz == 0 {
        return None;
    }
    Some((f64::from(frames) / f64::from(sample_rate_hz)) * 1_000.0)
}

/// Enumerate audio output devices on a control/background thread.
pub fn probe_output_devices_on_control() -> Result<OutputDevicesSnapshot, String> {
    let host = cpal::default_host();
    let default_uid = host
        .default_output_device()
        .and_then(|device| device.id().ok())
        .map(|id| id.to_string());
    let iter = host
        .output_devices()
        .map_err(|error| format!("could not enumerate audio output devices: {error}"))?;
    let mut devices = Vec::new();
    for device in iter.take(MAX_OUTPUT_DEVICES) {
        let Ok(id) = device.id() else {
            continue;
        };
        let uid = id.to_string();
        if uid.is_empty() {
            continue;
        }
        let Ok(default_cfg) = device.default_output_config() else {
            continue;
        };
        let name = device
            .description()
            .map(|desc| desc.name().to_owned())
            .unwrap_or_else(|_| uid.clone());
        let (min_buffer_frames, max_buffer_frames) = match *default_cfg.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } if min > 0 && min <= max => {
                (Some(min), Some(max))
            }
            _ => (None, None),
        };
        let mut supported_sample_rates = Vec::new();
        if let Ok(configs) = device.supported_output_configs() {
            let mut ranges = Vec::new();
            for cfg in configs.take(128) {
                if cfg.channels() > 0 {
                    ranges.push((cfg.min_sample_rate(), cfg.max_sample_rate()));
                }
            }
            for rate in STANDARD_SAMPLE_RATES {
                if ranges.iter().any(|&(min, max)| (min..=max).contains(&rate)) {
                    supported_sample_rates.push(rate);
                }
            }
        }
        let nominal_sample_rate_hz = default_cfg.sample_rate();
        if !supported_sample_rates.contains(&nominal_sample_rate_hz)
            && (8_000..=192_000).contains(&nominal_sample_rate_hz)
        {
            supported_sample_rates.push(nominal_sample_rate_hz);
            supported_sample_rates.sort_unstable();
        }
        let is_default = default_uid.as_deref() == Some(uid.as_str());
        devices.push(OutputDeviceInfo {
            uid,
            name,
            nominal_sample_rate_hz,
            channels: default_cfg.channels(),
            min_buffer_frames,
            max_buffer_frames,
            supported_sample_rates,
            is_default,
        });
    }
    Ok(OutputDevicesSnapshot {
        devices,
        default_uid,
    })
}

/// Resolve the CPAL output device for `device_uid` (`None` returns the system default).
/// Refuses explicitly with a clear lowercase error when a pinned UID is not available.
pub(crate) fn resolve_output_device_on_control(
    device_uid: Option<&str>,
) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match device_uid {
        None => host
            .default_output_device()
            .ok_or_else(|| "no default audio output device".to_owned()),
        Some(uid) => {
            let iter = host
                .output_devices()
                .map_err(|error| format!("could not enumerate audio output devices: {error}"))?;
            for device in iter.take(MAX_OUTPUT_DEVICES) {
                if device.id().ok().map(|id| id.to_string()).as_deref() == Some(uid) {
                    return Ok(device);
                }
            }
            Err("selected audio output device is unavailable".to_owned())
        }
    }
}

/// Probe the route metadata of either a pinned output device or the system default.
pub fn probe_output_route_for_device_on_control(device_uid: Option<&str>) -> Option<OutputRoute> {
    let device = resolve_output_device_on_control(device_uid).ok()?;
    route_of(&device)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_device(min: Option<u32>, max: Option<u32>) -> OutputDeviceInfo {
        OutputDeviceInfo {
            uid: "coreaudio:TestInterface".into(),
            name: "Test Interface".into(),
            nominal_sample_rate_hz: 48_000,
            channels: 2,
            min_buffer_frames: min,
            max_buffer_frames: max,
            supported_sample_rates: vec![44_100, 48_000, 96_000],
            is_default: false,
        }
    }

    #[test]
    fn buffer_size_validation_reports_clear_errors_for_unsupported_ranges() {
        let dev = sample_device(Some(64), Some(512));
        assert!(dev.validate_buffer_size(64).is_ok());
        assert!(dev.validate_buffer_size(256).is_ok());
        assert!(dev.validate_buffer_size(512).is_ok());
        let too_large = dev.validate_buffer_size(1024).unwrap_err();
        assert!(too_large.contains("1024"));
        assert!(too_large.contains("64..=512"));

        let unknown = sample_device(None, None);
        assert!(
            unknown
                .validate_buffer_size(128)
                .unwrap_err()
                .contains("configurable buffer sizes")
        );
    }

    #[test]
    fn unavailable_pinned_device_returns_clean_lowercase_error() {
        let err =
            resolve_output_device_on_control(Some("coreaudio:nonexistent-device-uid-hex-test"))
                .err()
                .expect("nonexistent pinned UID must fail");
        assert_eq!(err, "selected audio output device is unavailable");
    }

    #[test]
    fn latency_calculation_handles_valid_and_zero_inputs() {
        assert_eq!(buffer_latency_ms(0, 48_000), None);
        assert_eq!(buffer_latency_ms(256, 0), None);
        let ms = buffer_latency_ms(256, 48_000).unwrap();
        assert!((ms - 5.333_333_3).abs() < 1e-4);
    }
}
