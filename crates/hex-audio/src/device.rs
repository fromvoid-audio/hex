//! Device adapter for the canonical compiled playback handoff.
use crate::spectrum_worker::{
    SpectrumReader, SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker,
};
use crate::{lifecycle, prepared};
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use hex_engine::Frame;
use std::{
    sync::{Arc, atomic::Ordering::Relaxed},
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
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no default audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        validate_expected_rate(expected_sample_rate, config.sample_rate)?;
        if config.channels == 0 {
            return Err("output device has no channels".into());
        }
        let sample_format = supported.sample_format();
        if !matches!(
            sample_format,
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err(format!("unsupported output format: {sample_format:?}"));
        }

        let (capture, spectrum, spectrum_reaper, spectrum_error) = if (8_000..=192_000)
            .contains(&config.sample_rate)
        {
            let initial = SpectrumStamp {
                target: SpectrumTargetId(0),
                generation: 0,
                sample_rate_hz: config.sample_rate,
                applied_revision: 0,
            };
            match spawn_spectrum_worker(initial) {
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
        let channel_result = match capture {
            Some(capture) => prepared::channel_with_spectrum(config.sample_rate, Some(capture)),
            None => prepared::channel(config.sample_rate),
        };
        let (control, callback) = match channel_result {
            Ok(channel) => channel,
            Err(error) => {
                if let Some(reaper) = spectrum_reaper {
                    let _ = reaper.shutdown_and_join();
                }
                return Err(error.to_string());
            }
        };
        let status = control.status.clone();
        let stream = if sample_format == cpal::SampleFormat::F32 {
            build::<f32>(&device, &config, callback, status).map_err(|error| error.to_string())
        } else if sample_format == cpal::SampleFormat::I16 {
            build::<i16>(&device, &config, callback, status).map_err(|error| error.to_string())
        } else if sample_format == cpal::SampleFormat::U16 {
            build::<u16>(&device, &config, callback, status).map_err(|error| error.to_string())
        } else {
            Err(format!("unsupported output format: {sample_format:?}"))
        };
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                drop(control);
                if let Some(reaper) = spectrum_reaper {
                    let _ = reaper.shutdown_and_join();
                }
                return Err(error);
            }
        };
        if let Err(error) = stream.play() {
            drop(lifecycle::StreamOwner::with_spectrum(
                stream,
                lease,
                spectrum_reaper,
            ));
            return Err(error.to_string());
        }
        Ok(Self {
            _stream: lifecycle::StreamOwner::with_spectrum(stream, lease, spectrum_reaper),
            control,
            sample_rate: config.sample_rate,
            label: format!(
                "coreaudio · {} hz · {} channels",
                config.sample_rate, config.channels
            ),
            spectrum,
            spectrum_error,
        })
    }
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
    mut callback: prepared::Callback,
    status: Arc<prepared::Status>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let errors = status.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let start = Instant::now();
            callback.begin_block();
            let mut block: [Frame; 256] = [[0.0; 2]; 256];
            let mut peaks = [0.0_f32; 2];
            let complete = data.len() / channels * channels;
            data[complete..].fill(T::from_sample(0.0));
            for chunk in data[..complete].chunks_mut(channels * block.len()) {
                let frames = chunk.len() / channels;
                let render_valid = callback.render(&mut block[..frames]).is_ok()
                    && block[..frames]
                        .iter()
                        .flatten()
                        .all(|sample| sample.is_finite());
                let _ = callback.finish_spectrum_block(start, render_valid);
                if !render_valid {
                    block[..frames].fill([0.0; 2]);
                    status.render_fault.store(true, Relaxed);
                    callback.stop_after_fault();
                }
                for (frame, stereo) in chunk.chunks_mut(channels).zip(&block[..frames]) {
                    for channel in 0..2 {
                        peaks[channel] = peaks[channel].max(stereo[channel].abs());
                    }
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
            }
            status.peak_left.store(peaks[0].to_bits(), Relaxed);
            status.peak_right.store(peaks[1].to_bits(), Relaxed);
            let elapsed = start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            status.callback_max_ns.fetch_max(elapsed, Relaxed);
            let deadline = (data.len() / channels) as u64 * 1_000_000_000 / u64::from(rate);
            if elapsed > deadline {
                status.deadline_misses.fetch_add(1, Relaxed);
            }
        },
        move |_| {
            errors.device_fault.store(true, Relaxed);
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::validate_expected_rate;

    #[test]
    fn recovery_rate_mismatch_is_rejected_before_stream_construction() {
        assert!(validate_expected_rate(Some(48_000), 44_100).is_err());
        assert!(validate_expected_rate(Some(48_000), 48_000).is_ok());
        assert!(validate_expected_rate(None, 44_100).is_ok());
    }
}
