use super::OSStatus;
use super::Stream;
use super::{asbd_from_config, check_os_status, frames_to_duration, host_time_to_stream_instant};
use crate::host::coreaudio::macos::loopback::LoopbackDevice;
use crate::host::coreaudio::macos::StreamInner;
use crate::traits::DeviceTrait;
use crate::{
    BackendSpecificError, BufferSize, BuildStreamError, ChannelCount, Data,
    DefaultStreamConfigError, DeviceId, DeviceIdError, DeviceNameError, InputCallbackInfo,
    OutputCallbackInfo, SampleFormat, SampleRate, StreamConfig, StreamError, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange, SupportedStreamConfigsError,
};
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, Scope};
use objc2_audio_toolbox::{
    kAudioOutputUnitProperty_CurrentDevice, kAudioOutputUnitProperty_EnableIO,
    kAudioUnitProperty_Latency, kAudioUnitProperty_MaximumFramesPerSlice,
    kAudioUnitProperty_StreamFormat, AudioUnitAddPropertyListener, AudioUnitGetProperty,
    AudioUnitRemovePropertyListenerWithUserData,
};
use objc2_core_audio::kAudioDevicePropertyDeviceUID;
use objc2_core_audio::kAudioObjectPropertyElementMain;
use objc2_core_audio::{
    kAudioAggregateDeviceClassID, kAudioDevicePropertyAvailableNominalSampleRates,
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyBufferFrameSizeRange,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreamConfiguration,
    kAudioDevicePropertyStreamFormat, kAudioObjectPropertyClass, kAudioObjectPropertyElementMaster,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, AudioClassID, AudioDeviceID, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertyScope, AudioObjectSetPropertyData,
};
use objc2_core_audio::{
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyLatency,
    kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectSystemObject,
    kAudioStreamPropertyLatency, AudioObjectAddPropertyListener, AudioObjectRemovePropertyListener,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioValueRange,
};
use objc2_core_foundation::CFString;
use objc2_core_foundation::Type;

pub use super::enumerate::{default_output_device, SupportedInputConfigs, SupportedOutputConfigs};
use std::ffi::c_void;
use std::fmt;
use std::mem::{self, size_of, size_of_val};
use std::ptr::{null, NonNull};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::invoke_error_callback;
use super::property_listener::AudioObjectPropertyListener;
use coreaudio::audio_unit::macos_helpers::get_device_name;

/// Attempt to set the device sample rate to the provided rate.
/// Return an error if the requested sample rate is not supported by the device.
fn set_sample_rate(
    audio_device_id: AudioObjectID,
    target_sample_rate: SampleRate,
) -> Result<(), BuildStreamError> {
    // Get the current sample rate.
    let mut property_address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMaster,
    };
    let mut sample_rate: f64 = 0.0;
    let mut data_size = mem::size_of::<f64>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            audio_device_id,
            NonNull::from(&property_address),
            0,
            null(),
            NonNull::from(&mut data_size),
            NonNull::from(&mut sample_rate).cast(),
        )
    };
    coreaudio::Error::from_os_status(status)?;

    // If the requested sample rate is different to the device sample rate, update the device.
    if sample_rate as u32 != target_sample_rate {
        // Get available sample rate ranges.
        property_address.mSelector = kAudioDevicePropertyAvailableNominalSampleRates;
        let mut data_size = 0u32;
        let status = unsafe {
            AudioObjectGetPropertyDataSize(
                audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
            )
        };
        coreaudio::Error::from_os_status(status)?;
        let n_ranges = data_size as usize / mem::size_of::<AudioValueRange>();
        let mut ranges: Vec<AudioValueRange> = Vec::with_capacity(n_ranges);
        let status = unsafe {
            AudioObjectGetPropertyData(
                audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
                NonNull::new(ranges.as_mut_ptr()).unwrap().cast(),
            )
        };
        coreaudio::Error::from_os_status(status)?;
        unsafe {
            ranges.set_len(n_ranges);
        }

        // Now that we have the available ranges, pick the one matching the desired rate.
        let sample_rate = target_sample_rate;
        if !ranges
            .iter()
            .any(|r| sample_rate as f64 >= r.mMinimum && sample_rate as f64 <= r.mMaximum)
        {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }

        let (send, recv) = channel::<Result<f64, coreaudio::Error>>();
        let sample_rate_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyNominalSampleRate,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMaster,
        };
        // Send sample rate updates back on a channel.
        let sample_rate_handler = move || {
            let mut rate: f64 = 0.0;
            let mut data_size = mem::size_of::<f64>() as u32;

            let result = unsafe {
                AudioObjectGetPropertyData(
                    audio_device_id,
                    NonNull::from(&sample_rate_address),
                    0,
                    null(),
                    NonNull::from(&mut data_size),
                    NonNull::from(&mut rate).cast(),
                )
            };
            send.send(coreaudio::Error::from_os_status(result).map(|_| rate))
                .ok();
        };

        let listener = AudioObjectPropertyListener::new(
            audio_device_id,
            sample_rate_address,
            sample_rate_handler,
        )?;

        // Finally, set the sample rate.
        property_address.mSelector = kAudioDevicePropertyNominalSampleRate;
        // Set the nominal sample rate using a single f64 as required by CoreAudio.
        let rate = sample_rate as f64;
        let data_size = mem::size_of::<f64>() as u32;
        let status = unsafe {
            AudioObjectSetPropertyData(
                audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                data_size,
                NonNull::from(&rate).cast(),
            )
        };
        coreaudio::Error::from_os_status(status)?;

        // Wait for the reported_rate to change.
        //
        // This should not take longer than a few ms, but we timeout after 1 sec just in case.
        // We loop over potentially several events from the channel to ensure
        // that we catch the expected change in sample rate.
        let mut timeout = Duration::from_secs(1);
        let start = Instant::now();

        loop {
            match recv.recv_timeout(timeout) {
                Err(err) => {
                    let description = match err {
                        RecvTimeoutError::Disconnected => {
                            "sample rate listener channel disconnected unexpectedly"
                        }
                        RecvTimeoutError::Timeout => {
                            "timeout waiting for sample rate update for device"
                        }
                    }
                    .to_string();
                    return Err(BackendSpecificError { description }.into());
                }
                Ok(Ok(reported_sample_rate)) => {
                    if reported_sample_rate == target_sample_rate as f64 {
                        break;
                    }
                }
                Ok(Err(_)) => {
                    // TODO: should we consider collecting this error?
                }
            };
            timeout = timeout
                .checked_sub(start.elapsed())
                .unwrap_or(Duration::ZERO);
        }
        listener.remove()?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static AUDIO_UNIT_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn audio_unit_from_device(device: &Device, input: bool) -> Result<AudioUnit, coreaudio::Error> {
    audio_unit_on_device(device, input, false)
}

fn audio_unit_on_device(
    device: &Device,
    input: bool,
    pin_output: bool,
) -> Result<AudioUnit, coreaudio::Error> {
    #[cfg(test)]
    AUDIO_UNIT_CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
    let output_type = if !pin_output && !input && is_default_output_device(device) {
        coreaudio::audio_unit::IOType::DefaultOutput
    } else {
        coreaudio::audio_unit::IOType::HalOutput
    };
    let mut audio_unit = AudioUnit::new(output_type)?;

    if input {
        // Enable input processing.
        let enable_input = 1u32;
        audio_unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Input,
            Element::Input,
            Some(&enable_input),
        )?;

        // Disable output processing.
        let disable_output = 0u32;
        audio_unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Output,
            Element::Output,
            Some(&disable_output),
        )?;
    }

    // Device selection is a device-level property: always use Scope::Global + Element::Output
    audio_unit.set_property(
        kAudioOutputUnitProperty_CurrentDevice,
        Scope::Global,
        Element::Output,
        Some(&device.audio_device_id),
    )?;

    Ok(audio_unit)
}

#[cfg(test)]
mod route_probe_regression {
    use super::*;

    #[test]
    fn repeated_default_output_config_does_not_construct_audio_units() {
        let device =
            default_output_device().expect("this native regression needs a default output");
        let before = AUDIO_UNIT_CONSTRUCTIONS.with(|count| count.get());
        for _ in 0..4 {
            let config = device
                .default_output_config()
                .expect("default output config");
            assert!(config.sample_rate() > 0);
        }
        let after = AUDIO_UNIT_CONSTRUCTIONS.with(|count| count.get());
        assert_eq!(
            before, after,
            "route polling must not construct Audio Units"
        );
    }

    #[test]
    fn direct_default_buffer_range_matches_existing_audio_unit_query() {
        let device =
            default_output_device().expect("this native regression needs a default output");
        let config = device
            .default_output_config()
            .expect("default output config");
        let unit = audio_unit_from_device(&device, false).expect("output Audio Unit");
        let previous = get_io_buffer_frame_size_range(&unit).expect("Audio Unit buffer range");
        assert_eq!(*config.buffer_size(), previous);
    }
}

fn get_io_buffer_frame_size_range(
    audio_unit: &AudioUnit,
) -> Result<SupportedBufferSize, coreaudio::Error> {
    // Device-level property: always use Scope::Global + Element::Output
    // regardless of whether this audio unit is configured for input or output
    let buffer_size_range: AudioValueRange = audio_unit.get_property(
        kAudioDevicePropertyBufferFrameSizeRange,
        Scope::Global,
        Element::Output,
    )?;

    Ok(SupportedBufferSize::Range {
        min: buffer_size_range.mMinimum as u32,
        max: buffer_size_range.mMaximum as u32,
    })
}

impl DeviceTrait for Device {
    type SupportedInputConfigs = SupportedInputConfigs;
    type SupportedOutputConfigs = SupportedOutputConfigs;
    type Stream = Stream;

    fn description(&self) -> Result<crate::DeviceDescription, DeviceNameError> {
        Device::description(self)
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        Device::id(self)
    }

    fn supported_input_configs(
        &self,
    ) -> Result<Self::SupportedInputConfigs, SupportedStreamConfigsError> {
        Device::supported_input_configs(self)
    }

    fn supported_output_configs(
        &self,
    ) -> Result<Self::SupportedOutputConfigs, SupportedStreamConfigsError> {
        Device::supported_output_configs(self)
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_input_config(self)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_output_config(self)
    }

    fn build_input_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        data_callback: D,
        error_callback: E,
        timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        Device::build_input_stream_raw(
            self,
            config,
            sample_format,
            data_callback,
            error_callback,
            timeout,
        )
    }

    fn build_output_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        data_callback: D,
        error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Self::Stream, BuildStreamError>
    where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        self.build_output_stream_with_limit(
            config,
            sample_format,
            data_callback,
            error_callback,
            None,
        )
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Device {
    pub(crate) audio_device_id: AudioDeviceID,
}

fn is_default_output_device(device: &Device) -> bool {
    default_output_device().is_some_and(|d| d.audio_device_id == device.audio_device_id)
}

// The input stream is pinned to the selected device even when it is the
// system default. Its disconnection must reach the stream's error callback.
fn input_disconnect_callback<E>(callback: Arc<Mutex<E>>) -> super::ErrorCallback
where
    E: FnMut(StreamError) + Send + 'static,
{
    Box::new(move |err| {
        invoke_error_callback(&callback, err);
    })
}

#[cfg(test)]
mod input_disconnect_regression {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn device_unavailable_reaches_the_input_error_callback() {
        let observed = Arc::new(AtomicUsize::new(0));
        let target = Arc::clone(&observed);
        let callback = Arc::new(Mutex::new(move |error| {
            if matches!(error, StreamError::DeviceNotAvailable) {
                target.fetch_add(1, Ordering::Relaxed);
            }
        }));
        // Input stream construction uses this same callback for every selected
        // device, without a separate no-op route for the default device.
        let mut disconnect = input_disconnect_callback(callback);
        disconnect(StreamError::DeviceNotAvailable);
        assert_eq!(observed.load(Ordering::Relaxed), 1);
    }
}

impl Device {
    /// Construct a new device given its ID.
    /// Useful for constructing hidden devices.
    pub fn new(audio_device_id: AudioDeviceID) -> Self {
        Self { audio_device_id }
    }

    /// Checks if this device is an aggregate device.
    ///
    /// Aggregate devices combine multiple physical devices into a single logical device.
    fn is_aggregate_device(&self) -> bool {
        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioObjectPropertyClass,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };

        let mut class_id: AudioClassID = 0;
        let data_size = size_of::<AudioClassID>() as u32;

        // SAFETY: AudioObjectGetPropertyData is documented to write an AudioClassID
        // for kAudioObjectPropertyClass. We check the status before using the value.
        let status = unsafe {
            AudioObjectGetPropertyData(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&data_size),
                NonNull::from(&mut class_id).cast(),
            )
        };

        // If successful, check if it's an aggregate device
        status == 0 && class_id == kAudioAggregateDeviceClassID
    }

    fn description(&self) -> Result<crate::DeviceDescription, DeviceNameError> {
        let name = get_device_name(self.audio_device_id).map_err(|err| {
            DeviceNameError::BackendSpecific {
                err: BackendSpecificError {
                    description: err.to_string(),
                },
            }
        })?;

        let input_configs = self
            .supported_input_configs()
            .map(|configs| configs.count() as ChannelCount)
            .ok();
        let output_configs = self
            .supported_output_configs()
            .map(|configs| configs.count() as ChannelCount)
            .ok();

        let direction =
            crate::device_description::direction_from_counts(input_configs, output_configs);

        let mut builder = crate::DeviceDescriptionBuilder::new(name).direction(direction);

        // Check if this is an aggregate device
        if self.is_aggregate_device() {
            builder = builder.interface_type(crate::InterfaceType::Aggregate);
        }

        Ok(builder.build())
    }

    fn id(&self) -> Result<DeviceId, DeviceIdError> {
        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyDeviceUID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        };

        // CFString is copied from the audio object, use wrap_under_create_rule
        let mut uid: *mut CFString = std::ptr::null_mut();
        let mut data_size = size_of::<*mut CFString>() as u32;

        // SAFETY: AudioObjectGetPropertyData is documented to write a CFString pointer
        // for kAudioDevicePropertyDeviceUID. We check the status code before use.
        let status = unsafe {
            AudioObjectGetPropertyData(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
                NonNull::from(&mut uid).cast(),
            )
        };
        check_os_status(status)?;

        // SAFETY: Status was successful, meaning the API call succeeded.
        // We now check if the returned uid is non-null before use.
        if !uid.is_null() {
            let uid_string = unsafe { CFString::wrap_under_create_rule(uid).to_string() };
            Ok(DeviceId(crate::platform::HostId::CoreAudio, uid_string))
        } else {
            Err(DeviceIdError::BackendSpecific {
                err: BackendSpecificError {
                    description: "Device UID is null".to_string(),
                },
            })
        }
    }

    // Logic re-used between `supported_input_configs` and `supported_output_configs`.
    #[allow(clippy::cast_ptr_alignment)]
    fn supported_configs(
        &self,
        scope: AudioObjectPropertyScope,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        let mut property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyStreamConfiguration,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMaster,
        };

        unsafe {
            // Retrieve the devices audio buffer list.
            let mut data_size = 0u32;
            let status = AudioObjectGetPropertyDataSize(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
            );
            check_os_status(status)?;

            let mut audio_buffer_list: Vec<u8> = vec![];
            audio_buffer_list.reserve_exact(data_size as usize);
            let status = AudioObjectGetPropertyData(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
                NonNull::new(audio_buffer_list.as_mut_ptr()).unwrap().cast(),
            );
            check_os_status(status)?;

            let audio_buffer_list = audio_buffer_list.as_mut_ptr() as *mut AudioBufferList;

            // Read the number of buffers without assuming alignment (avoid UB).
            let nb_ptr = core::ptr::addr_of!((*audio_buffer_list).mNumberBuffers);
            let n_buffers = core::ptr::read_unaligned(nb_ptr) as usize;
            // If there are no buffers, skip.
            if n_buffers == 0 {
                return Ok(vec![].into_iter());
            }

            // Count the number of channels as the sum of all channels in all output buffers.
            let first_buf_ptr =
                core::ptr::addr_of!((*audio_buffer_list).mBuffers) as *const AudioBuffer;
            let mut n_channels = 0usize;
            for i in 0..n_buffers {
                let buf_ptr = first_buf_ptr.add(i);
                // Read potentially unaligned
                let buf: AudioBuffer = core::ptr::read_unaligned(buf_ptr);
                n_channels += buf.mNumberChannels as usize;
            }

            // TODO: macOS should support U8, I16, I32, F32 and F64. This should allow for using
            // I16 but just use F32 for now as it's the default anyway.
            let sample_format = SampleFormat::F32;

            // Get available sample rate ranges.
            // The property "kAudioDevicePropertyAvailableNominalSampleRates" returns a list of pairs of
            // minimum and maximum sample rates but most of the devices returns pairs of same values though the underlying mechanism is unclear.
            // This may cause issues when, for example, sorting the configs by the sample rates.
            // We follows the implementation of RtAudio, which returns single element of config
            // when all the pairs have the same values and returns multiple elements otherwise.
            // See https://github.com/thestk/rtaudio/blob/master/RtAudio.cpp#L1369C1-L1375C39

            property_address.mSelector = kAudioDevicePropertyAvailableNominalSampleRates;
            let mut data_size = 0u32;
            let status = AudioObjectGetPropertyDataSize(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
            );
            check_os_status(status)?;

            let n_ranges = data_size as usize / mem::size_of::<AudioValueRange>();
            let mut ranges: Vec<AudioValueRange> = Vec::with_capacity(n_ranges);
            let status = AudioObjectGetPropertyData(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
                NonNull::new(ranges.as_mut_ptr()).unwrap().cast(),
            );
            check_os_status(status)?;

            ranges.set_len(n_ranges);

            #[allow(non_upper_case_globals)]
            let input = match scope {
                kAudioObjectPropertyScopeInput => Ok(true),
                kAudioObjectPropertyScopeOutput => Ok(false),
                _ => Err(BackendSpecificError {
                    description: format!("unexpected scope (neither input nor output): {scope:?}"),
                }),
            }?;
            let audio_unit = audio_unit_from_device(self, input)?;
            let buffer_size = get_io_buffer_frame_size_range(&audio_unit)?;

            // Collect the supported formats for the device.

            let contains_different_sample_rates = ranges.iter().any(|r| r.mMinimum != r.mMaximum);
            if ranges.is_empty() {
                Ok(vec![].into_iter())
            } else if contains_different_sample_rates {
                let res = ranges.iter().map(|range| SupportedStreamConfigRange {
                    channels: n_channels as ChannelCount,
                    min_sample_rate: range.mMinimum as u32,
                    max_sample_rate: range.mMaximum as u32,
                    buffer_size,
                    sample_format,
                });
                Ok(res.collect::<Vec<_>>().into_iter())
            } else {
                let fmt = SupportedStreamConfigRange {
                    channels: n_channels as ChannelCount,
                    min_sample_rate: ranges
                        .iter()
                        .map(|v| v.mMinimum as u32)
                        .min()
                        .expect("the list must not be empty"),
                    max_sample_rate: ranges
                        .iter()
                        .map(|v| v.mMaximum as u32)
                        .max()
                        .expect("the list must not be empty"),
                    buffer_size,
                    sample_format,
                };

                Ok(vec![fmt].into_iter())
            }
        }
    }

    fn supported_input_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        self.supported_configs(kAudioObjectPropertyScopeInput)
    }

    fn supported_output_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        self.supported_configs(kAudioObjectPropertyScopeOutput)
    }

    fn default_config(
        &self,
        scope: AudioObjectPropertyScope,
    ) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        fn default_config_error_from_os_status(
            status: OSStatus,
        ) -> Result<(), DefaultStreamConfigError> {
            let err = match coreaudio::Error::from_os_status(status) {
                Err(err) => err,
                Ok(_) => return Ok(()),
            };
            match err {
                coreaudio::Error::AudioUnit(
                    coreaudio::error::AudioUnitError::FormatNotSupported,
                )
                | coreaudio::Error::AudioCodec(_)
                | coreaudio::Error::AudioFormat(_) => {
                    Err(DefaultStreamConfigError::StreamTypeNotSupported)
                }
                coreaudio::Error::AudioUnit(coreaudio::error::AudioUnitError::NoConnection) => {
                    Err(DefaultStreamConfigError::DeviceNotAvailable)
                }
                err => {
                    let description = format!("{err}");
                    let err = BackendSpecificError { description };
                    Err(err.into())
                }
            }
        }

        let property_address = AudioObjectPropertyAddress {
            mSelector: kAudioDevicePropertyStreamFormat,
            mScope: scope,
            mElement: kAudioObjectPropertyElementMaster,
        };

        unsafe {
            let mut asbd: AudioStreamBasicDescription = mem::zeroed();
            let mut data_size = mem::size_of::<AudioStreamBasicDescription>() as u32;
            let status = AudioObjectGetPropertyData(
                self.audio_device_id,
                NonNull::from(&property_address),
                0,
                null(),
                NonNull::from(&mut data_size),
                NonNull::from(&mut asbd).cast(),
            );
            default_config_error_from_os_status(status)?;

            let sample_format = {
                let audio_format = coreaudio::audio_unit::AudioFormat::from_format_and_flag(
                    asbd.mFormatID,
                    Some(asbd.mFormatFlags),
                );
                let flags = match audio_format {
                    Some(coreaudio::audio_unit::AudioFormat::LinearPCM(flags)) => flags,
                    _ => return Err(DefaultStreamConfigError::StreamTypeNotSupported),
                };
                let maybe_sample_format =
                    coreaudio::audio_unit::SampleFormat::from_flags_and_bits_per_sample(
                        flags,
                        asbd.mBitsPerChannel,
                    );
                match maybe_sample_format {
                    Some(coreaudio::audio_unit::SampleFormat::F32) => SampleFormat::F32,
                    Some(coreaudio::audio_unit::SampleFormat::I16) => SampleFormat::I16,
                    _ => return Err(DefaultStreamConfigError::StreamTypeNotSupported),
                }
            };

            #[allow(non_upper_case_globals)]
            let input = match scope {
                kAudioObjectPropertyScopeInput => Ok(true),
                kAudioObjectPropertyScopeOutput => Ok(false),
                _ => Err(BackendSpecificError {
                    description: format!("unexpected scope (neither input nor output): {scope:?}"),
                }),
            }?;
            let buffer_size = if input {
                let audio_unit = audio_unit_from_device(self, true)?;
                get_io_buffer_frame_size_range(&audio_unit)?
            } else {
                // The range is a device property; querying it directly avoids
                // constructing a temporary Audio Unit on each output config read.
                let range_address = AudioObjectPropertyAddress {
                    mSelector: kAudioDevicePropertyBufferFrameSizeRange,
                    mScope: kAudioObjectPropertyScopeGlobal,
                    mElement: kAudioObjectPropertyElementMain,
                };
                let mut range = AudioValueRange {
                    mMinimum: 0.0,
                    mMaximum: 0.0,
                };
                let mut range_size = mem::size_of::<AudioValueRange>() as u32;
                // SAFETY: Core Audio receives the exact size and a valid pointer to
                // one AudioValueRange. Check both status and returned size before use.
                let status = AudioObjectGetPropertyData(
                    self.audio_device_id,
                    NonNull::from(&range_address),
                    0,
                    null(),
                    NonNull::from(&mut range_size),
                    NonNull::from(&mut range).cast(),
                );
                default_config_error_from_os_status(status)?;
                if range_size as usize != mem::size_of::<AudioValueRange>() {
                    return Err(DefaultStreamConfigError::BackendSpecific {
                        err: BackendSpecificError {
                            description: "invalid device buffer range size".to_string(),
                        },
                    });
                }
                SupportedBufferSize::Range {
                    min: range.mMinimum as u32,
                    max: range.mMaximum as u32,
                }
            };

            let config = SupportedStreamConfig {
                sample_rate: asbd.mSampleRate as _,
                channels: asbd.mChannelsPerFrame as _,
                buffer_size,
                sample_format,
            };
            Ok(config)
        }
    }

    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        self.default_config(kAudioObjectPropertyScopeInput)
    }

    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        self.default_config(kAudioObjectPropertyScopeOutput)
    }

    /// Check if this device supports input (recording).
    fn supports_input(&self) -> bool {
        // Check if the device has input channels by trying to get its input configuration
        self.supported_input_configs()
            .map(|mut configs| configs.next().is_some())
            .unwrap_or(false)
    }
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Device")
            .field("audio_device_id", &self.audio_device_id)
            .field("name", &self.name())
            .finish()
    }
}

impl Device {
    #[allow(clippy::cast_ptr_alignment)]
    #[allow(clippy::while_immutable_condition)]
    #[allow(clippy::float_cmp)]
    fn build_input_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        mut data_callback: D,
        error_callback: E,
        _timeout: Option<Duration>,
    ) -> Result<Stream, BuildStreamError>
    where
        D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        // The scope and element for working with a device's input stream.
        let scope = Scope::Output;
        let element = Element::Input;

        // Potentially change the device sample rate to match the config.
        set_sample_rate(self.audio_device_id, config.sample_rate)?;

        let mut loopback_aggregate: Option<LoopbackDevice> = None;
        let mut audio_unit = if self.supports_input() {
            audio_unit_from_device(self, true)?
        } else {
            loopback_aggregate.replace(LoopbackDevice::from_device(self)?);
            audio_unit_from_device(&loopback_aggregate.as_ref().unwrap().aggregate_device, true)?
        };

        // Configure stream format and buffer size for predictable callback behavior.
        configure_stream_format_and_buffer(&mut audio_unit, config, sample_format, scope, element)?;

        let error_callback = Arc::new(Mutex::new(error_callback));
        let error_callback_disconnect = error_callback.clone();

        // Register the callback that is being called by coreaudio whenever it needs data to be
        // fed to the audio buffer.
        let (bytes_per_channel, sample_rate, device_buffer_frames) =
            setup_callback_vars(&audio_unit, config, sample_format);

        type Args = render_callback::Args<data::Raw>;
        audio_unit.set_input_callback(move |args: Args| unsafe {
            // SAFETY: We configure the stream format as interleaved (via asbd_from_config which
            // does not set kAudioFormatFlagIsNonInterleaved). Interleaved format always has
            // exactly one buffer containing all channels, so mBuffers[0] is always valid.
            let AudioBuffer {
                mNumberChannels: channels,
                mDataByteSize: data_byte_size,
                mData: data,
            } = (*args.data.data).mBuffers[0];

            let data = data as *mut ();
            let len = data_byte_size as usize / bytes_per_channel;
            let data = Data::from_parts(data, len, sample_format);

            let callback = match host_time_to_stream_instant(args.time_stamp.mHostTime) {
                Err(err) => {
                    invoke_error_callback(&error_callback, err.into());
                    return Err(());
                }
                Ok(cb) => cb,
            };
            let buffer_frames = len / channels as usize;
            // Use device buffer size for latency calculation if available
            let latency_frames = device_buffer_frames.unwrap_or(
                // Fallback to callback buffer size if device buffer size is unknown
                // (may overestimate latency for BufferSize::Default)
                buffer_frames,
            );
            let delay = frames_to_duration(latency_frames, sample_rate);
            let capture = callback
                .sub(delay)
                .expect("`capture` occurs before origin of alsa `StreamInstant`");
            let timestamp = crate::InputStreamTimestamp { callback, capture };

            let info = InputCallbackInfo { timestamp };
            data_callback(&data, &info);
            Ok(())
        })?;

        let error_callback_for_stream = input_disconnect_callback(error_callback_disconnect);

        let stream = Stream::new(
            StreamInner {
                playing: true,
                timing_watch: None,
                audio_unit,
                device_id: self.audio_device_id,
                _loopback_device: loopback_aggregate,
            },
            error_callback_for_stream,
        )?;

        stream
            .inner
            .lock()
            .map_err(|_| BuildStreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "A cpal stream operation panicked while holding the lock - this is a bug, please report it".to_string(),
                },
            })?
            .audio_unit
            .start()?;

        Ok(stream)
    }

    /// Construct a fixed-buffer stream with a verified client-frame ceiling.
    /// The hardware rate is observed separately from the client rate.
    pub fn build_bounded_output_stream<T, D, E>(
        &self,
        config: &StreamConfig,
        device_rate: SampleRate,
        client_limit: u16,
        mut data_callback: D,
        error_callback: E,
    ) -> Result<Stream, BuildStreamError>
    where
        T: crate::SizedSample,
        D: FnMut(&mut [T], &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        self.build_output_stream_with_limit(
            config,
            T::FORMAT,
            move |data, info| {
                if let Some(samples) = data.as_slice_mut::<T>() {
                    data_callback(samples, info);
                }
            },
            error_callback,
            Some((device_rate, client_limit)),
        )
    }

    fn build_output_stream_with_limit<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        mut data_callback: D,
        error_callback: E,
        bounds: Option<(SampleRate, u16)>,
    ) -> Result<Stream, BuildStreamError>
    where
        D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
        E: FnMut(StreamError) + Send + 'static,
    {
        let mut rollback = if bounds.is_some() {
            let BufferSize::Fixed(requested) = config.buffer_size else {
                return Err(bounded_output_error("a fixed hardware buffer is required"));
            };
            Some(OutputBufferRollback {
                device: self.audio_device_id,
                original: device_buffer_frames(self.audio_device_id)?,
                requested,
            })
        } else {
            None
        };
        // Pin the verified stream to the device just negotiated. The host
        // lifecycle owns reopening it after a default route change.
        let mut audio_unit = audio_unit_on_device(self, false, bounds.is_some())?;
        if bounds.is_some() {
            // CoreAudio derives the converted client limit when initialized.
            audio_unit.uninitialize()?;
        }

        // The scope and element for working with a device's output stream.
        let scope = Scope::Input;
        let element = Element::Output;

        // Configure device buffer (see comprehensive documentation in input stream above)
        configure_stream_format_and_buffer(&mut audio_unit, config, sample_format, scope, element)?;
        if let Some(((device_rate, _), buffer)) = bounds.zip(rollback.as_ref()) {
            let hardware_frames = buffer.requested;
            if self
                .default_output_config()
                .map_err(|error| bounded_output_error(&error.to_string()))?
                .sample_rate()
                != device_rate
                || device_buffer_frames(self.audio_device_id)? != hardware_frames
            {
                return Err(bounded_output_error(
                    "device rate or hardware buffer changed during output setup",
                ));
            }
            audio_unit.set_property(
                kAudioUnitProperty_MaximumFramesPerSlice,
                Scope::Global,
                Element::Output,
                Some(&hardware_frames),
            )?;
        }

        let error_callback = Arc::new(Mutex::new(error_callback));
        let error_callback_disconnect = error_callback.clone();

        // Register the callback that is being called by coreaudio whenever it needs data to be
        // fed to the audio buffer.
        let (bytes_per_channel, sample_rate, device_buffer_frames) =
            setup_callback_vars(&audio_unit, config, sample_format);

        type Args = render_callback::Args<data::Raw>;
        audio_unit.set_render_callback(move |args: Args| unsafe {
            // SAFETY: We configure the stream format as interleaved (via asbd_from_config which
            // does not set kAudioFormatFlagIsNonInterleaved). Interleaved format always has
            // exactly one buffer containing all channels, so mBuffers[0] is always valid.
            let AudioBuffer {
                mNumberChannels: channels,
                mDataByteSize: data_byte_size,
                mData: data,
            } = (*args.data.data).mBuffers[0];

            let data = data as *mut ();
            let len = data_byte_size as usize / bytes_per_channel;
            let mut data = Data::from_parts(data, len, sample_format);

            let callback = match host_time_to_stream_instant(args.time_stamp.mHostTime) {
                Err(err) => {
                    invoke_error_callback(&error_callback, err.into());
                    return Err(());
                }
                Ok(cb) => cb,
            };
            let buffer_frames = len / channels as usize;
            // Use device buffer size for latency calculation if available
            let latency_frames = device_buffer_frames.unwrap_or(
                // Fallback to callback buffer size if device buffer size is unknown
                // (may overestimate latency for BufferSize::Default)
                buffer_frames,
            );
            let latency_rate = bounds
                .filter(|_| device_buffer_frames.is_some())
                .map_or(sample_rate, |(device_rate, _)| device_rate);
            let delay = frames_to_duration(latency_frames, latency_rate);
            let playback = callback
                .add(delay)
                .expect("`playback` occurs beyond representation supported by `StreamInstant`");
            let timestamp = crate::OutputStreamTimestamp { callback, playback };

            let info = OutputCallbackInfo { timestamp };
            data_callback(&mut data, &info);
            Ok(())
        })?;

        if let Some((_, client_limit)) = bounds {
            audio_unit.initialize()?;
            let maximum: u32 = audio_unit.get_property(
                kAudioUnitProperty_MaximumFramesPerSlice,
                Scope::Global,
                Element::Output,
            )?;
            if maximum == 0 || maximum > u32::from(client_limit) {
                return Err(bounded_output_error(
                    "coreaudio client callback limit exceeds the plugin bridge quantum; reduce the hardware buffer",
                ));
            }
        }

        // Create error callback for stream - either dummy or real based on device type
        let error_callback_for_stream: super::ErrorCallback = if is_default_output_device(self) {
            Box::new(|_: StreamError| {})
        } else {
            let error_callback_clone = error_callback_disconnect.clone();
            Box::new(move |err: StreamError| {
                invoke_error_callback(&error_callback_clone, err);
            })
        };

        let stream = Stream::new(
            StreamInner {
                playing: true,
                timing_watch: None,
                audio_unit,
                device_id: self.audio_device_id,
                _loopback_device: None,
            },
            error_callback_for_stream,
        )?;

        stream
            .inner
            .lock()
            .map_err(|_| BuildStreamError::BackendSpecific {
                err: BackendSpecificError {
                    description: "A cpal stream operation panicked while holding the lock - this is a bug, please report it".to_string(),
                },
            })?
            .audio_unit
            .start()?;

        if let Some(rollback) = &mut rollback {
            rollback.original = rollback.requested;
        }
        Ok(stream)
    }
}

fn bounded_output_error(description: &str) -> BuildStreamError {
    BackendSpecificError {
        description: description.into(),
    }
    .into()
}

const DEVICE_BUFFER_ADDRESS: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
    mSelector: kAudioDevicePropertyBufferFrameSize,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain,
};

/// Roll back only a failed construction whose hardware setting still matches
/// our request. An accepted stream uses the existing fixed-buffer lifecycle.
struct OutputBufferRollback {
    device: AudioDeviceID,
    original: u32,
    requested: u32,
}

impl Drop for OutputBufferRollback {
    fn drop(&mut self) {
        if self.original != self.requested
            && device_buffer_frames(self.device).is_ok_and(|size| size == self.requested)
        {
            // SAFETY: The supplied value is one initialized u32, read
            // synchronously by the device buffer frame-size property.
            unsafe {
                AudioObjectSetPropertyData(
                    self.device,
                    NonNull::from(&DEVICE_BUFFER_ADDRESS),
                    0,
                    null(),
                    size_of::<u32>() as u32,
                    NonNull::from(&self.original).cast(),
                );
            }
        }
    }
}

/// One copied observation of the actual output unit and its pinned device.
/// Device components are in hardware frames; the unit reports seconds. This
/// grants no ongoing route validity: rate, buffer, stream, safety or latency
/// changes invalidate it. Callers must observe those changes before using it
/// for a persistent presentation plan. It is not measured physical latency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputTiming {
    pub device_id: AudioDeviceID,
    pub device_sample_rate: f64,
    pub client_sample_rate: f64,
    pub buffer_frames: u32,
    pub safety_frames: u32,
    pub device_frames: u32,
    /// All output streams must report this same value; differing streams refuse.
    pub stream_frames: u32,
    pub audio_unit_seconds: f64,
}

impl OutputTiming {
    /// Declared buffer/safety/device/stream path rounded up once to client
    /// frames using exact integral-rate arithmetic. Fractional nominal rates
    /// refuse. EXCLUDES the AudioUnit property: its device overlap is not
    /// established by this query. Do not add it or infer full presentation
    /// timing without a supported composition and route-validity contract.
    pub fn hardware_path_client_frames(&self) -> Result<u32, BuildStreamError> {
        if self.device_id == 0
            || !self.device_sample_rate.is_finite()
            || self.device_sample_rate < 1.0
            || self.device_sample_rate > f64::from(u32::MAX)
            || self.device_sample_rate.fract() != 0.0
            || !self.client_sample_rate.is_finite()
            || self.client_sample_rate < 1.0
            || self.client_sample_rate > f64::from(u32::MAX)
            || self.client_sample_rate.fract() != 0.0
            || !self.audio_unit_seconds.is_finite()
            || self.audio_unit_seconds < 0.0
            || self.audio_unit_seconds * self.client_sample_rate > f64::from(u32::MAX)
            || self.buffer_frames == 0
        {
            return Err(bounded_output_error("invalid coreaudio output timing"));
        }
        let hardware = self
            .buffer_frames
            .checked_add(self.safety_frames)
            .and_then(|n| n.checked_add(self.device_frames))
            .and_then(|n| n.checked_add(self.stream_frames))
            .ok_or_else(|| bounded_output_error("coreaudio output timing overflow"))?;
        // CPAL rates are integral. Exact integer arithmetic avoids adding a
        // spurious frame when a floating-point quotient lands above an integer.
        let numerator = u64::from(hardware)
            .checked_mul(u64::from(self.client_sample_rate as u32))
            .ok_or_else(|| bounded_output_error("coreaudio output timing overflow"))?;
        u32::try_from(numerator.div_ceil(u64::from(self.device_sample_rate as u32)))
            .map_err(|_| bounded_output_error("coreaudio output timing overflow"))
    }
}

impl Stream {
    /// Read declared output timing on CONTROL only. This locks and calls native
    /// property APIs; never call from a data/error callback. Two equal bounded
    /// snapshots detect observed setup changes, not atomicity or future changes.
    /// Enumeration is capped at 16 output streams; no OS call deadline is claimed.
    /// Missing, malformed or nonuniform properties return errors, never zero.
    pub fn output_timing_on_control(&self) -> Result<OutputTiming, BuildStreamError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| bounded_output_error("coreaudio output control is poisoned"))?;
        stable_output_timing(&inner.audio_unit, inner.device_id)
    }
}

/// Identity of a watched observation. Only the original Stream can revalidate
/// it. The receipt holds no Stream, listener or audio-thread custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputTimingReceipt(usize);

// The OS treats user data as an opaque cookie. A callback never dereferences
// it, even after failed removal or late delivery. A slot is reused only after
// successful unregister, with a fresh serial. Quarantine on removal failure
// bounds the number of surviving native registrations for this process.
const WATCH_SLOTS: usize = 16;
const WATCH_STRIDE: usize = WATCH_SLOTS * 2;
const WATCH_DIRTY: usize = 1;
static WATCH_SERIAL: AtomicUsize = AtomicUsize::new(1);
// Array-repeat const operands support the pinned crate's Rust 1.78 MSRV.
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY_WATCH_STATE: AtomicUsize = AtomicUsize::new(0);
static WATCH_STATE: [AtomicUsize; WATCH_SLOTS] = [EMPTY_WATCH_STATE; WATCH_SLOTS];

impl OutputTimingReceipt {
    fn reserve() -> Result<Self, BuildStreamError> {
        let serial = WATCH_SERIAL
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < usize::MAX / WATCH_STRIDE).then(|| n + 1)
            })
            .map_err(|_| bounded_output_error("coreaudio timing receipt identity exhausted"))?;
        for (index, slot) in WATCH_STATE.iter().enumerate() {
            let receipt = Self(serial * WATCH_STRIDE + index * 2);
            if slot
                .compare_exchange(0, receipt.0, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(receipt);
            }
        }
        Err(bounded_output_error(
            "coreaudio timing watch capacity exhausted",
        ))
    }

    fn slot(self) -> &'static AtomicUsize {
        &WATCH_STATE[(self.0 / 2) % WATCH_SLOTS]
    }

    fn current(self) -> bool {
        self.slot().load(Ordering::Acquire) == self.0
    }

    fn invalidate(self) {
        invalidate_watch_slot(self.slot(), self.0);
    }

    fn release(self) {
        let _ = self.slot().compare_exchange(
            self.0 | WATCH_DIRTY,
            0,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }

    fn user_data(self) -> *mut c_void {
        self.0 as *mut c_void
    }
}

fn invalidate_watch_slot(slot: &AtomicUsize, identity: usize) {
    let _ = slot.compare_exchange(
        identity,
        identity | WATCH_DIRTY,
        Ordering::AcqRel,
        Ordering::Relaxed,
    );
}

unsafe extern "C-unwind" fn invalidate_hal_timing(
    _: AudioObjectID,
    _: u32,
    _: NonNull<AudioObjectPropertyAddress>,
    data: *mut c_void,
) -> i32 {
    if !data.is_null() {
        OutputTimingReceipt(data as usize).invalidate();
    }
    0
}

unsafe extern "C-unwind" fn invalidate_unit_timing(
    data: NonNull<c_void>,
    _: objc2_audio_toolbox::AudioUnit,
    _: u32,
    _: u32,
    _: u32,
) {
    OutputTimingReceipt(data.as_ptr() as usize).invalidate();
}

const UNIT_TIMING_PROPERTIES: [u32; 4] = [
    kAudioOutputUnitProperty_CurrentDevice,
    kAudioOutputUnitProperty_EnableIO,
    kAudioUnitProperty_StreamFormat,
    kAudioUnitProperty_Latency,
];

fn timing_address(selector: u32, scope: AudioObjectPropertyScope) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

pub(super) struct OutputTimingWatch {
    receipt: OutputTimingReceipt,
    snapshot: OutputTimingSnapshot,
    hal: Vec<(AudioObjectID, AudioObjectPropertyAddress)>,
    unit_count: usize,
}

impl OutputTimingWatch {
    fn install(unit: &AudioUnit, device: AudioDeviceID) -> Result<Self, BuildStreamError> {
        let snapshot = output_timing_snapshot(unit, device)?;
        let mut hal = Vec::new();
        // Seven fixed addresses and at most sixteen stream latency properties.
        hal.try_reserve_exact(7 + snapshot.2)
            .map_err(|_| bounded_output_error("coreaudio timing watch storage unavailable"))?;
        let mut watch = Self {
            receipt: OutputTimingReceipt::reserve()?,
            snapshot,
            hal,
            unit_count: 0,
        };
        if let Err(error) = watch
            .register(unit, device)
            .and_then(|()| watch.validate(unit, device))
        {
            watch.retire(unit);
            return Err(error);
        }
        Ok(watch)
    }

    fn register(
        &mut self,
        unit: &AudioUnit,
        device: AudioDeviceID,
    ) -> Result<(), BuildStreamError> {
        let global = kAudioObjectPropertyScopeGlobal;
        let output = kAudioObjectPropertyScopeOutput;
        for (object, address) in [
            (
                kAudioObjectSystemObject as AudioObjectID,
                timing_address(kAudioHardwarePropertyDefaultOutputDevice, global),
            ),
            (
                device,
                timing_address(kAudioDevicePropertyDeviceIsAlive, global),
            ),
            (
                device,
                timing_address(kAudioDevicePropertyNominalSampleRate, global),
            ),
            (device, DEVICE_BUFFER_ADDRESS),
            (
                device,
                timing_address(kAudioDevicePropertySafetyOffset, output),
            ),
            (device, timing_address(kAudioDevicePropertyLatency, output)),
            (device, timing_address(kAudioDevicePropertyStreams, output)),
        ] {
            self.register_hal(object, address)?;
        }
        for index in 0..self.snapshot.2 {
            let stream = self.snapshot.1[index];
            self.register_hal(stream, timing_address(kAudioStreamPropertyLatency, global))?;
        }
        for selector in UNIT_TIMING_PROPERTIES {
            // SAFETY: This is the live Stream's unit and a scalar property
            // selector. The callback receives an opaque integer, never a borrow.
            let status = unsafe {
                AudioUnitAddPropertyListener(
                    *unit.as_ref(),
                    selector,
                    Some(invalidate_unit_timing),
                    self.receipt.user_data(),
                )
            };
            coreaudio::Error::from_os_status(status)?;
            self.unit_count += 1;
        }
        Ok(())
    }

    fn register_hal(
        &mut self,
        object: AudioObjectID,
        address: AudioObjectPropertyAddress,
    ) -> Result<(), BuildStreamError> {
        // SAFETY: Core Audio reads this address synchronously. Callback data is
        // an opaque cookie with no backing object lifetime.
        let status = unsafe {
            AudioObjectAddPropertyListener(
                object,
                NonNull::from(&address),
                Some(invalidate_hal_timing),
                self.receipt.user_data(),
            )
        };
        coreaudio::Error::from_os_status(status)?;
        self.hal.push((object, address));
        Ok(())
    }

    fn validate(&self, unit: &AudioUnit, device: AudioDeviceID) -> Result<(), BuildStreamError> {
        let result = (|| {
            if !self.receipt.current() {
                return Err(bounded_output_error("coreaudio output timing is stale"));
            }
            let now = output_timing_snapshot(unit, device)?;
            validate_stable_output_timing(self.snapshot, now)?;
            if !self.receipt.current() {
                return Err(bounded_output_error(
                    "coreaudio output timing changed during query",
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            self.receipt.invalidate();
        }
        result
    }

    pub(super) fn retire(self, unit: &AudioUnit) {
        self.receipt.invalidate();
        let mut removed = true;
        for &selector in &UNIT_TIMING_PROPERTIES[..self.unit_count] {
            // SAFETY: This is the original unit and matching registered tuple,
            // with the same opaque cookie. Even a late callback is harmless.
            let status = unsafe {
                AudioUnitRemovePropertyListenerWithUserData(
                    *unit.as_ref(),
                    selector,
                    Some(invalidate_unit_timing),
                    self.receipt.user_data(),
                )
            };
            removed &= status == 0;
        }
        for (object, address) in self.hal {
            // SAFETY: The address and opaque cookie match the registration.
            let status = unsafe {
                AudioObjectRemovePropertyListener(
                    object,
                    NonNull::from(&address),
                    Some(invalidate_hal_timing),
                    self.receipt.user_data(),
                )
            };
            removed &= status == 0;
        }
        if removed {
            self.receipt.release();
        }
    }
}

impl Stream {
    /// Start a fresh observation on control and retire any previous watch.
    /// A delivered notification permanently revokes a receipt. Registration
    /// or query failure leaves no current receipt. One watch per stream and
    /// sixteen process slots bound retained native notification registrations.
    pub fn watch_output_timing_on_control(&self) -> Result<OutputTimingReceipt, BuildStreamError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| bounded_output_error("coreaudio output control is poisoned"))?;
        inner.clear_timing_watch();
        let watch = OutputTimingWatch::install(&inner.audio_unit, inner.device_id)?;
        let receipt = watch.receipt;
        inner.timing_watch = Some(watch);
        Ok(receipt)
    }

    /// Revalidate the original property values and notification state on
    /// control. Notifications can arrive after a property change, so this is a
    /// point-in-time check, with no guarantee beyond its final observation.
    pub fn revalidate_output_timing_on_control(
        &self,
        receipt: OutputTimingReceipt,
    ) -> Result<OutputTiming, BuildStreamError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| bounded_output_error("coreaudio output control is poisoned"))?;
        let watch = inner
            .timing_watch
            .as_ref()
            .filter(|watch| watch.receipt == receipt)
            .ok_or_else(|| {
                bounded_output_error(
                    "coreaudio output timing receipt is not current for this stream",
                )
            })?;
        watch.validate(&inner.audio_unit, inner.device_id)?;
        Ok(watch.snapshot.0)
    }
}

fn stable_output_timing(
    unit: &AudioUnit,
    device: AudioDeviceID,
) -> Result<OutputTiming, BuildStreamError> {
    let first = output_timing_snapshot(unit, device)?;
    let second = output_timing_snapshot(unit, device)?;
    validate_stable_output_timing(first, second)
}

type OutputTimingSnapshot = (OutputTiming, [u32; 16], usize);

fn validate_stable_output_timing(
    first: OutputTimingSnapshot,
    second: OutputTimingSnapshot,
) -> Result<OutputTiming, BuildStreamError> {
    if first != second {
        return Err(bounded_output_error(
            "coreaudio output timing changed during query",
        ));
    }
    first.0.hardware_path_client_frames()?;
    Ok(first.0)
}

fn output_timing_snapshot(
    unit: &AudioUnit,
    device: AudioDeviceID,
) -> Result<(OutputTiming, [u32; 16], usize), BuildStreamError> {
    let actual: AudioDeviceID = output_unit_timing_property(
        unit,
        kAudioOutputUnitProperty_CurrentDevice,
        Scope::Global,
        Element::Output,
    )?;
    let enabled: u32 = output_unit_timing_property(
        unit,
        kAudioOutputUnitProperty_EnableIO,
        Scope::Output,
        Element::Output,
    )?;
    if actual != device
        || device == 0
        || enabled != 1
        || output_timing_u32(
            device,
            kAudioDevicePropertyDeviceIsAlive,
            kAudioObjectPropertyScopeGlobal,
        )? != 1
    {
        return Err(bounded_output_error(
            "coreaudio output device is unavailable",
        ));
    }
    let format: AudioStreamBasicDescription = output_unit_timing_property(
        unit,
        kAudioUnitProperty_StreamFormat,
        Scope::Input,
        Element::Output,
    )?;
    if format.mChannelsPerFrame == 0 {
        return Err(bounded_output_error(
            "coreaudio output channels are unavailable",
        ));
    }
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut rate = 0.0_f64;
    let mut size = size_of::<f64>() as u32;
    // SAFETY: These initialized writable outputs have the exact property type
    // and lifetime for this synchronous native read; its returned size is checked.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut rate).cast(),
        )
    };
    coreaudio::Error::from_os_status(status)?;
    if size != size_of::<f64>() as u32 {
        return Err(bounded_output_error("invalid coreaudio device rate size"));
    }
    let (streams, count) = output_timing_streams(device)?;
    let stream_frames = uniform_output_timing(&streams[..count], |stream| {
        output_timing_u32(
            stream,
            kAudioStreamPropertyLatency,
            kAudioObjectPropertyScopeGlobal,
        )
    })?;
    let timing = OutputTiming {
        device_id: device,
        device_sample_rate: rate,
        client_sample_rate: format.mSampleRate,
        buffer_frames: device_buffer_frames(device)?,
        safety_frames: output_timing_u32(
            device,
            kAudioDevicePropertySafetyOffset,
            kAudioObjectPropertyScopeOutput,
        )?,
        device_frames: output_timing_u32(
            device,
            kAudioDevicePropertyLatency,
            kAudioObjectPropertyScopeOutput,
        )?,
        stream_frames,
        audio_unit_seconds: output_unit_timing_property(
            unit,
            kAudioUnitProperty_Latency,
            Scope::Global,
            Element::Output,
        )?,
    };
    timing.hardware_path_client_frames()?;
    Ok((timing, streams, count))
}

// This private, closed list contains only initialized native POD values with
// valid arbitrary bit patterns. Do not implement for pointers or Rust owners.
trait OutputTimingProperty: Copy {
    fn initialized() -> Self;
}
impl OutputTimingProperty for u32 {
    fn initialized() -> Self {
        0
    }
}
impl OutputTimingProperty for f64 {
    fn initialized() -> Self {
        0.0
    }
}
impl OutputTimingProperty for AudioStreamBasicDescription {
    fn initialized() -> Self {
        Self {
            mSampleRate: 0.0,
            mFormatID: 0,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: 0,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 0,
            mBitsPerChannel: 0,
            mReserved: 0,
        }
    }
}

fn output_unit_timing_property<T: OutputTimingProperty>(
    unit: &AudioUnit,
    selector: u32,
    scope: Scope,
    element: Element,
) -> Result<T, BuildStreamError> {
    let mut value = T::initialized();
    let mut size = size_of::<T>() as u32;
    // SAFETY: The unit is borrowed and live, and the private POD list supplies
    // initialized writable storage of the advertised size/alignment. All calls
    // below use the documented property type. Returned sizes are checked rather
    // than assuming an uninitialized native result is complete.
    let status = unsafe {
        AudioUnitGetProperty(
            *unit.as_ref(),
            selector,
            scope as u32,
            element as u32,
            NonNull::from(&mut value).cast(),
            NonNull::from(&mut size),
        )
    };
    coreaudio::Error::from_os_status(status)?;
    if size != size_of::<T>() as u32 {
        return Err(bounded_output_error(
            "invalid coreaudio output unit timing property size",
        ));
    }
    Ok(value)
}

fn output_timing_u32(
    object: u32,
    selector: u32,
    scope: AudioObjectPropertyScope,
) -> Result<u32, BuildStreamError> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: The selected UInt32 property writes into a live initialized UInt32
    // with its exact capacity. The synchronous read's returned size is checked.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    coreaudio::Error::from_os_status(status)?;
    if size != size_of::<u32>() as u32 {
        return Err(bounded_output_error(
            "invalid coreaudio timing property size",
        ));
    }
    Ok(value)
}

fn uniform_output_timing(
    streams: &[u32],
    mut read: impl FnMut(u32) -> Result<u32, BuildStreamError>,
) -> Result<u32, BuildStreamError> {
    let Some((first, rest)) = streams.split_first().filter(|_| streams.len() <= 16) else {
        return Err(bounded_output_error(
            "invalid coreaudio output stream count",
        ));
    };
    let frames = read(*first)?;
    for stream in rest {
        if read(*stream)? != frames {
            return Err(bounded_output_error(
                "nonuniform coreaudio output stream latency",
            ));
        }
    }
    Ok(frames)
}

fn output_timing_streams(device: AudioDeviceID) -> Result<([u32; 16], usize), BuildStreamError> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreams,
        mScope: kAudioObjectPropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut streams = [0_u32; 16];
    let mut size = size_of_val(&streams) as u32;
    // SAFETY: The initialized array provides the advertised writable capacity.
    // Core Audio refuses insufficient space; count/alignment/IDs are checked
    // before indexing. No returned size can extend the slice past this array.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new(streams.as_mut_ptr())
                .ok_or_else(|| bounded_output_error("coreaudio stream storage unavailable"))?
                .cast(),
        )
    };
    coreaudio::Error::from_os_status(status)?;
    let count = validate_output_timing_streams(&streams, size as usize)?;
    Ok((streams, count))
}

fn validate_output_timing_streams(
    streams: &[u32; 16],
    size: usize,
) -> Result<usize, BuildStreamError> {
    let count = size / size_of::<u32>();
    if size == 0 || size > size_of_val(streams) || size % size_of::<u32>() != 0 {
        return Err(bounded_output_error(
            "invalid coreaudio output stream inventory",
        ));
    }
    for i in 0..count {
        if streams[i] == 0 || streams[..i].contains(&streams[i]) {
            return Err(bounded_output_error(
                "invalid coreaudio output stream identity",
            ));
        }
    }
    Ok(count)
}

#[cfg(test)]
mod output_timing_regression {
    use super::*;

    fn timing() -> OutputTiming {
        OutputTiming {
            device_id: 1,
            device_sample_rate: 48_000.0,
            client_sample_rate: 44_100.0,
            buffer_frames: 128,
            safety_frames: 16,
            device_frames: 7,
            stream_frames: 9,
            audio_unit_seconds: 0.125,
        }
    }

    #[test]
    fn changing_nonuniform_and_unavailable_observations_refuse() {
        assert_eq!(uniform_output_timing(&[5, 6], |_| Ok(0)).unwrap(), 0);
        assert!(uniform_output_timing(&[], |_| Ok(0)).is_err());
        assert!(uniform_output_timing(&[1; 17], |_| Ok(0)).is_err());
        assert!(uniform_output_timing(&[5, 6], Ok).is_err());
        assert!(
            uniform_output_timing(&[5], |_| Err(bounded_output_error("property unavailable")))
                .is_err()
        );
        let first = (timing(), [5; 16], 1);
        assert_eq!(
            validate_stable_output_timing(first, first).unwrap(),
            timing()
        );
        assert!(validate_stable_output_timing(
            first,
            (
                OutputTiming {
                    safety_frames: 17,
                    ..timing()
                },
                first.1,
                1
            )
        )
        .is_err());
        assert!(validate_stable_output_timing(first, (timing(), [6; 16], 1)).is_err());
        assert!(validate_stable_output_timing(first, (timing(), first.1, 2)).is_err());
    }

    #[test]
    fn units_round_once_and_do_not_double_count_unclassified_audio_unit_latency() {
        let observed = timing();
        assert_eq!(observed.hardware_path_client_frames().unwrap(), 147);
        assert_eq!(
            OutputTiming {
                client_sample_rate: 48_000.0,
                ..observed
            }
            .hardware_path_client_frames()
            .unwrap(),
            160
        );
        // The AU value remains a separate reported observation. It may already
        // include hardware latency, so it must not silently enter the sum.
        assert_eq!(observed.audio_unit_seconds, 0.125);
        assert_eq!(
            OutputTiming {
                audio_unit_seconds: 0.0,
                ..observed
            }
            .hardware_path_client_frames()
            .unwrap(),
            147
        );
    }

    #[test]
    fn malformed_and_overflowing_timing_never_becomes_known_zero() {
        for value in [
            44_100.5,
            f64::NAN,
            f64::INFINITY,
            0.0,
            -1.0,
            f64::MIN_POSITIVE,
            f64::MAX,
        ] {
            assert!(OutputTiming {
                device_sample_rate: value,
                ..timing()
            }
            .hardware_path_client_frames()
            .is_err());
            assert!(OutputTiming {
                client_sample_rate: value,
                ..timing()
            }
            .hardware_path_client_frames()
            .is_err());
        }
        for value in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] {
            assert!(OutputTiming {
                audio_unit_seconds: value,
                ..timing()
            }
            .hardware_path_client_frames()
            .is_err());
        }
        assert!(OutputTiming {
            device_id: 0,
            ..timing()
        }
        .hardware_path_client_frames()
        .is_err());
        assert!(OutputTiming {
            buffer_frames: 0,
            ..timing()
        }
        .hardware_path_client_frames()
        .is_err());
        assert!(OutputTiming {
            device_frames: u32::MAX,
            ..timing()
        }
        .hardware_path_client_frames()
        .is_err());
        assert!(OutputTiming {
            device_sample_rate: 1.0,
            client_sample_rate: f64::from(u32::MAX),
            ..timing()
        }
        .hardware_path_client_frames()
        .is_err());
        let mut streams = [0; 16];
        for size in [0, 1, 3, 4, 65, usize::MAX] {
            assert!(validate_output_timing_streams(&streams, size).is_err());
        }
        streams[0] = 5;
        assert_eq!(validate_output_timing_streams(&streams, 4).unwrap(), 1);
        streams[1] = 5;
        assert!(validate_output_timing_streams(&streams, 8).is_err());
        streams[1] = 6;
        assert_eq!(validate_output_timing_streams(&streams, 8).unwrap(), 2);
        assert!(output_timing_u32(
            0,
            kAudioDevicePropertyLatency,
            kAudioObjectPropertyScopeOutput
        )
        .is_err());
    }

    #[test]
    fn a_late_signal_cannot_invalidate_a_reused_slot() {
        // Local slot exercises the same matching rule without depending on
        // parallel tests or actual native listener scheduling.
        let slot = AtomicUsize::new(WATCH_STRIDE);
        invalidate_watch_slot(&slot, WATCH_STRIDE);
        assert_eq!(slot.load(Ordering::Acquire), WATCH_STRIDE | WATCH_DIRTY);
        slot.store(2 * WATCH_STRIDE, Ordering::Release);
        invalidate_watch_slot(&slot, WATCH_STRIDE);
        assert_eq!(slot.load(Ordering::Acquire), 2 * WATCH_STRIDE);
    }

    #[test]
    fn actual_pinned_output_unit_reports_component_observations_on_control() {
        let device = default_output_device().expect("native timing probe needs an output device");
        // No stream start, hardware buffer/rate mutation or audio callback.
        let unit = audio_unit_on_device(&device, false, true).unwrap();
        let stream = Stream::new(
            StreamInner {
                playing: false,
                timing_watch: None,
                audio_unit: unit,
                device_id: device.audio_device_id,
                _loopback_device: None,
            },
            Box::new(|_| {}),
        )
        .unwrap();
        let observed: crate::platform::CoreAudioOutputTiming =
            stream.output_timing_on_control().unwrap();
        assert_eq!(observed.device_id, device.audio_device_id);
        assert_eq!(
            observed.buffer_frames,
            device_buffer_frames(device.audio_device_id).unwrap()
        );
        assert!(observed.hardware_path_client_frames().unwrap() > 0);
        println!("control-only declared output components: {observed:?}");
        let received: crate::platform::CoreAudioOutputTimingReceipt =
            stream.watch_output_timing_on_control().unwrap();
        assert_eq!(
            stream
                .revalidate_output_timing_on_control(received)
                .unwrap(),
            observed
        );
        let foreign = OutputTimingReceipt::reserve().unwrap();
        assert!(stream.revalidate_output_timing_on_control(foreign).is_err());
        foreign.invalidate();
        foreign.release();

        // Inject a delivered HAL signal through the actual registered shim.
        // No hardware device property is modified.
        unsafe {
            invalidate_hal_timing(
                device.audio_device_id,
                1,
                NonNull::from(&DEVICE_BUFFER_ADDRESS),
                received.user_data(),
            );
        }
        assert!(stream
            .revalidate_output_timing_on_control(received)
            .is_err());
        let fresh = stream.watch_output_timing_on_control().unwrap();
        assert_ne!(received, fresh);
        assert!(stream
            .revalidate_output_timing_on_control(received)
            .is_err());
        received.invalidate(); // Simulate late delivery after unregister.
        assert_eq!(
            stream.revalidate_output_timing_on_control(fresh).unwrap(),
            observed
        );
        drop(stream);
        assert!(!fresh.current());
    }
}

fn device_buffer_frames(device: AudioDeviceID) -> Result<u32, BuildStreamError> {
    let mut frames = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: Both output pointers are writable for the synchronous property read.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&DEVICE_BUFFER_ADDRESS),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut frames).cast(),
        )
    };
    coreaudio::Error::from_os_status(status)?;
    if size != size_of::<u32>() as u32 || frames == 0 {
        return Err(bounded_output_error("invalid coreaudio device buffer size"));
    }
    Ok(frames)
}

/// Configure stream format and buffer size for CoreAudio stream.
///
/// This handles the common setup tasks for both input and output streams:
/// - Sets the stream format (ASBD)
/// - Configures buffer size for Fixed buffer size requests
fn configure_stream_format_and_buffer(
    audio_unit: &mut AudioUnit,
    config: &StreamConfig,
    sample_format: SampleFormat,
    scope: Scope,
    element: Element,
) -> Result<(), BuildStreamError> {
    // Set the stream format using stream-specific scope/element
    // - Input streams: scope=Output, element=Input (configuring output format of input element)
    // - Output streams: scope=Input, element=Output (configuring input format of output element)
    let asbd = asbd_from_config(config, sample_format);
    audio_unit.set_property(kAudioUnitProperty_StreamFormat, scope, element, Some(&asbd))?;

    // Configure device buffer size if requested
    if let BufferSize::Fixed(buffer_size) = config.buffer_size {
        // IMPORTANT: Buffer frame size is a DEVICE-LEVEL property, not stream-specific.
        // Unlike stream format above, we ALWAYS use Scope::Global + Element::Output
        // for device properties, regardless of whether this is an input or output stream.
        // This is consistent with other device properties like:
        // - kAudioOutputUnitProperty_CurrentDevice
        // - kAudioDevicePropertyBufferFrameSizeRange
        // The Element::Output here doesn't mean "output stream only" - it's the
        // canonical element used for device-wide properties in Core Audio.
        audio_unit.set_property(
            kAudioDevicePropertyBufferFrameSize,
            Scope::Global,
            Element::Output,
            Some(&buffer_size),
        )?;
    }

    Ok(())
}

/// Setup common callback variables and query device buffer size.
///
/// Returns (bytes_per_channel, sample_rate, device_buffer_frames)
fn setup_callback_vars(
    audio_unit: &AudioUnit,
    config: &StreamConfig,
    sample_format: SampleFormat,
) -> (usize, crate::SampleRate, Option<usize>) {
    let bytes_per_channel = sample_format.sample_size();
    let sample_rate = config.sample_rate;

    // Query device buffer size for latency calculation
    let device_buffer_frames = get_device_buffer_frame_size(audio_unit).ok();

    (bytes_per_channel, sample_rate, device_buffer_frames)
}

/// Query the current device buffer frame size from CoreAudio.
///
/// Buffer frame size is a device-level property that always uses Scope::Global + Element::Output,
/// regardless of whether the audio unit is configured for input or output streams.
fn get_device_buffer_frame_size(audio_unit: &AudioUnit) -> Result<usize, coreaudio::Error> {
    // Device-level property: always use Scope::Global + Element::Output
    // This is consistent with how we set the buffer size and query the buffer size range
    let frames: u32 = audio_unit.get_property(
        kAudioDevicePropertyBufferFrameSize,
        Scope::Global,
        Element::Output,
    )?;
    Ok(frames as usize)
}
