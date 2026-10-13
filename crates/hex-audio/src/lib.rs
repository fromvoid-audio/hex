mod device;
mod render_fault;
pub use device::PreparedOutput;
pub use lifecycle::{RecoveryOpen, RecoveryOpenError, request_initial_open, request_recovery_open};
pub use render_fault::RenderFaultSnapshot;
mod lifecycle;
pub mod processor_audition;
pub mod spectrum;
pub mod spectrum_target;
pub mod spectrum_worker;
pub mod typing;
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
