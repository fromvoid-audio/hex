use super::*;
use hex_lattice::{BorrowedDeviceResources, EMPTY_DEVICE_RESOURCES};
use hex_lattice_contract::DeviceAssetKey;
use std::f64::consts::TAU;

fn slot(id: u32) -> DeviceAssetSlot {
    DeviceAssetSlot::new(id).unwrap()
}

fn identity(asset: u64, tag: u8) -> DeviceResourceIdentity {
    DeviceResourceIdentity {
        asset: DeviceAssetKey::new(asset).unwrap(),
        content: [tag; 32],
        derivation_version: 1,
        derivation: [tag.wrapping_add(1); 32],
    }
}

fn constant_view<'a>(
    slot_id: u32,
    asset: u64,
    tag: u8,
    sample_rate: u32,
    pcm: &'a [[f32; 2]],
) -> DeviceResourceView<'a> {
    DeviceResourceView {
        slot: slot(slot_id),
        identity: identity(asset, tag),
        sample_rate,
        pcm: DeviceResourcePcm::Stereo(pcm),
    }
}

fn sine_pcm(frames: usize, freq_hz: f64, sample_rate: u32) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|i| {
            let s = (TAU * freq_hz * (i as f64) / f64::from(sample_rate)).sin() as f32;
            [s, s]
        })
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
fn key_boundaries_missing_gaps_and_all_overlap_policies_are_deterministic() {
    let pcm_a = [[0.25_f32, 0.25]; 256];
    let pcm_b = [[0.50_f32, 0.50]; 256];
    let views = [
        constant_view(1, 10, 1, 48_000, &pcm_a),
        constant_view(2, 20, 2, 48_000, &pcm_b),
    ];
    let resources = BorrowedDeviceResources(&views);
    let tuning = TuningTable::default();
    // Zone 0: 36..=60 (root 48). Zone 1: 55..=84 (root 72). Overlap: 55..=60. Gaps: 0..36, 85..=127.
    let specs = [
        KeyZoneSpec {
            slot: slot(1),
            low_key: 36,
            high_key: 60,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 48,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
        KeyZoneSpec {
            slot: slot(2),
            low_key: 55,
            high_key: 84,
            low_velocity: 1,
            high_velocity: 127,
            root_key: 72,
            detune_cents: 0.0,
            gain: 1.0,
            release_seconds: 0.005,
        },
    ];

    let first = PreparedMultisampleMap::prepare_on_control(
        &specs,
        &resources,
        48_000,
        &tuning,
        ZoneOverlapPolicy::FirstMatch,
        MultisampleLimits::DEFAULT,
    )
    .unwrap();
    assert_eq!(first.selected_zones_for_key(35), &[]);
    assert_eq!(first.selected_zones_for_key(36), &[0]);
    assert_eq!(first.selected_zones_for_key(58), &[0]);
    assert_eq!(first.selected_zones_for_key(61), &[1]);
    assert_eq!(first.selected_zones_for_key(84), &[1]);
    assert_eq!(first.selected_zones_for_key(85), &[]);

    // HighestRootProximity: key 55..=60 is closer to root 48 (dist 7..12) than root 72 (dist 17..12).
    // At key 60, |60-48|=12 == |60-72|=12 -> tie breaks deterministically to zone 0!
    // If zone 1 root is 62, key 58 has |58-48|=10 vs |58-62|=4 -> zone 1 wins!
    let mut prox_specs = specs;
    prox_specs[1].root_key = 62;
    let prox = PreparedMultisampleMap::prepare_on_control(
        &prox_specs,
        &resources,
        48_000,
        &tuning,
        ZoneOverlapPolicy::HighestRootProximity,
        MultisampleLimits::DEFAULT,
    )
    .unwrap();
    assert_eq!(prox.selected_zones_for_key(55), &[0]); // dist 7 vs 7 -> tie breaks to 0
    assert_eq!(prox.selected_zones_for_key(56), &[1]); // dist 8 vs 6 -> zone 1 wins

    // LayerAll layers both zones (0.25 + 0.50 = 0.75) on overlap key 58, and produces silence on unmapped key 20.
    let layered = PreparedMultisampleMap::prepare_on_control(
        &specs,
        &resources,
        48_000,
        &tuning,
        ZoneOverlapPolicy::LayerAll,
        MultisampleLimits::DEFAULT,
    )
    .unwrap();
    assert_eq!(layered.selected_zones_for_key(58), &[0, 1]);
    let mut rt = layered.create_runtime();
    let mut out = [[0.0_f32; 2]; 16];
    rt.process_block(
        &layered,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 20, // unmapped gap
            velocity: 127,
        }],
        &mut out,
        &resources,
    )
    .unwrap();
    assert!(rt.is_quiescent());
    assert!(out.iter().flatten().all(|&s| s == 0.0));

    rt.process_block(
        &layered,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 58, // overlap key -> spawns 2 layered voices
            velocity: 127,
        }],
        &mut out,
        &resources,
    )
    .unwrap();
    assert_eq!(rt.active_voice_count(), 2);
    assert!((out[0][0] - 0.75).abs() < 1e-5);
}

#[test]
fn multi_rate_and_custom_tuning_oracle_with_note_off_and_partitions() {
    // Source recorded at 44.1 kHz with 440 Hz tone (root A4 = 69), played at 48 kHz output rate.
    let pcm = sine_pcm(8_192, 440.0, 44_100);
    let view = constant_view(1, 5, 3, 44_100, &pcm);
    let resources = BorrowedDeviceResources(std::slice::from_ref(&view));
    let specs = [KeyZoneSpec {
        slot: slot(1),
        low_key: 48,
        high_key: 84,
        low_velocity: 1,
        high_velocity: 127,
        root_key: 69,
        detune_cents: 0.0,
        gain: 1.0,
        release_seconds: 0.005, // 240 frames at 48 kHz
    }];

    // Custom microtonal tuning: map key 72 to 600.0 Hz (root 69 stays 440.0 Hz).
    let mut freqs = *TuningTable::default().frequencies();
    freqs[72] = 600.0;
    let tuning = TuningTable::new(freqs).unwrap();
    let map = PreparedMultisampleMap::prepare_on_control(
        &specs,
        &resources,
        48_000,
        &tuning,
        ZoneOverlapPolicy::FirstMatch,
        MultisampleLimits::DEFAULT,
    )
    .unwrap();

    let mut whole_rt = map.create_runtime();
    let mut whole = vec![[0.0_f32; 2]; 2_048];
    whole_rt
        .process_block(
            &map,
            &[
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 0,
                    occurrence_id: 1,
                    pitch: 72,
                    velocity: 127,
                },
                NativeInstrumentEvent::NoteOff {
                    frame_offset: 1_500,
                    occurrence_id: 1,
                },
            ],
            &mut whole,
            &resources,
        )
        .unwrap();
    assert!(whole_rt.is_quiescent());
    assert!(whole[1_740..].iter().flatten().all(|&s| s == 0.0));
    let held: Vec<f32> = whole[..1_400].iter().map(|f| f[0]).collect();
    assert!(
        dft_magnitude(&held, 600.0, 48_000) > 0.8,
        "multi-rate custom-tuned note must render 600 Hz fundamental"
    );

    // Bit-identical across irregular block partitions.
    let mut part_rt = map.create_runtime();
    let mut parts = vec![[0.0_f32; 2]; 2_048];
    let mut cursor = 0;
    for chunk in [1, 7, 13, 64, 512, 950, 501] {
        let end = cursor + chunk;
        let mut ev = Vec::new();
        if cursor == 0 {
            ev.push(NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 72,
                velocity: 127,
            });
        }
        if (cursor..end).contains(&1_500) {
            ev.push(NativeInstrumentEvent::NoteOff {
                frame_offset: 1_500 - cursor,
                occurrence_id: 1,
            });
        }
        part_rt
            .process_block(&map, &ev, &mut parts[cursor..end], &resources)
            .unwrap();
        cursor = end;
    }
    assert_eq!(whole, parts);
}

#[test]
fn missing_corrupt_stale_oversized_and_capacity_refusals_preserve_state() {
    let good_pcm = [[0.5_f32, 0.5]; 128];
    let good_view = constant_view(1, 1, 1, 48_000, &good_pcm);
    let spec = KeyZoneSpec {
        slot: slot(1),
        low_key: 0,
        high_key: 127,
        low_velocity: 1,
        high_velocity: 127,
        root_key: 60,
        detune_cents: 0.0,
        gain: 1.0,
        release_seconds: 0.01,
    };
    let tuning = TuningTable::default();

    // Missing source refused on control.
    assert_eq!(
        PreparedMultisampleMap::prepare_on_control(
            &[spec],
            &EMPTY_DEVICE_RESOURCES,
            48_000,
            &tuning,
            ZoneOverlapPolicy::FirstMatch,
            MultisampleLimits::DEFAULT,
        )
        .unwrap_err(),
        MultisampleError::MissingSource
    );

    // Non-finite PCM refused on control.
    let mut bad_pcm = good_pcm;
    bad_pcm[10][1] = f32::INFINITY;
    let bad_view = constant_view(1, 1, 2, 48_000, &bad_pcm);
    assert_eq!(
        PreparedMultisampleMap::prepare_on_control(
            &[spec],
            &BorrowedDeviceResources(std::slice::from_ref(&bad_view)),
            48_000,
            &tuning,
            ZoneOverlapPolicy::FirstMatch,
            MultisampleLimits::DEFAULT,
        )
        .unwrap_err(),
        MultisampleError::NonFiniteSample
    );

    // Tight memory limit refuses oversized source, while shared source across 2 zones is only charged once.
    let tight = MultisampleLimits {
        max_zones: 4,
        max_voices: 2,
        max_events_per_block: 8,
        max_total_frames: 128,
        max_total_bytes: 128 * 8,
    };
    let resources = BorrowedDeviceResources(std::slice::from_ref(&good_view));
    let shared_map = PreparedMultisampleMap::prepare_on_control(
        &[spec, spec],
        &resources,
        48_000,
        &tuning,
        ZoneOverlapPolicy::LayerAll,
        tight,
    )
    .unwrap();
    assert_eq!(shared_map.unique_source_frames(), 128);
    assert_eq!(shared_map.unique_source_bytes(), 128 * 8);

    // Voice capacity refusal is atomic & read-only: 1 note uses 2 layered voices -> 2nd note refuses without partial state.
    let mut rt = shared_map.create_runtime();
    let mut out = [[0.0_f32; 2]; 16];
    rt.process_block(
        &shared_map,
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        }],
        &mut out,
        &resources,
    )
    .unwrap();
    assert_eq!(rt.active_voice_count(), 2);
    assert_eq!(
        rt.process_block(
            &shared_map,
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 2,
                pitch: 60,
                velocity: 100,
            }],
            &mut out,
            &resources,
        ),
        Err(MultisampleError::VoiceCapacityExceeded)
    );
    assert_eq!(rt.active_voice_count(), 2);

    // Stale / changed identity on callback refuses atomically with silence, keeping voices intact.
    let changed_view = constant_view(1, 1, 99, 48_000, &good_pcm);
    out = [[1.0; 2]; 16];
    assert_eq!(
        rt.process_block(
            &shared_map,
            &[],
            &mut out,
            &BorrowedDeviceResources(std::slice::from_ref(&changed_view)),
        ),
        Err(MultisampleError::SourceUnavailable)
    );
    assert!(out.iter().flatten().all(|&s| s == 0.0));
    assert_eq!(rt.active_voice_count(), 2);
}
