//! Device adapter for the canonical compiled playback handoff.
use crate::spectrum_worker::SpectrumReader;
use crate::{lifecycle, prepared};
use cpal::{
    FromSample, Sample, SizedSample,
    traits::{DeviceTrait, HostTrait},
};
use hex_engine::Frame;
use std::{sync::atomic::Ordering::Relaxed, time::Instant};

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
        open_prepared(expected_sample_rate, lease, false, None)?.into_legacy()
    }
}
/// Worker-owned native output. Only take_client() may deliver its UI endpoint;
/// configure the virgin publication Root before that one-way boundary.
pub struct WorkerPreparedOutput {
    _stream: lifecycle::StreamOwner,
    pub publication: prepared::WorkerControl,
    client: Option<prepared::Client>,
    pub sample_rate: u32,
    pub label: String,
    pub spectrum_error: Option<String>,
}
impl WorkerPreparedOutput {
    /// Blocking device setup, intended only for the existing lifecycle worker.
    pub fn open() -> Result<Self, String> {
        Self::open_with_lease(None, lifecycle::Lease::acquire()?)
    }
    pub(crate) fn open_with_lease(
        expected_sample_rate: Option<u32>,
        lease: lifecycle::Lease,
    ) -> Result<Self, String> {
        open_prepared(expected_sample_rate, lease, true, None)?.into_worker()
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
    sample_rate: u32,
    label: String,
    spectrum: Option<SpectrumReader>,
    spectrum_error: Option<String>,
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
            sample_rate: self.sample_rate,
            label: self.label,
            spectrum_error: self.spectrum_error,
        })
    }
}
pub(crate) fn open_prepared(
    expected_sample_rate: Option<u32>,
    lease: lifecycle::Lease,
    worker: bool,
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
) -> Result<Opened, String> {
    let negotiated = negotiate_default_output(expected_sample_rate)?;
    complete_prepared_open(negotiated, lease, worker, factory_reservation)
}

// Native backend configuration is RESOURCE, not a fabricated prepared channel.
// In particular negotiation never creates a Root/Reaper/empty callback owner.
struct NegotiatedOutput {
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
}
fn negotiate_default_output(expected_sample_rate: Option<u32>) -> Result<NegotiatedOutput, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no default audio output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    validate_expected_rate(expected_sample_rate, config.sample_rate)?;
    let sample_format = supported.sample_format();
    validate_output_configuration(config.channels, sample_format)?;
    Ok(NegotiatedOutput {
        device,
        config,
        sample_format,
    })
}

fn validate_output_configuration(
    channels: u16,
    sample_format: cpal::SampleFormat,
) -> Result<(), String> {
    if channels == 0 {
        return Err("output device has no channels".into());
    }
    if !matches!(
        sample_format,
        cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
    ) {
        return Err(format!("unsupported output format: {sample_format:?}"));
    }
    Ok(())
}

// Move-only stage of the existing open operation, never a second custodian.
// Guards/endpoints drop before the same retirement parent on rejected handoff.
pub(crate) struct PreparedBackend {
    // The actual negotiated backend selection travels with this DATA stage.
    // Only the resource-negotiating constructor installs it; a sample-rate
    // scalar cannot recreate or qualify a device selection.
    negotiated: Option<NegotiatedOutput>,
    data_guard:
        crate::native_callback_owner::Guard<crate::native_callback_owner::NativeCallbackOwner>,
    error_guard:
        crate::native_callback_owner::Guard<crate::native_callback_owner::ErrorCallbackOwner>,
    legacy: Option<prepared::Control>,
    publication: Option<prepared::WorkerControl>,
    client: Option<prepared::Client>,
    spectrum: Option<SpectrumReader>,
    spectrum_error: Option<String>,
    sample_rate: u32,
    stream_owner: lifecycle::StreamOwner,
}
#[derive(Debug)]
pub(crate) enum PreparedDataError {
    Constructor(hex_engine::Invalid),
    Association,
    Endpoint(String),
}
impl PreparedBackend {
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    // Kind/consistency check on the actual private constructor fields, not a
    // caller's worker flag or an admission/retirement certificate. The later
    // complete host/lineage/guard traversal remains mandatory.
    pub(crate) fn has_worker_endpoints(&self) -> bool {
        let Self {
            negotiated: _,
            data_guard,
            error_guard,
            legacy,
            publication,
            client,
            spectrum,
            spectrum_error: _,
            sample_rate: _,
            stream_owner: _,
        } = self;
        let Some(publication) = publication
            .as_ref()
            .and_then(prepared::WorkerControl::owner_ref)
        else {
            return false;
        };
        let Some(client) = client
            .as_ref()
            .filter(|client| client.owner_ref().is_some())
        else {
            return false;
        };
        legacy.is_none()
            && spectrum.is_none()
            && data_guard.owner_ref().is_some()
            && error_guard.owner_ref().is_some()
            && publication.output_identity().is_some()
            && publication.output_identity() == client.output_identity()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_output_identity(&self) -> u64 {
        self.client
            .as_ref()
            .and_then(prepared::Client::output_identity)
            .or_else(|| {
                self.legacy
                    .as_ref()
                    .and_then(|control| control.output_identity())
            })
            .unwrap()
    }
}
impl std::fmt::Display for PreparedDataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Constructor(error) => std::fmt::Display::fmt(error, formatter),
            Self::Association => {
                formatter.write_str("native carrier constructor association refused")
            }
            Self::Endpoint(error) => formatter.write_str(error),
        }
    }
}

// Includes the ACTUAL inline parent exactly once. Descendants remain separately
// priced by the Root/channel/analyzer constructor and mandatory host inventory.
pub(crate) fn prepared_open_stage_body_bytes() -> usize {
    std::mem::size_of::<PreparedBackend>()
}
pub(crate) fn prepared_open_stage_scratch_bytes() -> Option<usize> {
    std::mem::size_of::<PreparedBackend>()
        .checked_add(std::mem::size_of::<NegotiatedOutput>())?
        .checked_add(std::mem::size_of::<Box<PreparedBackend>>())
}

fn complete_prepared_open(
    negotiated: NegotiatedOutput,
    lease: lifecycle::Lease,
    worker: bool,
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
) -> Result<Opened, String> {
    let mut prepared = prepare_backend_data(
        lease,
        negotiated.config.sample_rate,
        worker,
        factory_reservation,
    )
    .map_err(|error| error.to_string())?;
    prepared.negotiated = Some(negotiated);
    finish_prepared_backend(prepared)
}

pub(crate) fn prepare_backend_data(
    lease: lifecycle::Lease,
    sample_rate: u32,
    worker: bool,
    factory_reservation: Option<crate::AudioCandidatePublicationReservation>,
) -> Result<Box<PreparedBackend>, PreparedDataError> {
    let channel = lifecycle::StreamOwner::construct_prepared_channel_on_control(
        lease,
        sample_rate,
        worker,
        factory_reservation,
    )
    .map_err(PreparedDataError::Constructor)?;
    install_backend_carriers(channel, sample_rate, worker)
}

// Same actual carrier constructor for the ordinary analyzer-bearing operation
// and a headless channel whose parent genuinely never requested an analyzer.
fn install_backend_carriers(
    channel: lifecycle::ConstructedChannel,
    sample_rate: u32,
    worker: bool,
) -> Result<Box<PreparedBackend>, PreparedDataError> {
    let (mut stream_owner, control, callback, mut spectrum, spectrum_error) = channel;
    let spectrum_error = spectrum_error.map(|error| error.to_string());
    let endpoints = if worker {
        crate::native_callback_owner::reserve_native_worker(control, callback, spectrum.take()).map(
            |(data, errors, terminals, client, publication)| {
                (
                    data,
                    errors,
                    terminals,
                    None,
                    Some(publication),
                    Some(client),
                )
            },
        )
    } else {
        crate::native_callback_owner::reserve_native(callback, &control.status)
            .map(|(data, errors, terminals)| (data, errors, terminals, Some(control), None, None))
    };
    let (data_guard, error_guard, terminals, mut legacy_control, mut worker_control, client) =
        match endpoints {
            Ok(endpoints) => endpoints,
            Err(error) => {
                // reserve_native(_worker) returned after normally dropping
                // its rejected channel inputs. Retain the actual existing
                // Root/Reaper/lease parent; never invent a failure custodian.
                let _aborted = stream_owner.abort_before_carriers();
                return Err(PreparedDataError::Constructor(error));
            }
        };
    let mut terminals = Some(terminals);
    if stream_owner
        .install_constructed_native_carriers_on_control(&mut terminals)
        .is_err()
    {
        // The intended empty destination consumes and retains the actual
        // terminals BEFORE association validation. A duplicate refusal leaves
        // its incoming owner in this explicit local destination.
        return Err(PreparedDataError::Association);
    }
    if let Some(control) = legacy_control.as_mut() {
        control
            .enable_native_custody()
            .map_err(PreparedDataError::Endpoint)?;
    }
    if let Some(worker) = worker_control.as_mut() {
        worker
            .control()
            .ok_or(PreparedDataError::Association)?
            .enable_native_custody()
            .map_err(PreparedDataError::Endpoint)?;
    }
    // The actual stage body is priced by preflight_native_worker_birth BEFORE
    // the Root/channel/analyzer births above. Box has no hidden header; its
    // inline pointer and overlapping construction body belong to peak scratch.
    // This allocation does not create another retirement custodian.
    Ok(Box::new(PreparedBackend {
        negotiated: None,
        data_guard,
        error_guard,
        legacy: legacy_control,
        publication: worker_control,
        client,
        spectrum,
        spectrum_error,
        sample_rate,
        stream_owner,
    }))
}

#[cfg(all(test, feature = "allocation-probe"))]
pub(crate) fn prepare_headless_legacy_test_stage(
    lease: lifecycle::Lease,
    sample_rate: u32,
    reservation: crate::AudioCandidatePublicationReservation,
) -> Result<Box<PreparedBackend>, PreparedDataError> {
    crate::prepared::preflight_native_worker_birth(false, Some(reservation))
        .map_err(PreparedDataError::Constructor)?;
    let (control, callback) =
        prepared::channel_with_factory_reservation(sample_rate, None, Some(reservation))
            .map_err(PreparedDataError::Constructor)?;
    let parent =
        lifecycle::StreamOwner::begin_prepared_custody(lease, control.native_candidate_root());
    // No analyzer constructor was called; parent's real phase is NotRequested.
    // In particular no legacy analyzer receipt is cleared or reclassified.
    install_backend_carriers((parent, control, callback, None, None), sample_rate, false)
}

pub(crate) fn finish_prepared_backend(
    mut prepared: Box<PreparedBackend>,
) -> Result<Opened, String> {
    let negotiated = prepared
        .negotiated
        .take()
        .ok_or("prepared output has no negotiated backend resource")?;
    // A negotiated rate is correlation, never a saved native admission.
    // Refusal drops this SAME parent/carrier payload, not a reconstructed Root.
    validate_expected_rate(Some(prepared.sample_rate), negotiated.config.sample_rate)?;
    let NegotiatedOutput {
        device,
        config,
        sample_format,
    } = negotiated;
    let PreparedBackend {
        negotiated: _,
        data_guard,
        error_guard,
        legacy: legacy_control,
        publication: worker_control,
        client,
        spectrum,
        spectrum_error,
        sample_rate: _,
        mut stream_owner,
    } = *prepared;
    stream_owner.expose_backend();
    let stream = if sample_format == cpal::SampleFormat::F32 {
        build::<f32>(&device, &config, data_guard, error_guard)
    } else if sample_format == cpal::SampleFormat::I16 {
        build::<i16>(&device, &config, data_guard, error_guard)
    } else {
        build::<u16>(&device, &config, data_guard, error_guard)
    }
    .map_err(|error| error.to_string())?;
    stream_owner.set_stream(stream);
    stream_owner.play()?;
    let label = format!(
        "coreaudio · {} hz · {} channels",
        config.sample_rate, config.channels
    );
    Ok(Opened {
        stream: stream_owner,
        legacy: legacy_control,
        publication: worker_control,
        client,
        sample_rate: config.sample_rate,
        label,
        spectrum,
        spectrum_error,
    })
}

fn validate_expected_rate(expected: Option<u32>, actual: u32) -> Result<(), String> {
    if let Some(expected) = expected {
        if expected != actual {
            return Err(format!(
                "default output rate is {actual} hz; project requires {expected} hz"
            ));
        }
    }
    Ok(())
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
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
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let Some(owner) = data_guard.owner_mut() else {
                data.fill(T::from_sample(0.0));
                return;
            };
            owner.run(|callback, status| {
                let start = Instant::now();
                callback.begin_block();
                let mut block: [Frame; 256] = [[0.0; 2]; 256];
                let mut peaks = [0.0_f32; 2];
                let complete = data.len() / channels * channels;
                data[complete..].fill(T::from_sample(0.0));
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
                status.callback_max_ns.fetch_max(elapsed, Relaxed);
                let deadline = (data.len() / channels) as u64 * 1_000_000_000 / u64::from(rate);
                if elapsed > deadline {
                    status.deadline_misses.fetch_add(1, Relaxed);
                }
            });
        },
        move |_| {
            if let Some(owner) = error_guard.owner_mut() {
                owner.report();
            }
        },
        None,
    )
}

fn finish_host_chunk<T: SizedSample + FromSample<f32>>(
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
    let valid = rendered.is_ok() && copy_host_chunk(output, source, channels);
    let _ = callback.finish_spectrum_block(capture_start, valid);
    if !valid {
        output.fill(T::from_sample(0.0));
        source.fill([0.0; 2]);
        callback.report_render_fault(rendered.as_ref().err(), block_start_frame);
    }
    callback.finish_gain_recording_quantum(valid);
    valid
}

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
    let lease = lifecycle::Lease::acquire()?;
    let (control, callback) = prepared::channel(rate).map_err(|e| e.to_string())?;
    let root = control.native_candidate_root();
    let (data, errors, terminals, client, mut publication) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None).unwrap();
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
            sample_rate: rate,
            label: "headless constructor fixture".into(),
            spectrum_error: None,
        },
        crate::worker_fixture::HeadlessBackend::new(data, errors),
    ))
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn headless_reserved_worker_output(
    rate: u32,
    reservation: crate::AudioCandidatePublicationReservation,
) -> Result<(WorkerPreparedOutput, crate::worker_fixture::HeadlessBackend), String> {
    // Same factory and same lifecycle parent; actual constructor NotRequested
    // means no analyzer thread or captured buffer cohort exists in this fixture.
    crate::prepared::preflight_native_worker_birth(false, Some(reservation))
        .map_err(|error| error.to_string())?;
    let lease = lifecycle::Lease::acquire()?;
    let (control, callback) =
        prepared::channel_with_factory_reservation(rate, None, Some(reservation))
            .map_err(|error| error.to_string())?;
    let parent =
        lifecycle::StreamOwner::begin_prepared_custody(lease, control.native_candidate_root());
    let prepared = install_backend_carriers((parent, control, callback, None, None), rate, true)
        .map_err(|error| error.to_string())?;
    let PreparedBackend {
        negotiated,
        data_guard,
        error_guard,
        legacy,
        publication,
        client,
        spectrum,
        spectrum_error,
        sample_rate,
        mut stream_owner,
    } = *prepared;
    if negotiated.is_some() || legacy.is_some() || spectrum.is_some() {
        return Err("headless constructor fields differ".into());
    }
    stream_owner.expose_headless_backend();
    Ok((
        WorkerPreparedOutput {
            _stream: stream_owner,
            publication: publication.ok_or("worker endpoint unavailable")?,
            client,
            sample_rate,
            label: "headless reserved constructor fixture".into(),
            spectrum_error,
        },
        crate::worker_fixture::HeadlessBackend::new(data_guard, error_guard),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        copy_host_chunk, finish_host_chunk, mono_mean, validate_expected_rate,
        validate_output_configuration,
    };
    #[test]
    fn actual_prepared_parent_and_carrier_stage_is_send_without_sync_widening() {
        fn require_send<T: Send>() {}
        // This resolves only while the ACTUAL stage is not Sync. If a later
        // change widens it, both marker implementations apply and compilation
        // fails; no unsafe/negative impl or runtime owner getter is involved.
        trait AmbiguousIfSync<Marker> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSync<()> for T {}
        impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
        let _ = <super::PreparedBackend as AmbiguousIfSync<_>>::check;
        require_send::<super::PreparedBackend>();
        require_send::<super::Opened>();
        require_send::<crate::lifecycle::StreamOwner>();
        assert!(
            super::prepared_open_stage_body_bytes()
                >= std::mem::size_of::<crate::lifecycle::StreamOwner>()
        );
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn prepared_data_stage_both_minus_one_refuse_without_allocating_an_error() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (retained, peak) = crate::prepared::native_worker_birth_bound(true).unwrap();
        for (retained, peak) in [(retained - 1, peak), (retained, peak - 1)] {
            let limits = crate::AudioCandidatePublicationReservation {
                runtime_slots: 3,
                archive_slots: 3,
                member_rows: 128,
                transfer_rows: 128,
                receipt_rows: 3,
                data_rows: 256,
                handle_rows: 256,
                max_retained_data_bytes: retained,
                max_construction_peak_data_bytes: peak,
            };
            let lease = crate::lifecycle::Lease::acquire().unwrap();
            let mut result = None;
            let measured = allocation_counter::measure(|| {
                result = Some(super::prepare_backend_data(
                    lease,
                    48000,
                    true,
                    Some(limits),
                ));
            });
            assert!(matches!(
                result,
                Some(Err(super::PreparedDataError::Constructor(_)))
            ));
            assert_eq!(measured.count_total, 0);
            assert_eq!(measured.bytes_total, 0);
            assert_eq!(measured.count_current, 0);
            assert_eq!(measured.bytes_current, 0);
        }
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn prepared_backend_full_handoff_returns_same_parent_and_retires_without_backend() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (retained, peak) = crate::prepared::native_worker_birth_bound(true).unwrap();
        let limits = crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 128,
            transfer_rows: 128,
            receipt_rows: 3,
            data_rows: 256,
            handle_rows: 256,
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        };
        let prepared = super::prepare_backend_data(
            crate::lifecycle::Lease::acquire().unwrap(),
            48000,
            true,
            Some(limits),
        )
        .unwrap();
        let identity = prepared.client.as_ref().unwrap().output_identity().unwrap();
        // This stage has its REAL Root/Reaper and both carriers; no backend
        // request or supplied receipt is involved. A full handoff must return
        // that exact payload, never rebuild a parent or silently discard it.
        let (sender, _receiver) = std::sync::mpsc::sync_channel(0);
        let refused = match sender.try_send(prepared) {
            Err(std::sync::mpsc::TrySendError::Full(prepared)) => prepared,
            _ => panic!("zero-capacity handoff must retain the prepared payload"),
        };
        assert_eq!(
            refused.client.as_ref().unwrap().output_identity(),
            Some(identity)
        );
        assert_eq!(refused.sample_rate, 48000);
        assert!(refused.publication.is_some());
        assert!(refused.legacy.is_none());
        drop(refused);
        // The same leased parent must complete its actual analyzer join and
        // terminal cleanup before another lease can be obtained. No scalar
        // caller retirement receipt can release OCCUPIED.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(next) = crate::lifecycle::Lease::acquire() {
                drop(next);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "original parent did not retire"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn negotiated_configuration_refuses_before_prepared_channel_construction() {
        assert!(validate_output_configuration(0, cpal::SampleFormat::F32).is_err());
        assert!(validate_output_configuration(2, cpal::SampleFormat::F64).is_err());
        for format in [
            cpal::SampleFormat::F32,
            cpal::SampleFormat::I16,
            cpal::SampleFormat::U16,
        ] {
            assert!(validate_output_configuration(1, format).is_ok());
            assert!(validate_output_configuration(2, format).is_ok());
        }
    }
    use crate::gain_recording::{AbortReason, Receipt, Target};
    use crate::prepared;
    use cpal::{FromSample, SizedSample};
    use hex_engine::{Audio, PlaybackCommand, PreparedRevision, playback::Playback};
    use std::{
        collections::BTreeMap,
        sync::{Arc, atomic::Ordering::Acquire},
        time::Instant,
    };

    #[test]
    fn recovery_rate_mismatch_is_rejected_before_stream_construction() {
        assert!(validate_expected_rate(Some(48_000), 44_100).is_err());
        assert!(validate_expected_rate(Some(48_000), 48_000).is_ok());
        assert!(validate_expected_rate(None, 44_100).is_ok());
    }

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
                    assert_eq!(signed[sample], cpal::Sample::from_sample(expected));
                    assert_eq!(unsigned[sample], cpal::Sample::from_sample(expected));
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
        let revision = Arc::new(
            PreparedRevision::prepare_for_gain_recording(
                editor.project(),
                &BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]; 4096]).unwrap())]),
                hex_engine::SharedSineTable::new_on_control(),
                None,
                track,
            )
            .unwrap(),
        );
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
