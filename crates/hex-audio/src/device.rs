//! Native adapter for canonical compiled playback and host sample conversion.
//!
//! A complete bounded stereo chunk is validated through its final channel/sample conversion before
//! host copying, gain-delivery confirmation or spectrum capture. Failure silences/stops the failed
//! chunk and latches its cause; already committed prefix output remains intact. Float output
//! preserves finite range, integer output uses the native conversion law, and extra channels
//! receive silence. Stream setup, reopen and final teardown stay on the lifecycle/control lane,
//! not the callback.
use crate::open_outcome::{BACKEND_CONVERTS_OUTPUT_RATE, OutputOpenError, negotiate_stream_rate};
use crate::spectrum_worker::{
    SpectrumReader, SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker,
};
use crate::{lifecycle, prepared};
use cpal::{
    FromSample, Sample, SizedSample,
    traits::{DeviceTrait, HostTrait},
};
use hex_engine::Frame;
use std::{
    sync::{
        Arc,
        atomic::Ordering::{Acquire, Relaxed},
    },
    time::Instant,
};

pub struct PreparedOutput {
    _stream: lifecycle::StreamOwner,
    pub control: prepared::Control,
    pub sample_rate: u32,
    pub label: String,
    pub spectrum: Option<SpectrumReader>,
    pub spectrum_error: Option<String>,
}
impl PreparedOutput {
    /// Opens the current default device with an empty, paused arrangement.
    /// Prepare and publish rate-matching graphs before sending Play.
    pub fn open() -> Result<Self, String> {
        let lease = lifecycle::Lease::acquire()?;
        Self::open_with_lease(None, lease)
    }

    pub(crate) fn open_with_lease(
        expected_sample_rate: Option<u32>,
        lease: lifecycle::Lease,
    ) -> Result<Self, String> {
        open_prepared(expected_sample_rate, lease, PreparedOutputKind::Legacy)?.into_legacy()
    }
}
/// Explicit output construction policy. Ordinary outputs preserve the device
/// default; only a plugin-capable open requests the bridge's finite buffer
/// profile. This is an off-callback request, never proof of actual delivery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkerOutputProfile {
    #[default]
    DeviceDefault,
    PluginBridge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedOutputKind {
    Legacy,
    Worker(WorkerOutputProfile),
    WorkerReliable(WorkerOutputProfile),
}
/// Device frames configure CoreAudio. Project frames bound each callback and
/// define the immutable VST3 transport quantum and its 2Q delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BridgeStreamPlan {
    device_frames: u16,
    project_quantum: u16,
    converting_device_rate: Option<u32>,
}

impl PreparedOutputKind {
    fn bridge_stream_plan(
        self,
        supported: cpal::SupportedBufferSize,
        device_rate: u32,
        project_rate: u32,
    ) -> Result<Option<BridgeStreamPlan>, String> {
        if !matches!(
            self,
            Self::Worker(WorkerOutputProfile::PluginBridge)
                | Self::WorkerReliable(WorkerOutputProfile::PluginBridge)
        ) {
            return Ok(None);
        }
        let quantum = hex_plugin_host::transport::MAX_FRAMES as u16;
        if device_rate == project_rate {
            let frames = bridge_callback_request(supported, u32::from(quantum), false)?;
            return Ok(Some(BridgeStreamPlan {
                device_frames: frames,
                project_quantum: frames,
                converting_device_rate: None,
            }));
        }
        // Choose a hardware buffer whose corresponding client duration fits Q.
        // AUHAL's initialized maximum is checked separately before startup.
        let cap = u64::from(quantum) * u64::from(device_rate) / u64::from(project_rate);
        let cap = cap.min(u64::from(quantum)) as u32;
        let device_frames = bridge_callback_request(supported, cap, true)?;
        Ok(Some(BridgeStreamPlan {
            device_frames,
            project_quantum: quantum,
            converting_device_rate: Some(device_rate),
        }))
    }
}

/// Worker-owned native output. Only take_client() may deliver its UI endpoint;
/// configure the virgin publication Root before that one-way boundary.
pub struct WorkerPreparedOutput {
    _stream: lifecycle::StreamOwner,
    pub publication: prepared::WorkerControl,
    client: Option<prepared::Client>,
    action_outcomes: Option<prepared::ActionOutcomeReader>,
    pub sample_rate: u32,
    pub label: String,
    pub spectrum_error: Option<String>,
    /// Default route this output opened on, when the backend reported it.
    /// Compare with [`crate::OutputRouteWatch`] to follow route changes.
    pub route: Option<crate::OutputRoute>,
}
impl WorkerPreparedOutput {
    /// Blocking device setup, intended only for the existing lifecycle worker.
    pub fn open() -> Result<Self, String> {
        Self::open_with_profile(WorkerOutputProfile::DeviceDefault)
    }
    /// Select before opening/exposing the actual backend. Never call this as a
    /// hidden device reconfiguration during insertion or Play.
    pub fn open_with_profile(profile: WorkerOutputProfile) -> Result<Self, String> {
        open_prepared(
            None,
            lifecycle::Lease::acquire()?,
            PreparedOutputKind::Worker(profile),
        )?
        .into_worker()
    }
    /// Open an output with bounded action outcomes selected before native
    /// callback exposure, including streamed playback.
    pub fn open_with_reliable_action_outcomes(
        profile: WorkerOutputProfile,
    ) -> Result<Self, String> {
        open_prepared(
            None,
            lifecycle::Lease::acquire()?,
            PreparedOutputKind::WorkerReliable(profile),
        )?
        .into_worker()
    }
    /// Take the unique observer; it can outlive this output and its client.
    pub fn take_action_outcomes(&mut self) -> Option<prepared::ActionOutcomeReader> {
        self.action_outcomes.take()
    }
    pub fn take_client(&mut self) -> Option<prepared::Client> {
        let mut client = self.client.take()?;
        client.activate();
        Some(client)
    }
}
pub(crate) struct Opened {
    stream: lifecycle::StreamOwner,
    legacy: Option<prepared::Control>,
    publication: Option<prepared::WorkerControl>,
    client: Option<prepared::Client>,
    action_outcomes: Option<prepared::ActionOutcomeReader>,
    sample_rate: u32,
    label: String,
    spectrum: Option<SpectrumReader>,
    spectrum_error: Option<String>,
    route: Option<crate::OutputRoute>,
}
impl Opened {
    pub(crate) fn into_legacy(self) -> Result<PreparedOutput, String> {
        Ok(PreparedOutput {
            _stream: self.stream,
            control: self.legacy.ok_or("legacy audio endpoint unavailable")?,
            sample_rate: self.sample_rate,
            label: self.label,
            spectrum: self.spectrum,
            spectrum_error: self.spectrum_error,
        })
    }
    pub(crate) fn into_worker(self) -> Result<WorkerPreparedOutput, String> {
        Ok(WorkerPreparedOutput {
            _stream: self.stream,
            publication: self
                .publication
                .ok_or("worker publication endpoint unavailable")?,
            client: Some(self.client.ok_or("worker command endpoint unavailable")?),
            action_outcomes: self.action_outcomes,
            sample_rate: self.sample_rate,
            label: self.label,
            spectrum_error: self.spectrum_error,
            route: self.route,
        })
    }
}
pub(crate) fn open_prepared(
    expected_sample_rate: Option<u32>,
    lease: lifecycle::Lease,
    kind: PreparedOutputKind,
) -> Result<Opened, OutputOpenError> {
    let worker = matches!(
        kind,
        PreparedOutputKind::Worker(_) | PreparedOutputKind::WorkerReliable(_)
    );
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default audio output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let route = crate::output_route::route_of(&device);
    let mut config = supported.config();
    let device_rate = config.sample_rate;
    // A converting backend runs the stream at the project rate; the device's
    // nominal rate is left as the user set it (see `open_outcome`).
    config.sample_rate = negotiate_stream_rate(
        expected_sample_rate,
        device_rate,
        BACKEND_CONVERTS_OUTPUT_RATE,
    )?;
    if config.channels == 0 {
        return Err("output device has no channels".into());
    }
    let sample_format = supported.sample_format();
    if !matches!(
        sample_format,
        cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
    ) {
        return Err(format!("unsupported output format: {sample_format:?}").into());
    }

    // CPAL's CoreAudio fixed request changes a device-wide property. Make it
    // explicit on the existing lifecycle open lane; ordinary native outputs
    // retain Default, including devices reporting Unknown buffer ranges.
    let bridge =
        kind.bridge_stream_plan(*supported.buffer_size(), device_rate, config.sample_rate)?;
    if let Some(plan) = bridge {
        config.buffer_size = cpal::BufferSize::Fixed(u32::from(plan.device_frames));
    }

    let (capture, mut spectrum, spectrum_reaper, spectrum_error) =
        if (8_000..=192_000).contains(&config.sample_rate) {
            let initial = SpectrumStamp {
                target: SpectrumTargetId(0),
                generation: 0,
                sample_rate_hz: config.sample_rate,
                applied_revision: 0,
            };
            let spawned = if worker {
                crate::spectrum_worker::spawn_native_spectrum_worker(initial)
            } else {
                spawn_spectrum_worker(initial)
            };
            match spawned {
                Ok((capture, reader, reaper)) => (Some(capture), Some(reader), Some(reaper), None),
                Err(error) => (None, None, None, Some(error.to_string())),
            }
        } else {
            (
                None,
                None,
                None,
                Some(format!(
                    "spectrum analysis unavailable at {} hz; supported range is 8000..=192000 hz",
                    config.sample_rate
                )),
            )
        };
    let channel_result =
        prepared::channel_with_interface(config.sample_rate, capture, Some(device_rate));
    let (mut control, mut callback) = match channel_result {
        Ok(channel) => channel,
        Err(error) => {
            // Even pre-channel failure transfers the actual worker/lease to
            // persistent lifecycle custody; no consuming join loses a panic.
            drop(lifecycle::StreamOwner::failed_preparation_custody(
                lease,
                spectrum_reaper,
            ));
            return Err(error.to_string().into());
        }
    };
    let action_outcomes = if matches!(kind, PreparedOutputKind::WorkerReliable(_)) {
        match control.enable_action_outcomes_on_control(&mut callback) {
            Ok(reader) => Some(reader),
            Err(error) => {
                drop(lifecycle::StreamOwner::failed_preparation_custody(
                    lease,
                    spectrum_reaper,
                ));
                return Err(error.to_owned().into());
            }
        }
    } else {
        None
    };
    if let Some(plan) = bridge
        && let Err(error) =
            control.configure_plugin_callback_quantum(&mut callback, plan.project_quantum)
    {
        drop(lifecycle::StreamOwner::failed_preparation_custody(
            lease,
            spectrum_reaper,
        ));
        return Err(error.to_string().into());
    }
    let root = control.native_candidate_root();
    let (data_guard, error_guard, terminals, mut legacy_control, mut worker_control, client) =
        if worker {
            let (data, errors, terminals, client, publication) =
                crate::native_callback_owner::reserve_native_worker(
                    control,
                    callback,
                    spectrum.take(),
                );
            (
                data,
                errors,
                terminals,
                None,
                Some(publication),
                Some(client),
            )
        } else {
            let (data, errors, terminals) =
                crate::native_callback_owner::reserve_native(callback, Arc::clone(&control.status));
            (data, errors, terminals, Some(control), None, None)
        };
    let native = crate::native_callback_owner::NativeRetirement::new(terminals, root);
    // Anchor both terminal receivers and lease BEFORE CPAL can start or
    // retain a callback, including failed construction with no Stream handle.
    let mut stream_owner = lifecycle::StreamOwner::prepared_custody(lease, spectrum_reaper, native);
    if let Some(control) = legacy_control.as_mut() {
        control.enable_native_custody()?;
    }
    if let Some(worker) = worker_control.as_mut() {
        worker
            .control()
            .ok_or("worker publication endpoint unavailable")?
            .enable_native_custody()?;
    }
    stream_owner.expose_backend();
    let stream = if sample_format == cpal::SampleFormat::F32 {
        build::<f32>(&device, &config, bridge, data_guard, error_guard)
    } else if sample_format == cpal::SampleFormat::I16 {
        build::<i16>(&device, &config, bridge, data_guard, error_guard)
    } else {
        build::<u16>(&device, &config, bridge, data_guard, error_guard)
    }
    .map_err(|error| error.to_string())?;
    stream_owner.set_stream(stream);
    stream_owner.play()?;
    let label = if config.sample_rate == device_rate {
        format!(
            "coreaudio · {} hz · {} channels",
            config.sample_rate, config.channels
        )
    } else {
        format!(
            "coreaudio · {} hz converted to device {} hz · {} channels",
            config.sample_rate, device_rate, config.channels
        )
    };
    Ok(Opened {
        stream: stream_owner,
        legacy: legacy_control,
        publication: worker_control,
        client,
        action_outcomes,
        sample_rate: config.sample_rate,
        label,
        spectrum,
        spectrum_error,
        route,
    })
}

/// Select within the device's reported finite range and the existing bridge
/// cap. Unknown/invalid ranges refuse rather than infer a bound from scratch
/// chunks. A backend can still refuse the fixed request during stream creation.
fn bridge_callback_request(
    supported: cpal::SupportedBufferSize,
    cap: u32,
    prefer_power_of_two: bool,
) -> Result<u16, String> {
    let cpal::SupportedBufferSize::Range { min, max } = supported else {
        return Err(
            "output device does not report a bounded buffer range for plugin processing".into(),
        );
    };
    let frames = max.min(cap);
    if min == 0 || min > max || min > frames {
        return Err(
            "output device cannot provide a buffer within the plugin callback limit".into(),
        );
    }
    let frames = if prefer_power_of_two {
        let conventional = 1 << frames.ilog2();
        if conventional >= min {
            conventional
        } else {
            frames
        }
    } else {
        frames
    };
    u16::try_from(frames).map_err(|_| "plugin callback size exceeds its supported range".into())
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    bridge: Option<BridgeStreamPlan>,
    mut data_guard: crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
    mut error_guard: crate::native_callback_owner::Guard<
        crate::native_callback_owner::ErrorCallbackOwner,
    >,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let bounded_bridge =
        bridge.and_then(|plan| Some((plan.converting_device_rate?, plan.project_quantum)));
    let data_callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        let Some(owner) = data_guard.owner_mut() else {
            data.fill(T::from_sample(0.0));
            return;
        };
        owner.run(|callback, status| {
            let start = Instant::now();
            callback.begin_block();
            if status.device_fault.load(Acquire) {
                callback.fence_device_fault();
                data.fill(T::from_sample(0.0));
                status.peak_left.store(0, Relaxed);
                status.peak_right.store(0, Relaxed);
                return;
            }
            let mut block: [Frame; 256] = [[0.0; 2]; 256];
            let mut peaks = [0.0_f32; 2];
            let complete = data.len() / channels * channels;
            data[complete..].fill(T::from_sample(0.0));
            if bounded_bridge.is_some_and(|(_, quantum)| {
                complete != data.len() || complete / channels > usize::from(quantum)
            }) || !valid_transport_buffer(callback, data.len(), channels)
            {
                // Render chunks are not physical callback bounds. Never
                // turn a large callback into hidden same-callback 2Q waits.
                callback.revoke_output_transport();
                data.fill(T::from_sample(0.0));
                callback.report_render_fault(
                    Some(&hex_engine::Invalid(
                        "plugin physical callback extent exceeds its fixed quantum",
                    )),
                    status.position.load(Relaxed),
                );
                return;
            }
            if let Err(error) = callback.begin_native_effect_callback(complete / channels) {
                data.fill(T::from_sample(0.0));
                callback.report_render_fault(Some(&error), status.position.load(Relaxed));
                return;
            }
            for chunk in data[..complete].chunks_mut(channels * block.len()) {
                let frames = chunk.len() / channels;
                let block_start_frame = status.position.load(Relaxed);
                let rendered = callback.render(&mut block[..frames]);
                finish_host_chunk(
                    callback,
                    chunk,
                    &mut block[..frames],
                    channels,
                    &rendered,
                    block_start_frame,
                    start,
                );
                for stereo in &block[..frames] {
                    for channel in 0..2 {
                        peaks[channel] = peaks[channel].max(stereo[channel].abs());
                    }
                }
            }
            status.peak_left.store(peaks[0].to_bits(), Relaxed);
            status.peak_right.store(peaks[1].to_bits(), Relaxed);
            let elapsed = start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            status
                .cpu_meter
                .record(elapsed, data.len() / channels, rate);
            status.callback_max_ns.fetch_max(elapsed, Relaxed);
            let deadline = (data.len() / channels) as u64 * 1_000_000_000 / u64::from(rate);
            if elapsed > deadline {
                status.deadline_misses.fetch_add(1, Relaxed);
            }
            callback.trace_callback_timing(elapsed, deadline, data.len() / channels);
        });
    };
    let error_callback = move |_| {
        if let Some(owner) = error_guard.owner_mut() {
            owner.report();
        }
    };
    #[cfg(target_os = "macos")]
    if let Some((device_rate, limit)) = bounded_bridge {
        match device.as_inner() {
            cpal::platform::DeviceInner::CoreAudio(device) => {
                let stream = device.build_bounded_output_stream(
                    config,
                    device_rate,
                    limit,
                    data_callback,
                    error_callback,
                )?;
                return Ok(cpal::platform::StreamInner::CoreAudio(stream).into());
            }
            #[allow(unreachable_patterns)]
            _ => return Err(cpal::BuildStreamError::StreamConfigNotSupported),
        }
    }
    device.build_output_stream(config, data_callback, error_callback, None)
}

pub(crate) fn valid_transport_buffer(
    callback: &prepared::Callback,
    samples: usize,
    channels: usize,
) -> bool {
    callback
        .output_transport_quantum()
        .into_iter()
        .chain(callback.native_effect_quantum())
        .min()
        .is_none_or(|quantum| {
            channels != 0 && samples.is_multiple_of(channels) && samples / channels <= quantum
        })
}

pub(crate) fn finish_host_chunk<T: SizedSample + FromSample<f32>>(
    callback: &mut prepared::Callback,
    output: &mut [T],
    source: &mut [Frame],
    channels: usize,
    rendered: &Result<(), hex_engine::Invalid>,
    block_start_frame: u64,
    capture_start: Instant,
) -> bool
where
    f32: FromSample<T>,
{
    let valid = if callback.output_transport_quantum().is_some() {
        let staged = rendered.is_ok() && callback.stage_output_transport(source);
        let converted = staged && copy_host_chunk(output, source, channels);
        let delivery = callback.finish_output_transport(converted);
        finish_transport_copy(
            delivery,
            converted,
            rendered.is_ok(),
            output,
            source,
            channels,
        )
    } else {
        // Keep ordinary output on its existing direct conversion path.
        rendered.is_ok() && copy_host_chunk(output, source, channels)
    };
    let delivery = callback.finish_native_effect_callback(valid);
    let valid = finish_transport_copy(delivery, valid, rendered.is_ok(), output, source, channels);
    let _ = callback.finish_spectrum_block(capture_start, valid);
    callback.finish_sample_recording(valid);
    callback.finish_midi_recording_quantum(valid);
    if !valid {
        output.fill(T::from_sample(0.0));
        source.fill([0.0; 2]);
        let transport_error = callback.output_transport_error();
        callback.report_render_fault(
            rendered.as_ref().err().or(transport_error.as_ref()),
            block_start_frame,
        );
    }
    callback.finish_gain_recording_quantum(valid);
    callback.finish_native_parameter_quantum(valid);
    valid
}

/// Final host ownership boundary: a clean terminal cancellation after copying
/// still replaces that output with silence. It never accepts worker input.
fn finish_transport_copy<T: SizedSample + FromSample<f32>>(
    delivery: prepared::OutputTransportDelivery,
    converted: bool,
    rendered: bool,
    output: &mut [T],
    source: &mut [Frame],
    channels: usize,
) -> bool
where
    f32: FromSample<T>,
{
    let valid = match delivery {
        prepared::OutputTransportDelivery::Accepted => converted,
        prepared::OutputTransportDelivery::Refused => false,
        prepared::OutputTransportDelivery::Cancelled => {
            source.fill([0.0; 2]);
            rendered && copy_host_chunk(output, source, channels)
        }
    };
    if !valid {
        output.fill(T::from_sample(0.0));
        source.fill([0.0; 2]);
    }
    valid
}

#[cfg(test)]
mod plugin_transport_tests;

#[cfg(test)]
mod physical_callback_tests;

#[cfg(test)]
mod handoff_profile;

// Preserve the existing rounding (including subnormals) when the sum fits.
// A same-sign overflowing sum still has a representable mean: halving each
// operand first cannot overflow. Source finiteness is checked by the caller.
fn mono_mean(left: f32, right: f32) -> f32 {
    let sum = left + right;
    if sum.is_finite() {
        sum * 0.5
    } else {
        left * 0.5 + right * 0.5
    }
}

fn host_value(stereo: &Frame, channels: usize, channel: usize) -> f32 {
    if channels == 1 {
        mono_mean(stereo[0], stereo[1])
    } else if channel < 2 {
        stereo[channel]
    } else {
        0.0
    }
}

// Validate the complete final channel/sample-format conversion before writing
// any host sample or publishing successful output evidence. The second pass
// copies the same deterministic conversions without temporary allocation.
// Supported i16/u16 retain CPAL's existing saturating conversion semantics;
// the f32 path retains finite values without clipping.
fn copy_host_chunk<T: SizedSample + FromSample<f32>>(
    output: &mut [T],
    source: &[Frame],
    channels: usize,
) -> bool
where
    f32: FromSample<T>,
{
    let valid = source.len().checked_mul(channels) == Some(output.len())
        && host_samples_are_finite::<T>(source, channels);
    if !valid {
        output.fill(T::from_sample(0.0));
        return false;
    }
    for (frame, stereo) in output.chunks_mut(channels).zip(source) {
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = T::from_sample(host_value(stereo, channels, channel));
        }
    }
    true
}

fn host_samples_are_finite<T: SizedSample + FromSample<f32>>(
    source: &[Frame],
    channels: usize,
) -> bool
where
    f32: FromSample<T>,
{
    channels != 0
        && source.iter().all(|stereo| {
            stereo.iter().all(|sample| sample.is_finite())
                && (0..channels).all(|channel| {
                    let converted = T::from_sample(host_value(stereo, channels, channel));
                    f32::from_sample(converted).is_finite()
                })
        })
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn headless_worker_output(
    rate: u32,
) -> Result<(WorkerPreparedOutput, crate::worker_fixture::HeadlessBackend), String> {
    headless_worker_output_with_outcomes(rate, false)
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn headless_worker_output_with_outcomes(
    rate: u32,
    reliable: bool,
) -> Result<(WorkerPreparedOutput, crate::worker_fixture::HeadlessBackend), String> {
    let lease = lifecycle::Lease::acquire()?;
    let (mut control, mut callback) = prepared::channel(rate).map_err(|e| e.to_string())?;
    let action_outcomes = if reliable {
        Some(
            control
                .enable_action_outcomes_on_control(&mut callback)
                .map_err(str::to_owned)?,
        )
    } else {
        None
    };
    control
        .configure_plugin_callback_quantum(
            &mut callback,
            hex_plugin_host::transport::MAX_FRAMES as u16,
        )
        .map_err(|error| error.to_string())?;
    let root = control.native_candidate_root();
    let (data, errors, terminals, client, mut publication) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None);
    let native = crate::native_callback_owner::NativeRetirement::new(terminals, root);
    let mut stream = lifecycle::StreamOwner::prepared_custody(lease, None, native);
    stream.expose_headless_backend();
    publication
        .control()
        .ok_or("worker publication endpoint unavailable")?
        .enable_native_custody()?;
    Ok((
        WorkerPreparedOutput {
            _stream: stream,
            publication,
            client: Some(client),
            action_outcomes,
            sample_rate: rate,
            label: "headless constructor fixture".into(),
            spectrum_error: None,
            route: None,
        },
        crate::worker_fixture::HeadlessBackend::new(data, errors),
    ))
}

#[cfg(test)]
mod tests {
    use super::{copy_host_chunk, finish_host_chunk, mono_mean};
    use crate::gain_recording::{AbortReason, Receipt, Target};
    use crate::prepared;
    use cpal::{FromSample, SizedSample};
    use hex_engine::{Audio, PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
    use std::{
        collections::BTreeMap,
        sync::{Arc, atomic::Ordering::Acquire},
        time::Instant,
    };

    #[test]
    fn finite_mono_extrema_have_representable_symmetric_means() {
        // The pre-fix sum overflows although both inputs and their mean fit.
        assert!(!((f32::MAX + f32::MAX) * 0.5).is_finite());
        let inputs = [
            f32::MAX,
            f32::from_bits(f32::MAX.to_bits() - 1),
            f32::MAX * 0.75,
            -f32::MAX,
            -f32::MAX * 0.75,
            1.0,
            -1.0,
            0.0,
            -0.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
        ];
        for left in inputs {
            for right in inputs {
                let actual = mono_mean(left, right);
                assert!(actual.is_finite(), "{left} / {right}");
                assert_eq!(actual, mono_mean(right, left));
                let sum = left + right;
                if sum.is_finite() {
                    assert_eq!(actual.to_bits(), (sum * 0.5).to_bits());
                } else {
                    // Independent wider mathematical oracle for overflow cases.
                    let expected = ((f64::from(left) + f64::from(right)) * 0.5) as f32;
                    assert_eq!(actual, expected);
                }
            }
        }
        assert_eq!(mono_mean(f32::MAX, -f32::MAX), 0.0);
        let mut mono = [0.0; 3];
        assert!(copy_host_chunk(
            &mut mono,
            &[[f32::MAX; 2], [-f32::MAX; 2], [f32::MAX, -f32::MAX]],
            1,
        ));
        assert_eq!(mono, [f32::MAX, -f32::MAX, 0.0]);
    }

    #[test]
    fn ordinary_host_layout_and_sample_formats_retain_previous_values() {
        let source = [[0.25, -0.75], [-1.0, 1.0], [0.0, -0.0]];
        for channels in [1, 2, 4] {
            let mut floats = vec![9.0_f32; source.len() * channels];
            let mut signed = vec![9_i16; source.len() * channels];
            let mut unsigned = vec![9_u16; source.len() * channels];
            assert!(copy_host_chunk(&mut floats, &source, channels));
            assert!(copy_host_chunk(&mut signed, &source, channels));
            assert!(copy_host_chunk(&mut unsigned, &source, channels));
            for (index, stereo) in source.iter().enumerate() {
                for channel in 0..channels {
                    let expected = if channels == 1 {
                        (stereo[0] + stereo[1]) * 0.5
                    } else {
                        stereo.get(channel).copied().unwrap_or(0.0)
                    };
                    let sample = index * channels + channel;
                    assert_eq!(floats[sample].to_bits(), expected.to_bits());
                    assert_eq!(signed[sample], <i16 as cpal::Sample>::from_sample(expected));
                    assert_eq!(
                        unsigned[sample],
                        <u16 as cpal::Sample>::from_sample(expected)
                    );
                }
            }
        }
        // Integer endpoints keep their existing saturation; unsigned silence
        // uses midpoint rather than zero. Float endpoints are not clipped.
        let source = [[f32::MAX; 2], [-f32::MAX; 2], [0.0; 2]];
        let mut signed = [0_i16; 3];
        let mut unsigned = [0_u16; 3];
        assert!(copy_host_chunk(&mut signed, &source, 1));
        assert!(copy_host_chunk(&mut unsigned, &source, 1));
        assert_eq!(signed, [i16::MAX, i16::MIN, 0]);
        assert_eq!(unsigned, [u16::MAX, u16::MIN, 32_768]);
    }

    #[test]
    fn invalid_late_sample_silences_only_its_chunk_in_every_host_format() {
        for channels in [1, 2, 4] {
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                for lane in 0..2 {
                    let mut source = [[0.25, -0.75]; 2];
                    source[1][lane] = invalid;
                    let count = channels * source.len();
                    let mut floats = vec![0.0; count * 2];
                    let mut signed = vec![0_i16; count * 2];
                    let mut unsigned = vec![0_u16; count * 2];
                    assert!(copy_host_chunk(
                        &mut floats[..count],
                        &[[0.5; 2]; 2],
                        channels
                    ));
                    assert!(copy_host_chunk(
                        &mut signed[..count],
                        &[[0.5; 2]; 2],
                        channels
                    ));
                    assert!(copy_host_chunk(
                        &mut unsigned[..count],
                        &[[0.5; 2]; 2],
                        channels
                    ));
                    let previous = (
                        floats[..count].to_vec(),
                        signed[..count].to_vec(),
                        unsigned[..count].to_vec(),
                    );
                    assert!(!copy_host_chunk(&mut floats[count..], &source, channels));
                    assert!(!copy_host_chunk(&mut signed[count..], &source, channels));
                    assert!(!copy_host_chunk(&mut unsigned[count..], &source, channels));
                    assert_eq!(floats[..count], previous.0);
                    assert_eq!(signed[..count], previous.1);
                    assert_eq!(unsigned[..count], previous.2);
                    assert!(floats[count..].iter().all(|value| *value == 0.0));
                    assert!(signed[count..].iter().all(|value| *value == 0));
                    assert!(unsigned[count..].iter().all(|value| *value == 32_768));
                }
            }
        }
    }

    #[test]
    fn invalid_host_shape_is_refused_before_copy() {
        let mut output = [9.0; 3];
        assert!(!copy_host_chunk(&mut output, &[[0.5; 2]; 2], 2));
        assert_eq!(output, [0.0; 3]);
        assert!(!copy_host_chunk(&mut output, &[], 0));
        assert!(!copy_host_chunk(&mut output, &[[0.0; 2]], usize::MAX));
    }

    fn recording_callback() -> (prepared::Control, prepared::Callback, Target) {
        recording_callback_with_main_limiter(false)
    }

    fn recording_callback_with_main_limiter(
        main_limiter: bool,
    ) -> (prepared::Control, prepared::Callback, Target) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "host conversion".into(),
                path: "memory.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("expected imported track");
        };
        if main_limiter {
            use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
            let definition = &hex_devices::limiter::DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(2),
                )
                .unwrap();
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        }
        let revision = Arc::new(
            PreparedRevision::prepare_for_gain_recording(
                editor.project(),
                &BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap())]),
                Arc::new(SineTable::new()),
                None,
                track,
            )
            .unwrap(),
        );
        if main_limiter {
            assert_eq!(revision.arrangement().main_latency_frames(), 288);
        }
        let playback = Playback::try_from_prepared_with_session(revision).unwrap();
        let (mut control, mut callback) = prepared::channel(48_000).unwrap();
        let revision = control.publish(playback).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let mut source = [[0.0; 2]; 16];
        callback.render(&mut source).unwrap();
        let mut output = [0.0; 32];
        assert!(checked_host_finish(
            &mut callback,
            &mut output,
            &mut source,
            2,
            &Ok(()),
            0,
            Instant::now(),
        ));
        assert!(control.status.gain_recording_ready.load(Acquire));
        let target = Target {
            track_id: track.value(),
            project_generation: 0,
            canonical_revision: 1,
            audio_revision: revision,
            recording_epoch: 1,
            transport_epoch: control.status.transport_epoch.load(Acquire),
            gesture_serial: 1,
            sample_rate: 48_000,
        };
        (control, callback, target)
    }

    fn checked_host_finish<T: SizedSample + FromSample<f32>>(
        callback: &mut prepared::Callback,
        output: &mut [T],
        source: &mut [hex_engine::Frame],
        channels: usize,
        rendered: &Result<(), hex_engine::Invalid>,
        block_start_frame: u64,
        capture_start: Instant,
    ) -> bool
    where
        f32: FromSample<T>,
    {
        #[cfg(feature = "allocation-probe")]
        {
            let mut accepted = false;
            let measured = allocation_counter::measure(|| {
                accepted = finish_host_chunk(
                    callback,
                    output,
                    source,
                    channels,
                    rendered,
                    block_start_frame,
                    capture_start,
                );
            });
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
            accepted
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            finish_host_chunk(
                callback,
                output,
                source,
                channels,
                rendered,
                block_start_frame,
                capture_start,
            )
        }
    }

    // Rendering and final conversion are independently instrumented. Construction,
    // assertions, receipt collection and reclamation remain outside both scopes.
    fn checked_recording_render(
        callback: &mut prepared::Callback,
        source: &mut [hex_engine::Frame],
    ) -> Result<(), hex_engine::Invalid> {
        #[cfg(feature = "allocation-probe")]
        {
            let mut result = Ok(());
            let info = allocation_counter::measure(|| {
                callback.begin_block();
                result = callback.render(source);
            });
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
            result
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(source)
        }
    }

    #[test]
    fn gain_begin_racing_fault_and_prequeued_recovery_keeps_failure_cause() {
        let (mut control, mut callback, target) = recording_callback();
        let replacement =
            hex_engine::playback::Playback::new(hex_engine::Project::new(48_000, vec![]).unwrap());
        control.publish(replacement).unwrap();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        control.gain_recording.begin_gate = Some(gate.clone());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                gate.wait();
                callback.report_render_fault(Some(&hex_engine::Invalid("injected failure")), 16);
                callback.begin_block();
                gate.wait();
            });
            control.gain_recording.begin(target, 0.8).unwrap();
        });
        control.gain_recording.begin_gate = None;
        let mut source = [[0.0; 2]; 8];
        checked_recording_render(&mut callback, &mut source).unwrap();
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Superseded { .. })
        ));
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Aborted {
                reason: AbortReason::OutputFailed,
                ..
            })
        ));
        assert!(!control.status.render_fault.load(Acquire));
        assert!(control.status.reserve_midi_begin().is_ok());
        control.status.midi_end();
    }

    #[test]
    fn render_fault_aborts_a_racing_gain_take() {
        let (mut control, mut callback, target) = recording_callback();
        control.gain_recording.begin(target, 0.8).unwrap();
        control
            .status
            .render_fault
            .store(true, std::sync::atomic::Ordering::Release);
        let mut source = [[0.0; 2]; 8];
        checked_recording_render(&mut callback, &mut source).unwrap();
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Superseded { .. })
        ));
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Aborted {
                reason: AbortReason::OutputFailed,
                ..
            })
        ));
    }

    #[test]
    fn device_error_aborts_a_queued_gain_take() {
        let (mut control, mut callback, target) = recording_callback();
        control.gain_recording.begin(target, 0.8).unwrap();
        control
            .status
            .device_fault
            .store(true, std::sync::atomic::Ordering::Release);
        let mut source = [[1.0; 2]; 8];
        assert!(checked_recording_render(&mut callback, &mut source).is_ok());
        assert_eq!(source, [[0.0; 2]; 8]);
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Superseded { .. })
        ));
        assert!(matches!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Aborted {
                reason: AbortReason::OutputFailed,
                ..
            })
        ));
        assert!(control.gain_recording.begin(target, 0.8).is_err());
    }

    #[test]
    fn gain_recording_receipt_waits_for_serial_main_limiter_sample_to_be_written() {
        const DELAY: usize = 288;
        const TOUCH: usize = 16;
        const FIRST_WRITTEN: usize = TOUCH + DELAY;
        let (mut control, mut callback, target) = recording_callback_with_main_limiter(true);
        // The fixture has actually written sixteen initial zero frames. The
        // Limiter is transparent for this signal, with precisely 288 delay frames.
        control.gain_recording.begin(target, 0.8).unwrap();
        let mut source = [[0.0; 2]; 128];
        let mut output = [0.0_f32; 256];
        let mut frame = TOUCH;
        for length in [16, 7, 63, 1, 31, 128].into_iter().cycle() {
            if frame == FIRST_WRITTEN {
                break;
            }
            let length = length.min(FIRST_WRITTEN - frame);
            let rendered = checked_recording_render(&mut callback, &mut source[..length]);
            assert!(rendered.is_ok());
            assert!(
                control.gain_recording.try_receipt().is_none(),
                "rendering alone is not sink delivery"
            );
            assert!(checked_host_finish(
                &mut callback,
                &mut output[..length * 2],
                &mut source[..length],
                2,
                &rendered,
                frame as u64,
                Instant::now()
            ));
            // Independent PCM oracle: Main retains every dry source frame for
            // 288 frames. None of the touched content is in this delivered prefix.
            for (offset, actual) in output[..length * 2].chunks_exact(2).enumerate() {
                let expected = if frame + offset < DELAY {
                    [0.0, 0.0]
                } else {
                    [0.25, -0.5]
                };
                assert_eq!(actual, expected);
            }
            assert!(
                control.gain_recording.try_receipt().is_none(),
                "gain must not be acknowledged before output frame 304 is written"
            );
            frame += length;
        }
        // The half-open delivered prefix ends at 304. Only this next successful
        // native write contains source frame 16 with the new fader gain.
        let rendered = checked_recording_render(&mut callback, &mut source[..1]);
        assert!(rendered.is_ok());
        assert!(control.gain_recording.try_receipt().is_none());
        assert!(checked_host_finish(
            &mut callback,
            &mut output[..2],
            &mut source[..1],
            2,
            &rendered,
            FIRST_WRITTEN as u64,
            Instant::now()
        ));
        let first_gain = 1.0 - 0.2 / 240.0;
        assert!((output[0] - 0.25 * first_gain).abs() < 1e-6);
        assert!((output[1] + 0.5 * first_gain).abs() < 1e-6);
        assert_eq!(
            control.gain_recording.try_receipt(),
            Some(Receipt::Applied {
                target,
                sequence: 1,
                gain: 0.8,
                performance_frame: TOUCH as u64,
                content_frame: TOUCH as u64,
                loop_pass: 0,
            })
        );
        assert!(control.gain_recording.try_receipt().is_none());
        control.request_shutdown();
        let rendered = checked_recording_render(&mut callback, &mut source[..1]);
        assert!(rendered.is_ok());
        assert!(checked_host_finish(
            &mut callback,
            &mut output[..2],
            &mut source[..1],
            2,
            &rendered,
            (FIRST_WRITTEN + 1) as u64,
            Instant::now()
        ));
        control.reclaim();
    }

    fn host_copy_receipt_regression<T: SizedSample + FromSample<f32> + std::fmt::Debug>()
    where
        f32: FromSample<T>,
    {
        for channels in [1, 2, 4] {
            // Nonfinite conversion input, a render error with finite PCM, and
            // a refused final layout must all abort delivery identically.
            for refusal in 0..3 {
                let (mut control, mut callback, target) = recording_callback();
                control.gain_recording.begin(target, 0.8).unwrap();
                callback.begin_block();
                let mut source = [[0.0; 2]; 16];
                let rendered = callback.render(&mut source);
                assert!(rendered.is_ok());
                assert!(control.gain_recording.try_receipt().is_none());
                // Inject extremes after real engine rendering to exercise the
                // actual adapter's representable mean and delivery receipt.
                source[0] = [f32::MAX; 2];
                let count = channels * source.len();
                let mut output = vec![T::from_sample(0.0); count * 2];
                assert!(checked_host_finish(
                    &mut callback,
                    &mut output[..count],
                    &mut source,
                    channels,
                    &rendered,
                    16,
                    Instant::now(),
                ));
                assert_eq!(output[0], T::from_sample(f32::MAX));
                assert!(matches!(control.gain_recording.try_receipt(),
                    Some(Receipt::Applied { target: actual, sequence: 1, gain: 0.8, .. })
                    if actual == target));
                let prefix = output[..count].to_vec();

                control.gain_recording.change(0.5).unwrap();
                callback.begin_block();
                let rendered = callback.render(&mut source);
                assert!(rendered.is_ok());
                assert!(source.iter().flatten().all(|sample| sample.is_finite()));
                assert!(control.gain_recording.try_receipt().is_none());
                let rendered = if refusal == 1 {
                    Err(hex_engine::Invalid(
                        "host conversion regression render refusal",
                    ))
                } else {
                    if refusal == 0 {
                        source[15][1] = f32::NAN;
                    }
                    rendered
                };
                let output_end = if refusal == 2 {
                    count * 2 - 1
                } else {
                    count * 2
                };
                assert!(!checked_host_finish(
                    &mut callback,
                    &mut output[count..output_end],
                    &mut source,
                    channels,
                    &rendered,
                    32,
                    Instant::now(),
                ));
                assert_eq!(output[..count], prefix);
                assert!(
                    output[count..]
                        .iter()
                        .all(|value| *value == T::from_sample(0.0))
                );
                assert_eq!(source, [[0.0; 2]; 16]);
                assert!(control.status.render_fault.load(Acquire));
                assert!(!control.status.playing.load(Acquire));
                assert!(matches!(control.gain_recording.try_receipt(),
                    Some(Receipt::Superseded { target: actual, sequence: 2 })
                    if actual == target));
                assert!(matches!(control.gain_recording.try_receipt(),
                    Some(Receipt::Aborted { target: actual, reason: AbortReason::OutputFailed })
                    if actual == target));
                assert!(control.gain_recording.try_receipt().is_none());
                assert!(control.gain_recording.target().is_none());

                // A later successful silent copy must never acknowledge the
                // aborted value or alter samples in the committed prefix.
                callback.begin_block();
                let rendered = callback.render(&mut source);
                assert!(rendered.is_ok());
                assert_eq!(source, [[0.0; 2]; 16]);
                assert!(checked_host_finish(
                    &mut callback,
                    &mut output[count..],
                    &mut source,
                    channels,
                    &rendered,
                    48,
                    Instant::now(),
                ));
                assert!(control.gain_recording.try_receipt().is_none());
                assert_eq!(output[..count], prefix);
            }
        }
    }

    #[test]
    fn actual_host_finish_commits_extrema_but_refused_chunks_never_acknowledge_gain() {
        host_copy_receipt_regression::<f32>();
        host_copy_receipt_regression::<i16>();
        host_copy_receipt_regression::<u16>();
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn successful_and_failed_host_conversion_allocate_nothing() {
        let source = [[f32::MAX; 2]; 256];
        let mut invalid = source;
        invalid[255][1] = f32::NAN;
        let mut floats = [0.0; 1024];
        let mut signed = [0_i16; 1024];
        let mut unsigned = [0_u16; 1024];
        let mut outcomes = [false; 6];
        let measured = allocation_counter::measure(|| {
            for channels in [1, 2, 4] {
                let samples = 256 * channels;
                outcomes[0] = copy_host_chunk(&mut floats[..samples], &source, channels);
                outcomes[1] = copy_host_chunk(&mut signed[..samples], &source, channels);
                outcomes[2] = copy_host_chunk(&mut unsigned[..samples], &source, channels);
                outcomes[3] = copy_host_chunk(&mut floats[..samples], &invalid, channels);
                outcomes[4] = copy_host_chunk(&mut signed[..samples], &invalid, channels);
                outcomes[5] = copy_host_chunk(&mut unsigned[..samples], &invalid, channels);
            }
        });
        assert_eq!(outcomes, [true, true, true, false, false, false]);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}

#[cfg(test)]
#[path = "device_midi_recording_tests.rs"]
mod midi_recording_tests;

#[cfg(test)]
#[path = "device_native_parameter_input_tests.rs"]
pub(crate) mod native_parameter_input_tests;
