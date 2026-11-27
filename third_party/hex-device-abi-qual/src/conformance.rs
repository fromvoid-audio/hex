//! Device-class conformance checks for independently authored `.hexdevice`
//! native modules (`#2997`).
//!
//! Runs strictly on control/test threads; never invoke from an audio callback.
#![deny(unsafe_code)]

use crate::InProcessModule;
use hex_lattice_contract::package_abi::{
    AbiNoteEvent, AbiParamEvent, AbiStatus, CAP_STATE_PERSISTENCE, DEVICE_KIND_AUDIO_EFFECT,
    DEVICE_KIND_INSTRUMENT, NOTE_EVENT_OFF, NOTE_EVENT_ON,
};

/// Exact stage where a native package module failed conformance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageConformanceFailure {
    pub stage: &'static str,
    pub detail: String,
}

/// Summary report returned when a native package passes all device-class checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageConformanceReport {
    pub type_id: String,
    pub device_kind: u32,
    pub parameter_count: u32,
    pub blocks_tested: usize,
}

/// Execute device-class conformance checks against an independently compiled
/// native device module:
/// 1. Descriptor & independent version validation (`package`/`state`/`abi`/`ui`).
/// 2. All declared parameter descriptors validate cleanly with unique non-zero IDs.
/// 3. State round-trip and malformed-state refusal (when `CAP_STATE_PERSISTENCE` is set).
/// 4. Finite PCM rendering across regular and odd block sizes (`1, 7, 13, 32, 64, 127, 256, 512`)
///    at 44.1 / 48 / 96 kHz, with parameter & note events.
/// 5. Stop/reset determinism: destroying and recreating an instance from the
///    same saved state reproduces bit-identical PCM on the same input.
pub fn check_package_conformance(
    module: &InProcessModule,
) -> Result<PackageConformanceReport, PackageConformanceFailure> {
    let desc = module.descriptor();
    let (type_id, _versions) = desc.validate().map_err(|s| PackageConformanceFailure {
        stage: "descriptor_validation",
        detail: format!("{s:?}"),
    })?;
    let type_id_str = type_id
        .as_str()
        .map_err(|_| PackageConformanceFailure {
            stage: "type_id",
            detail: "invalid utf-8 type id".to_owned(),
        })?
        .to_owned();

    let mut seen_param_ids = Vec::with_capacity(desc.parameter_count as usize);
    let mut first_param = None;
    for i in 0..desc.parameter_count {
        let p = module
            .parameter_descriptor(i)
            .map_err(|s| PackageConformanceFailure {
                stage: "parameter_descriptor",
                detail: format!("param[{i}] failed: {s:?}"),
            })?;
        let _name = p.validate().map_err(|s| PackageConformanceFailure {
            stage: "parameter_descriptor_validate",
            detail: format!("param[{i}] invalid: {s:?}"),
        })?;
        if seen_param_ids.contains(&p.id) {
            return Err(PackageConformanceFailure {
                stage: "parameter_unique_id",
                detail: format!("duplicate param id {}", p.id),
            });
        }
        seen_param_ids.push(p.id);
        if i == 0 {
            first_param = Some(p);
        }
    }

    // Out-of-range parameter query must refuse cleanly.
    if module.parameter_descriptor(desc.parameter_count).is_ok() {
        return Err(PackageConformanceFailure {
            stage: "parameter_bounds",
            detail: "accepted out-of-range parameter index".to_owned(),
        });
    }

    // Invalid sample rate must refuse cleanly.
    if module.create_instance(0).is_ok() || module.create_instance(1_000_000).is_ok() {
        return Err(PackageConformanceFailure {
            stage: "sample_rate_bounds",
            detail: "accepted out-of-range sample rate".to_owned(),
        });
    }

    let odd_and_regular_blocks = [1usize, 7, 13, 32, 64, 127, 256, 512];
    let mut blocks_tested = 0usize;

    for &rate in &[44_100u32, 48_000, 96_000] {
        let mut inst = module
            .create_instance(rate)
            .map_err(|s| PackageConformanceFailure {
                stage: "create_instance",
                detail: format!("rate {rate}: {s:?}"),
            })?;

        let mut saved_state = vec![0u8; (desc.max_state_bytes as usize).max(16)];
        let saved_len = if (desc.capabilities & CAP_STATE_PERSISTENCE) != 0 {
            let len = inst
                .get_state(&mut saved_state)
                .map_err(|s| PackageConformanceFailure {
                    stage: "state_get",
                    detail: format!("{s:?}"),
                })?;
            inst.set_state(&saved_state[..len])
                .map_err(|s| PackageConformanceFailure {
                    stage: "state_roundtrip",
                    detail: format!("{s:?}"),
                })?;
            // Truncated/malformed state must refuse.
            if len > 0 && inst.set_state(&[0xFF; 1]).is_ok() {
                return Err(PackageConformanceFailure {
                    stage: "malformed_state_refusal",
                    detail: "accepted 1-byte malformed state".to_owned(),
                });
            }
            len
        } else {
            0
        };

        for &frames in &odd_and_regular_blocks {
            let input: Vec<[f32; 2]> = (0..frames)
                .map(|i| {
                    let t = i as f32 / frames.max(1) as f32;
                    [(t * 4.0).sin() * 0.4, (t * 7.0).cos() * 0.3]
                })
                .collect();
            let mut output = vec![[0.0f32; 2]; frames];
            let param_events = first_param
                .map(|p| {
                    vec![AbiParamEvent {
                        sample_offset: (frames / 2) as u32,
                        parameter_id: p.id,
                        value: p.default_value,
                        reserved: 0,
                    }]
                })
                .unwrap_or_default();
            let note_events = if desc.device_kind == DEVICE_KIND_INSTRUMENT {
                vec![
                    AbiNoteEvent {
                        sample_offset: 0,
                        kind: NOTE_EVENT_ON,
                        note: 69,
                        channel: 0,
                        reserved0: 0,
                        velocity: 0.75,
                        detune_cents: 0.0,
                        reserved1: 0,
                    },
                    AbiNoteEvent {
                        sample_offset: (frames.saturating_sub(1)) as u32,
                        kind: NOTE_EVENT_OFF,
                        note: 69,
                        channel: 0,
                        reserved0: 0,
                        velocity: 0.0,
                        detune_cents: 0.0,
                        reserved1: 0,
                    },
                ]
            } else {
                Vec::new()
            };
            inst.process(&input, &mut output, &param_events, &note_events)
                .map_err(|s| PackageConformanceFailure {
                    stage: if s == AbiStatus::InvalidOutput {
                        "finite_pcm"
                    } else {
                        "process"
                    },
                    detail: format!("rate={rate} frames={frames}: {s:?}"),
                })?;
            blocks_tested += 1;
        }

        // Stop/reset determinism check at 48 kHz: fresh instance with restored
        // state renders identical PCM on the same block.
        if rate == 48_000 {
            let input = [[0.3f32, -0.2f32]; 64];
            let mut out_a = [[0.0f32; 2]; 64];
            let mut out_b = [[0.0f32; 2]; 64];
            let mut a = module.create_instance(rate).unwrap();
            let mut b = module.create_instance(rate).unwrap();
            if (desc.capabilities & CAP_STATE_PERSISTENCE) != 0 {
                a.set_state(&saved_state[..saved_len]).unwrap();
                b.set_state(&saved_state[..saved_len]).unwrap();
            }
            let notes = if desc.device_kind == DEVICE_KIND_INSTRUMENT {
                vec![AbiNoteEvent {
                    sample_offset: 4,
                    kind: NOTE_EVENT_ON,
                    note: 64,
                    channel: 0,
                    reserved0: 0,
                    velocity: 0.8,
                    detune_cents: 0.0,
                    reserved1: 0,
                }]
            } else {
                Vec::new()
            };
            a.process(&input, &mut out_a, &[], &notes).unwrap();
            b.process(&input, &mut out_b, &[], &notes).unwrap();
            if out_a != out_b {
                return Err(PackageConformanceFailure {
                    stage: "reset_determinism",
                    detail: "fresh recreated instances produced diverging PCM".to_owned(),
                });
            }
            // Audio effects must not silently zero all output on valid non-zero input
            // when default state is active.
            if desc.device_kind == DEVICE_KIND_AUDIO_EFFECT
                && out_a.iter().all(|f| *f == [0.0, 0.0])
            {
                return Err(PackageConformanceFailure {
                    stage: "effect_non_silent_default",
                    detail: "audio effect produced all-zero output on default state".to_owned(),
                });
            }
        }
    }

    Ok(PackageConformanceReport {
        type_id: type_id_str,
        device_kind: desc.device_kind,
        parameter_count: desc.parameter_count,
        blocks_tested,
    })
}
