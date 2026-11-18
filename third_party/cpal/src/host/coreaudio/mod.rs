//! CoreAudio backend implementation.
//!
//! Default backend on macOS and iOS.

use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger,
    kAudioFormatLinearPCM, AudioStreamBasicDescription,
};

use crate::DefaultStreamConfigError;
use crate::{BuildStreamError, SupportedStreamConfigsError};

use crate::{BackendSpecificError, SampleFormat, StreamConfig};

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "ios")]
#[allow(unused_imports)]
pub use self::ios::{
    enumerate::{Devices, SupportedInputConfigs, SupportedOutputConfigs},
    Device, Host, Stream,
};

#[cfg(target_os = "macos")]
pub use self::macos::{Host, OutputTiming, OutputTimingReceipt, Stream};

// Common helper methods used by both macOS and iOS

fn check_os_status(os_status: OSStatus) -> Result<(), BackendSpecificError> {
    match coreaudio::Error::from_os_status(os_status) {
        Ok(()) => Ok(()),
        Err(err) => {
            let description = err.to_string();
            Err(BackendSpecificError { description })
        }
    }
}

// Create a coreaudio AudioStreamBasicDescription from a CPAL Format.
fn asbd_from_config(
    config: &StreamConfig,
    sample_format: SampleFormat,
) -> AudioStreamBasicDescription {
    let n_channels = config.channels as usize;
    let sample_rate = config.sample_rate;
    let bytes_per_channel = sample_format.sample_size();
    let bits_per_channel = bytes_per_channel * 8;
    let bytes_per_frame = n_channels * bytes_per_channel;
    let frames_per_packet = 1;
    let bytes_per_packet = frames_per_packet * bytes_per_frame;
    let format_flags = match sample_format {
        SampleFormat::F32 | SampleFormat::F64 => kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        SampleFormat::I8
        | SampleFormat::I16
        | SampleFormat::I24
        | SampleFormat::I32
        | SampleFormat::I64 => kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
        _ => kAudioFormatFlagIsPacked,
    };
    AudioStreamBasicDescription {
        mBitsPerChannel: bits_per_channel as _,
        mBytesPerFrame: bytes_per_frame as _,
        mChannelsPerFrame: n_channels as _,
        mBytesPerPacket: bytes_per_packet as _,
        mFramesPerPacket: frames_per_packet as _,
        mFormatFlags: format_flags,
        mFormatID: kAudioFormatLinearPCM,
        mSampleRate: sample_rate as _,
        mReserved: 0,
    }
}

fn read_host_timebase() -> Result<mach2::mach_time::mach_timebase_info, BackendSpecificError> {
    let mut info: mach2::mach_time::mach_timebase_info = Default::default();
    // SAFETY: the OS writes the timebase into this initialized local value.
    let res = unsafe { mach2::mach_time::mach_timebase_info(&mut info) };
    check_os_status(res)?;
    Ok(info)
}

#[cfg(target_os = "ios")]
#[inline]
fn host_time_to_stream_instant(
    m_host_time: u64,
) -> Result<crate::StreamInstant, BackendSpecificError> {
    let info = read_host_timebase()?;
    let nanos = m_host_time * info.numer as u64 / info.denom as u64;
    let secs = nanos / 1_000_000_000;
    let subsec_nanos = nanos - secs * 1_000_000_000;
    Ok(crate::StreamInstant::new(secs as i64, subsec_nanos as u32))
}

/// The boot-wide conversion ratio for timestamps already supplied by Core Audio.
/// Reading the Mach timebase can enter the kernel during initialization. Prepare
/// it before stream startup. This value does not read or advance a clock.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct HostTimebase {
    numerator: std::num::NonZeroU32,
    denominator: std::num::NonZeroU32,
}

#[cfg(target_os = "macos")]
impl HostTimebase {
    fn prepare() -> Result<Self, BackendSpecificError> {
        let info = read_host_timebase()?;
        Self::new(info.numer, info.denom).ok_or_else(|| BackendSpecificError {
            description: "invalid Core Audio host timebase".into(),
        })
    }

    fn new(numerator: u32, denominator: u32) -> Option<Self> {
        Some(Self {
            numerator: std::num::NonZeroU32::new(numerator)?,
            denominator: std::num::NonZeroU32::new(denominator)?,
        })
    }

    /// Convert the callback instant and its capture or playback instant in
    /// the same clock domain. Pre-boot and unrepresentable instants refuse.
    fn timestamps(
        self,
        host_ticks: u64,
        endpoint_offset_ns: i128,
    ) -> Option<(crate::StreamInstant, crate::StreamInstant)> {
        // A u64 tick count times a u32 numerator fits in i128. Divide after
        // widening to keep the exact floor when the u64 product would wrap.
        let callback_ns = i128::from(host_ticks) * i128::from(self.numerator.get())
            / i128::from(self.denominator.get());
        let endpoint_ns = callback_ns.checked_add(endpoint_offset_ns)?;
        if endpoint_ns < 0 {
            return None;
        }
        Some((
            crate::StreamInstant::from_nanos_i128(callback_ns)?,
            crate::StreamInstant::from_nanos_i128(endpoint_ns)?,
        ))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod host_timebase_regression {
    use super::HostTimebase;
    use crate::StreamInstant;

    #[test]
    fn native_timebase_preserves_nonzero_device_offsets() {
        let timebase = HostTimebase::prepare().unwrap();
        let (callback, capture) = timebase.timestamps(1_000_000_000, -271).unwrap();
        let (same_callback, playback) = timebase.timestamps(1_000_000_000, 271).unwrap();
        assert_eq!(callback, same_callback);
        assert_eq!(callback.duration_since(&capture).unwrap().as_nanos(), 271);
        assert_eq!(playback.duration_since(&callback).unwrap().as_nanos(), 271);
    }

    #[test]
    fn host_timebase_preserves_exact_floor_past_u64_product() {
        let timebase = HostTimebase::new(125, 3).unwrap();
        assert_eq!(
            timebase.timestamps(u64::MAX, 0),
            Some((
                StreamInstant::new(768_614_336_404, 564_650_625),
                StreamInstant::new(768_614_336_404, 564_650_625),
            )),
        );
        assert_eq!(
            timebase.timestamps(2, 1),
            Some((StreamInstant::new(0, 83), StreamInstant::new(0, 84))),
        );
    }

    #[test]
    fn host_timebase_rejects_invalid_ratios_and_timestamp_pairs() {
        assert!(HostTimebase::new(0, 1).is_none());
        assert!(HostTimebase::new(1, 0).is_none());
        assert!(HostTimebase::new(1, 1).unwrap().timestamps(3, -4).is_none());
        assert!(HostTimebase::new(u32::MAX, 1)
            .unwrap()
            .timestamps(u64::MAX, 0)
            .is_none());
        let max_ns = i128::from(i64::MAX) * 1_000_000_000 + 999_999_999;
        assert!(HostTimebase::new(1, 1)
            .unwrap()
            .timestamps(1, max_ns)
            .is_none());
    }
}

// Convert the given duration in frames at the given sample rate to a `std::time::Duration`.
#[inline]
fn frames_to_duration(frames: usize, rate: crate::SampleRate) -> std::time::Duration {
    let secsf = frames as f64 / rate as f64;
    let secs = secsf as u64;
    let nanos = ((secsf - secs as f64) * 1_000_000_000.0) as u32;
    std::time::Duration::new(secs, nanos)
}

// TODO need stronger error identification
impl From<coreaudio::Error> for BuildStreamError {
    fn from(err: coreaudio::Error) -> BuildStreamError {
        match err {
            coreaudio::Error::RenderCallbackBufferFormatDoesNotMatchAudioUnitStreamFormat
            | coreaudio::Error::NoKnownSubtype
            | coreaudio::Error::AudioUnit(coreaudio::error::AudioUnitError::FormatNotSupported)
            | coreaudio::Error::AudioCodec(_)
            | coreaudio::Error::AudioFormat(_) => BuildStreamError::StreamConfigNotSupported,
            _ => BuildStreamError::DeviceNotAvailable,
        }
    }
}

impl From<coreaudio::Error> for SupportedStreamConfigsError {
    fn from(err: coreaudio::Error) -> SupportedStreamConfigsError {
        let description = format!("{err}");
        let err = BackendSpecificError { description };
        // Check for possible DeviceNotAvailable variant
        SupportedStreamConfigsError::BackendSpecific { err }
    }
}

impl From<coreaudio::Error> for DefaultStreamConfigError {
    fn from(err: coreaudio::Error) -> DefaultStreamConfigError {
        let description = format!("{err}");
        let err = BackendSpecificError { description };
        // Check for possible DeviceNotAvailable variant
        DefaultStreamConfigError::BackendSpecific { err }
    }
}

pub(crate) type OSStatus = i32;

// Compile-time assertion that Stream is Send and Sync
crate::assert_stream_send!(Stream);
crate::assert_stream_sync!(Stream);
