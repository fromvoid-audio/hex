use super::*;
use hex_lattice::BorrowedDeviceResources;
use hex_lattice_contract::DeviceAssetKey;
use std::f64::consts::TAU;

fn slot(id: u32) -> DeviceAssetSlot {
    DeviceAssetSlot::new(id).unwrap()
}

fn identity(asset: u64, content_tag: u8, derivation_version: u32) -> DeviceResourceIdentity {
    DeviceResourceIdentity {
        asset: DeviceAssetKey::new(asset).unwrap(),
        content: [content_tag; 32],
        derivation_version,
        derivation: [content_tag.wrapping_add(1); 32],
    }
}

fn sine_cycle(frame_size: usize, harmonic: usize) -> Vec<f32> {
    (0..frame_size)
        .map(|i| (TAU * (harmonic as f64) * (i as f64) / (frame_size as f64)).sin() as f32)
        .collect()
}

fn dft_magnitude(samples: &[f32], freq_hz: f64, sample_rate: u32) -> f64 {
    let n = samples.len();
    let mut re = 0.0_f64;
    let mut im = 0.0_f64;
    let w = TAU * freq_hz / f64::from(sample_rate);
    for (i, &x) in samples.iter().enumerate() {
        let win = 0.5 * (1.0 - (TAU * i as f64 / n as f64).cos());
        let v = f64::from(x) * win;
        let angle = w * i as f64;
        re += v * angle.cos();
        im -= v * angle.sin();
    }
    (re * re + im * im).sqrt() * 4.0 / n as f64
}

#[test]
fn interpolation_oracle_wraps_within_each_cycle_and_preserves_stereo() {
    // Two 64-sample tables: table 0 has L=+sine(1), R=+0.5; table 1 has L=-sine(1), R=-0.5.
    let frame_size = 64;
    let mut pcm = Vec::with_capacity(frame_size * 2);
    for i in 0..frame_size {
        let s = (TAU * (i as f64) / (frame_size as f64)).sin() as f32;
        pcm.push([s, 0.5]);
    }
    for i in 0..frame_size {
        let s = (TAU * (i as f64) / (frame_size as f64)).sin() as f32;
        pcm.push([-s, -0.5]);
    }
    let view = DeviceResourceView {
        slot: slot(1),
        identity: identity(10, 1, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Stereo(&pcm),
    };
    let prepared =
        PreparedWavetableSource::prepare_on_control(view, frame_size, WavetableLimits::DEFAULT)
            .unwrap();
    assert_eq!(prepared.layout().frame_size(), 64);
    assert_eq!(prepared.layout().table_count(), 2);
    assert_eq!(prepared.layout().channels(), WavetableChannels::Stereo);

    let resources = BorrowedDeviceResources(std::slice::from_ref(&view));

    // At morph = 0.0, right channel must remain +0.5 even across frame boundary 63 -> 0 (no bleed from table 1!).
    for mode in [
        WavetableInterpolation::Linear,
        WavetableInterpolation::CubicHermite,
    ] {
        let mut osc = prepared
            .oscillator(WavetableOscillatorSettings {
                output_sample_rate: 48_000,
                frequency_hz: 750.0, // 64 samples/cycle -> exact wrap every 64 frames
                table_position: 0.0,
                initial_phase: 63.25 / 64.0,
                gain: 1.0,
                interpolation: mode,
            })
            .unwrap();
        let mut out = [[0.0; 2]; 16];
        osc.render_block(&resources, &mut out).unwrap();
        for frame in &out {
            assert!(
                (frame[1] - 0.5).abs() < 1e-6,
                "table 0 right channel bled across cycle boundary: {frame:?}"
            );
        }
    }

    // At morph = 0.5, L and R cancel to 0.0 identically.
    let mut mid = prepared
        .oscillator(WavetableOscillatorSettings {
            output_sample_rate: 48_000,
            frequency_hz: 440.0,
            table_position: 0.5,
            initial_phase: 0.123,
            gain: 1.0,
            interpolation: WavetableInterpolation::CubicHermite,
        })
        .unwrap();
    let mut out = [[0.0; 2]; 64];
    mid.render_block(&resources, &mut out).unwrap();
    for frame in &out {
        assert!(frame[0].abs() < 1e-5 && frame[1].abs() < 1e-5);
    }

    // At morph = 1.0, right channel is exact -0.5.
    mid.set_table_position(1.0).unwrap();
    mid.render_block(&resources, &mut out).unwrap();
    for frame in &out {
        assert!((frame[1] + 0.5).abs() < 1e-6);
    }
}

#[test]
fn pitch_spectral_morph_and_partition_oracles() {
    let frame_size = 2_048;
    // Table 0: pure fundamental; Table 1: pure 3rd harmonic.
    let mut pcm = sine_cycle(frame_size, 1);
    pcm.extend(sine_cycle(frame_size, 3));
    let view = DeviceResourceView {
        slot: slot(1),
        identity: identity(42, 7, 1),
        sample_rate: 44_100, // Deliberately differs from 48 kHz and 96 kHz output rates!
        pcm: DeviceResourcePcm::Mono(&pcm),
    };
    let prepared =
        PreparedWavetableSource::prepare_on_control(view, frame_size, WavetableLimits::DEFAULT)
            .unwrap();
    let resources = BorrowedDeviceResources(std::slice::from_ref(&view));

    // Pitch & spectral purity oracle across output rates (independent of 44.1 kHz asset rate).
    for output_rate in [44_100, 48_000, 96_000] {
        for freq in [220.0_f64, 440.0, 880.0] {
            let mut osc = prepared
                .oscillator(WavetableOscillatorSettings {
                    output_sample_rate: output_rate,
                    frequency_hz: freq,
                    table_position: 0.0,
                    initial_phase: 0.0,
                    gain: 1.0,
                    interpolation: WavetableInterpolation::CubicHermite,
                })
                .unwrap();
            let mut out = vec![[0.0_f32; 2]; 4096];
            osc.render_block(&resources, &mut out).unwrap();
            let mono: Vec<f32> = out.iter().map(|f| f[0]).collect();
            let h1 = dft_magnitude(&mono, freq, output_rate);
            let h3 = dft_magnitude(&mono, freq * 3.0, output_rate);
            let spurious = dft_magnitude(&mono, freq * 2.5, output_rate);
            assert!(
                (h1 - 1.0).abs() < 0.02,
                "fs={output_rate} f={freq}: expected h1~1.0, got {h1}"
            );
            assert!(
                20.0 * (spurious.max(1e-9) / h1).log10() < -70.0,
                "fs={output_rate} f={freq}: cubic interpolation noise floor too high"
            );
            assert!(h3 < 1e-3, "table 0 must not leak 3rd harmonic: {h3}");
        }
    }

    // Spectral morph oracle: pos=0.5 blends 0.5 * h1 + 0.5 * h3.
    let mut osc = prepared
        .oscillator(WavetableOscillatorSettings {
            output_sample_rate: 48_000,
            frequency_hz: 440.0,
            table_position: 0.5,
            initial_phase: 0.0,
            gain: 1.0,
            interpolation: WavetableInterpolation::CubicHermite,
        })
        .unwrap();
    let mut whole = vec![[0.0_f32; 2]; 4096];
    osc.render_block(&resources, &mut whole).unwrap();
    let mono: Vec<f32> = whole.iter().map(|f| f[0]).collect();
    assert!((dft_magnitude(&mono, 440.0, 48_000) - 0.5).abs() < 0.02);
    assert!((dft_magnitude(&mono, 1320.0, 48_000) - 0.5).abs() < 0.02);

    // Block partition & reset reproducibility oracle: bit-identical across irregular splits.
    osc.reset();
    let mut parts = vec![[0.0_f32; 2]; 4096];
    let mut cursor = 0;
    for chunk in [1, 7, 13, 64, 199, 512, 1023, 2277] {
        osc.render_block(&resources, &mut parts[cursor..cursor + chunk])
            .unwrap();
        cursor += chunk;
    }
    assert_eq!(whole, parts);
}

#[test]
fn malformed_oversized_missing_changed_and_stale_assets_refuse_safely() {
    let valid_pcm = sine_cycle(256, 1);
    let valid_view = DeviceResourceView {
        slot: slot(1),
        identity: identity(1, 5, 1),
        sample_rate: 48_000,
        pcm: DeviceResourcePcm::Mono(&valid_pcm),
    };

    // Malformed / invalid geometry & metadata refusals.
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(
            DeviceResourceView {
                identity: identity(1, 5, 0),
                ..valid_view
            },
            256,
            WavetableLimits::DEFAULT
        ),
        Err(WavetableError::InvalidIdentity)
    );
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(
            DeviceResourceView {
                sample_rate: 0,
                ..valid_view
            },
            256,
            WavetableLimits::DEFAULT
        ),
        Err(WavetableError::InvalidSampleRate)
    );
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(valid_view, 192, WavetableLimits::DEFAULT),
        Err(WavetableError::InvalidFrameSize)
    );
    let unaligned = vec![0.0_f32; 260];
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(
            DeviceResourceView {
                pcm: DeviceResourcePcm::Mono(&unaligned),
                ..valid_view
            },
            256,
            WavetableLimits::DEFAULT
        ),
        Err(WavetableError::UnalignedFrameCount)
    );
    let mut non_finite = valid_pcm.clone();
    non_finite[17] = f32::NAN;
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(
            DeviceResourceView {
                pcm: DeviceResourcePcm::Mono(&non_finite),
                ..valid_view
            },
            256,
            WavetableLimits::DEFAULT
        ),
        Err(WavetableError::NonFiniteSample)
    );
    let tight_limits = WavetableLimits {
        max_frame_size: 256,
        max_table_count: 1,
        max_total_frames: 256,
        max_bytes: 256 * 4,
        max_work_frames_per_block: 16,
    };
    let two_tables = vec![0.0_f32; 512];
    assert_eq!(
        PreparedWavetableSource::prepare_on_control(
            DeviceResourceView {
                pcm: DeviceResourcePcm::Mono(&two_tables),
                ..valid_view
            },
            256,
            tight_limits
        ),
        Err(WavetableError::TableCountOutOfBounds)
    );

    // Work limit & stale/changed/missing resource refusals preserve oscillator state and silence output.
    let prepared =
        PreparedWavetableSource::prepare_on_control(valid_view, 256, tight_limits).unwrap();
    let mut osc = prepared
        .oscillator(WavetableOscillatorSettings {
            output_sample_rate: 48_000,
            frequency_hz: 440.0,
            table_position: 0.0,
            initial_phase: 0.25,
            gain: 1.0,
            interpolation: WavetableInterpolation::Linear,
        })
        .unwrap();
    let phase_before = osc.phase();
    let mut too_big = [[1.0; 2]; 17];
    assert_eq!(
        osc.render_block(
            &BorrowedDeviceResources(std::slice::from_ref(&valid_view)),
            &mut too_big
        ),
        Err(WavetableError::WorkLimitExceeded)
    );
    assert_eq!(osc.phase(), phase_before);

    // Missing resource -> SourceUnavailable, exact silence, phase unchanged.
    let mut out = [[1.0; 2]; 8];
    assert_eq!(
        osc.render_block(&hex_lattice::EMPTY_DEVICE_RESOURCES, &mut out),
        Err(WavetableError::SourceUnavailable)
    );
    assert!(out.iter().flatten().all(|&s| s == 0.0));
    assert_eq!(osc.phase(), phase_before);

    // Changed content digest at same slot -> SourceUnavailable, exact silence, phase unchanged.
    let changed_view = DeviceResourceView {
        identity: identity(1, 99, 1),
        ..valid_view
    };
    out = [[1.0; 2]; 8];
    assert_eq!(
        osc.render_block(
            &BorrowedDeviceResources(std::slice::from_ref(&changed_view)),
            &mut out
        ),
        Err(WavetableError::SourceUnavailable)
    );
    assert!(out.iter().flatten().all(|&s| s == 0.0));
    assert_eq!(osc.phase(), phase_before);
}
