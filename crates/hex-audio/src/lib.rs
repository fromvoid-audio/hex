//! # Purpose
//! Own native input discovery and capture, output lifecycle, bounded prepared-graph publication, callback
//! command delivery and off-callback reclamation. Owns copied isolated-processor endpoints and their
//! joining control worker; plugin ABI loading and native instance admission remain outside this crate.
//!
//! # Dependencies
//! Uses hex-engine for musical rendering, hex-stream for verified pages, CPAL for device I/O and
//! rtrb for bounded ownership queues. hex-plugin-host supplies the copied transport
//! and source-bound native client; native ABI remains in its external helper.
//! qos-threads provides a safe macOS scheduling request for the joining native
//! transport worker only; it grants no real-time or physical deadline guarantee.
//! hex-plugin-scan supplies only the checked instance-build provenance accepted by that client.
//! hex-lattice supplies the borrowed foreign node/factory contracts; hex-plugin-state
//! and hex-lattice-contract validate the exact native effect configuration.
//!
//! # Does not own
//! Does not edit projects, decode media on callback, own GPUI, or implement device DSP. Stop must
//! remain effective under queue pressure; final graph, source and endpoint destruction belongs on
//! control.
pub mod audio_diagnostics;
pub mod cpu_meter;
mod device;
pub mod input_capture;
pub mod input_devices;
mod open_outcome;
mod output_route;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod plugin_effects;
pub mod plugin_worker;
mod render_fault;
pub mod sample_preview;
pub mod sample_recording;
#[cfg(target_os = "macos")]
pub use device::PluginOutputObservation;
pub use device::{PreparedOutput, WorkerOutputProfile, WorkerPreparedOutput};
pub use lifecycle::{
    AudioLifecycleFailure, RecoveryOpen, RecoveryOpenError, WorkerRecoveryOpen,
    audio_lifecycle_failure, request_initial_open, request_recovery_open, request_worker_open,
    request_worker_open_with_profile, request_worker_open_with_reliable_action_outcomes,
};
pub use open_outcome::{OutputOpenError, RateMismatch};
pub use output_route::{OutputRoute, OutputRouteWatch, ROUTE_SAMPLE_INTERVAL};
pub use render_fault::RenderFaultSnapshot;
mod candidate_catalog;
mod candidate_custody;
// Per-callback-thread observation for the real readiness boundary regression.
// Initialized before measurement; it neither allocates nor changes admission.
#[cfg(all(test, feature = "allocation-probe", feature = "stream-test-support"))]
pub(crate) mod candidate_window_probe {
    use std::cell::Cell;
    thread_local! {
        static WINDOWS: Cell<Option<(usize, usize, usize)>> = const { Cell::new(None) };
        static DOMAINS: Cell<Option<(u8, usize)>> = const { Cell::new(None) };
    }
    pub(crate) fn begin() {
        WINDOWS.with(|value| value.set(Some((0, 0, usize::MAX))));
        DOMAINS.with(|value| value.set(Some((0, usize::MAX))));
    }
    pub(crate) fn attempt(frames: usize) {
        WINDOWS.with(|value| {
            if let Some((failed, next, smallest)) = value.get()
                && failed != 0
            {
                value.set(Some((
                    failed,
                    if next == 0 { frames } else { next },
                    smallest.min(frames),
                )));
            }
        });
    }
    pub(crate) fn accepted(slot: usize) {
        DOMAINS.with(|value| {
            if let Some((mask, usize::MAX)) = value.get() {
                value.set(Some((mask | (1 << slot), usize::MAX)));
            }
        });
    }
    pub(crate) fn capacity(frames: usize, slot: usize) {
        WINDOWS.with(|value| {
            if let Some((0, next, smallest)) = value.get() {
                value.set(Some((frames, next, smallest)));
            }
        });
        DOMAINS.with(|value| {
            if let Some((mask, usize::MAX)) = value.get() {
                value.set(Some((mask, slot)));
            }
        });
    }
    pub(crate) fn domains() -> Option<(u8, usize)> {
        DOMAINS.with(|value| value.get())
    }
    pub(crate) fn finish() -> Option<(usize, usize, usize)> {
        DOMAINS.with(|value| value.set(None));
        WINDOWS.with(|value| value.replace(None))
    }
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use candidate_custody::{
    NativeSourceDataReservation, NativeSourcePreparationProgress, NativeSourcePreparationReturn,
};
pub mod foreign_parameter_delivery;
pub mod gain_recording;
mod lifecycle;
mod live_delivery;
pub mod midi_recording;
mod native_callback_owner;
pub mod native_parameter_input;
pub use candidate_catalog::CandidateDomain;
pub use candidate_custody::{
    AudioCandidateCancellationResult, AudioCandidateCustodyReport,
    AudioCandidateDomainPreparationFailure, AudioCandidateDomainPreparationProgress,
    AudioCandidateDomainRefusal, AudioCandidateDomainReservation, AudioCandidateDomainStartError,
    AudioCandidateDomainToken, AudioCandidateDomainUsage, AudioCandidateEligibleStageRefusal,
    AudioCandidatePreparationProgress, AudioCandidatePreparationRefusal,
    AudioCandidatePreparationReservation, AudioCandidatePreparationToken,
    AudioCandidatePublicationReservation, AudioCandidateRecovery, AudioCandidateReturnKind,
    AudioCandidateStageRefusal, AudioCandidateTicket,
};
pub mod processor_audition;
pub mod spectrum;
pub mod spectrum_target;
pub mod spectrum_worker;
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use hex_engine::live::{Command, LiveEngine, Telemetry};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Instant,
};

pub struct AudioOutput {
    _stream: lifecycle::StreamOwner,
    pub commands: Producer<Command>,
    pub telemetry: Arc<Telemetry>,
    stop: Arc<AtomicBool>,
    pub label: String,
}
impl AudioOutput {
    pub fn open() -> Result<Self, String> {
        let lease = lifecycle::Lease::acquire()?;
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("No default audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        if config.channels == 0 {
            return Err("Output device has no channels".into());
        }
        let telemetry = Arc::new(Telemetry::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, consumer) = RingBuffer::new(32);
        let engine = LiveEngine::demo(config.sample_rate).map_err(|e| e.to_string())?;
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(
                &device,
                &config,
                engine,
                consumer,
                telemetry.clone(),
                stop.clone(),
            ),
            cpal::SampleFormat::I16 => build::<i16>(
                &device,
                &config,
                engine,
                consumer,
                telemetry.clone(),
                stop.clone(),
            ),
            cpal::SampleFormat::U16 => build::<u16>(
                &device,
                &config,
                engine,
                consumer,
                telemetry.clone(),
                stop.clone(),
            ),
            other => return Err(format!("Unsupported output format: {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        if let Err(error) = stream.play() {
            drop(lifecycle::StreamOwner::new(stream, lease));
            return Err(error.to_string());
        }
        Ok(Self {
            _stream: lifecycle::StreamOwner::new(stream, lease),
            commands,
            telemetry,
            stop,
            label: format!(
                "CoreAudio · {} Hz · {} channels",
                config.sample_rate, config.channels
            ),
        })
    }
    /// Stop wins over every command queued before the next callback; never queue-dependent.
    pub fn stop(&self) {
        self.stop.store(true, Relaxed);
    }
}
fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine: LiveEngine,
    mut commands: Consumer<Command>,
    telemetry: Arc<Telemetry>,
    stop: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let errors = telemetry.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let start = Instant::now();
            let stopping = stop.swap(false, Relaxed);
            apply_controls(&mut engine, &mut commands, stopping);
            engine.begin_block();
            let frames = data.len() / channels;
            for frame in data.chunks_mut(channels) {
                let stereo = engine.next_frame();
                for (channel, sample) in frame.iter_mut().enumerate() {
                    let value = if channels == 1 {
                        (stereo[0] + stereo[1]) * 0.5
                    } else if channel < 2 {
                        stereo[channel]
                    } else {
                        0.0
                    };
                    *sample = T::from_sample(value);
                }
            }
            engine.publish(&telemetry);
            let elapsed = start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            telemetry.callback_max_ns.fetch_max(elapsed, Relaxed);
            let deadline = frames as u64 * 1_000_000_000 / u64::from(rate);
            if elapsed > deadline {
                telemetry.deadline_misses.fetch_add(1, Relaxed);
            }
        },
        move |_| {
            errors.device_fault.store(true, Relaxed);
        },
        None,
    )
}

fn apply_controls(engine: &mut LiveEngine, commands: &mut Consumer<Command>, stopping: bool) {
    for _ in 0..32 {
        let Ok(command) = commands.pop() else {
            break;
        };
        if !stopping
            || !matches!(
                command,
                Command::Play | Command::Pause | Command::Stop | Command::SeekStart
            )
        {
            engine.apply(command);
        }
    }
    if stopping {
        engine.apply(Command::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_wins_when_command_queue_is_full() {
        let (mut producer, mut consumer) = RingBuffer::new(32);
        for _ in 0..32 {
            producer.push(Command::Play).unwrap();
        }
        assert!(producer.push(Command::Play).is_err());
        let mut engine = LiveEngine::demo(8_000).unwrap();
        apply_controls(&mut engine, &mut consumer, true);
        let telemetry = Telemetry::default();
        engine.publish(&telemetry);
        assert!(!telemetry.playing.load(Relaxed));
        assert_eq!(telemetry.beat(), 0.0);
        assert!(consumer.pop().is_err());
        producer.push(Command::Play).unwrap();
        apply_controls(&mut engine, &mut consumer, false);
        engine.publish(&telemetry);
        assert!(telemetry.playing.load(Relaxed));
    }
}

pub mod prepared;

/// Instrumented constructor-only fixture; no physical device/CPAL evidence.
#[cfg(feature = "allocation-probe")]
pub mod worker_fixture;

#[cfg(all(test, feature = "allocation-probe"))]
mod auxiliary_allocation;
#[cfg(all(test, feature = "allocation-probe"))]
mod eq_allocation;
#[cfg(all(test, feature = "allocation-probe"))]
mod owned_effect_allocation;
#[cfg(all(test, feature = "allocation-probe"))]
mod resource_borrow_allocation;
#[cfg(all(test, feature = "allocation-probe"))]
mod transition_admission_allocation;
