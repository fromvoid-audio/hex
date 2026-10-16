//! Device adapter for the canonical compiled playback handoff.
use crate::spectrum_worker::{
    SpectrumReader, SpectrumStamp, SpectrumTargetId, spawn_spectrum_worker,
};
use crate::{lifecycle, prepared};
use cpal::{
    FromSample, Sample, SizedSample,
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
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
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
                let block_start_frame = status.position.load(Relaxed);
                let rendered = callback.render(&mut block[..frames]);
                finish_host_chunk(
                    &mut callback,
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
        },
        move |_| {
            errors.device_fault.store(true, Relaxed);
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
    callback.finish_typing_output(valid);
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

pub(crate) fn host_samples_are_finite<T: SizedSample + FromSample<f32>>(
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

#[cfg(test)]
mod tests {
    use super::{copy_host_chunk, finish_host_chunk, mono_mean, validate_expected_rate};
    use crate::prepared;
    use cpal::{FromSample, SizedSample};
    use hex_engine::{
        PlaybackCommand,
        live_input::{PreparedTypingArm, TypingTarget},
        typing_protocol::{Action, Token},
    };
    use std::{collections::BTreeMap, sync::Arc, time::Instant};

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

    fn typing_callback() -> (prepared::Control, prepared::Callback, TypingTarget) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "host conversion".into(),
            })
            .unwrap()
        else {
            panic!("expected track");
        };
        let device = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .unwrap();
        let target = TypingTarget {
            project_generation: 0,
            project_revision: 1,
            track,
            device,
        };
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback =
            hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
        playback
            .prepare_typing_input_with_bridge(
                PreparedTypingArm::prepare(editor.project(), target).unwrap(),
                1,
                prepared::input_bridge_storage_bytes().unwrap(),
            )
            .unwrap();
        let (mut control, callback) = prepared::channel(48_000).unwrap();
        control.publish(playback).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
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

    fn host_copy_ack_regression<T: SizedSample + FromSample<f32> + std::fmt::Debug>(
        render_failed: bool,
    ) where
        f32: FromSample<T>,
    {
        for channels in [1, 2, 4] {
            let (mut control, mut callback, target) = typing_callback();
            let action = |serial, key| Action::On {
                owner: target.owner().unwrap(),
                token: Token { epoch: 1, serial },
                key,
                pitch: 60 + key,
                velocity: 100,
            };
            let success_ticket = control.typing_with_ticket(action(1, 0)).unwrap();
            callback.begin_block();
            let mut source = [[0.0; 2]; 256];
            let rendered = callback.render(&mut source);
            assert!(rendered.is_ok());
            assert!(control.status.typing.snapshot().unwrap().applications[0].is_none());
            // Inject finite boundary values after real engine rendering: the
            // actual host path must accept their representable mono mean.
            source[0] = [f32::MAX; 2];
            let count = channels * source.len();
            let mut output = vec![T::from_sample(0.0); count * 2];
            assert!(checked_host_finish(
                &mut callback,
                &mut output[..count],
                &mut source,
                channels,
                &rendered,
                0,
                Instant::now()
            ));
            assert_eq!(output[0], T::from_sample(f32::MAX));
            let success = control.status.typing.snapshot().unwrap().applications[0].unwrap();
            assert_eq!(success.ticket, success_ticket);
            assert_eq!(success.result, Ok(()));
            let prefix = output[..count].to_vec();

            let failed_ticket = control.typing_with_ticket(action(2, 1)).unwrap();
            callback.begin_block();
            let rendered = callback.render(&mut source);
            assert!(rendered.is_ok());
            assert!(control.status.typing.snapshot().unwrap().applications[1].is_none());
            let rendered = if render_failed {
                // Finite PCM must not turn a refused render into output success.
                Err(hex_engine::Invalid(
                    "host conversion regression render refusal",
                ))
            } else {
                source[255][1] = f32::NAN;
                rendered
            };
            assert!(!checked_host_finish(
                &mut callback,
                &mut output[count..],
                &mut source,
                channels,
                &rendered,
                256,
                Instant::now()
            ));
            let rejected = control.status.typing.snapshot().unwrap();
            assert!(!rejected.armed);
            assert_eq!(rejected.cut_through_epoch, 1);
            assert_eq!(rejected.applications[0].unwrap().ticket, success_ticket);
            assert!(rejected.applications[1].is_none());
            assert!(
                !rejected
                    .applications
                    .iter()
                    .flatten()
                    .any(|application| application.ticket == failed_ticket
                        && application.result.is_ok())
            );
            assert_eq!(output[..count], prefix);
            assert!(
                output[count..]
                    .iter()
                    .all(|value| *value == T::from_sample(0.0))
            );

            // A subsequent successful/silent copy must not resurrect the failed
            // onset or its application ACK, nor erase the committed prefix.
            callback.begin_block();
            let rendered = callback.render(&mut source);
            assert!(rendered.is_ok());
            assert!(source.iter().all(|frame| *frame == [0.0; 2]));
            assert!(checked_host_finish(
                &mut callback,
                &mut output[count..],
                &mut source,
                channels,
                &rendered,
                512,
                Instant::now()
            ));
            let after = control.status.typing.snapshot().unwrap();
            assert!(after.applications[1].is_none());
            assert_eq!(after.applications[0].unwrap().ticket, success_ticket);
            assert_eq!(output[..count], prefix);
        }
    }

    #[test]
    fn actual_host_copy_commits_extrema_but_failed_chunk_never_publishes_or_resurrects_ack() {
        for render_failed in [false, true] {
            host_copy_ack_regression::<f32>(render_failed);
            host_copy_ack_regression::<i16>(render_failed);
            host_copy_ack_regression::<u16>(render_failed);
        }
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
