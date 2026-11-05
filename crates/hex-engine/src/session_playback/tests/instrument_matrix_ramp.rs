//! Pulse #4105: a static-matrix edit after a sounding instrument ramps.

use super::*;
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

/// 5 ms at the fixture's 48 kHz, the #4101 static matrix ramp length.
const RAMP_FRAMES: usize = 240;

/// Instrument, then a stateful EQ8, then a memoryless Utility. The EQ8 keeps
/// the suffix nonquiescent while the note sounds, so an incompatible change
/// would retain the old suffix as a tail. The Utility comes last so its gain
/// ramp multiplies the unchanged EQ8 output exactly.
fn instrument_eq8_utility(
    utility_gain: f32,
    remove_eq8: bool,
    table: &Arc<hex_devices::SineTable>,
) -> Arc<PreparedRevision> {
    use hex_devices::eq8::parameter as eq8;
    let mut eq8_state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    for (id, value) in [
        (eq8::enabled(0, 0), ParameterValue::Bool(true)),
        (eq8::gain(0, 0), ParameterValue::Float(3.0)),
    ] {
        eq8_state = crate::patch_native_device_parameter(&eq8_state, id, value).unwrap();
    }
    let neutral = UTILITY_DEFINITION.default_state().unwrap();
    let utility_state = UTILITY_DEFINITION
        .patch_parameter(
            &neutral,
            parameter::GAIN,
            ParameterValue::Float(utility_gain),
        )
        .unwrap();
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    snapshot.tracks[0].devices = [(20, eq8_state), (21, utility_state)]
        .into_iter()
        .map(|(id, state)| DeviceSnapshot {
            automation: Vec::new(),
            comparison: None,
            id,
            bypassed: false,
            state,
        })
        .collect();
    if remove_eq8 {
        snapshot.tracks[0].devices.retain(|device| device.id != 20);
    }
    snapshot.next_id = 22;
    let project = legacy_project_from_snapshot(snapshot).unwrap();
    // The instrument precedes both effects, so they form its suffix.
    assert_eq!(project.tracks()[0].instrument_position(), Some(0));
    revision(&project, Arc::clone(table))
}

fn sounding(revision: &Arc<PreparedRevision>) -> SessionPlayback {
    let mut playback = SessionPlayback::new(Arc::clone(revision)).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 64];
    playback.render(&mut prefix).unwrap();
    assert!(prefix.iter().any(|frame| frame[0].abs() > 1.0e-3));
    let chain = &playback.device_chains[0];
    assert!(!chain.committed_range_is_quiescent(0..chain.len()));
    playback
}

/// A direct Utility gain edit while the instrument's note sounds keeps one
/// suffix owner and ramps its matrix linearly over 5 ms, at any block size.
#[test]
fn utility_gain_edit_after_a_sounding_instrument_ramps_over_five_ms() {
    const TARGET: f32 = 0.25;
    let table = Arc::new(hex_devices::SineTable::new());
    let old_revision = instrument_eq8_utility(1.0, false, &table);
    let candidate_revision = instrument_eq8_utility(TARGET, false, &table);

    let mut expected = [[0.0; 2]; 512];
    sounding(&old_revision).render(&mut expected).unwrap();
    for (frame, sample) in expected.iter_mut().enumerate() {
        let gain = if frame + 1 < RAMP_FRAMES {
            let position = (frame + 1) as f64 / RAMP_FRAMES as f64;
            (1.0 + (f64::from(TARGET) - 1.0) * position) as f32
        } else {
            TARGET
        };
        *sample = sample.map(|value| value * gain);
    }

    for block in [1_usize, 7, 64, 128] {
        let mut previous = sounding(&old_revision);
        let mut current = SessionPlayback::new(Arc::clone(&candidate_revision)).unwrap();
        current.try_inherit_state_from(&mut previous).unwrap();
        assert_eq!(
            current.tail_groups.iter().flatten().count(),
            0,
            "a static-matrix edit must not detach the instrument suffix"
        );
        let mut output = [[0.0; 2]; 512];
        for chunk in output.chunks_mut(block) {
            current.render(chunk).unwrap();
        }
        for (frame, (actual, wanted)) in output.iter().zip(&expected).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - wanted[channel]).abs() <= 1.0e-5,
                    "block {block} frame {frame} channel {channel}: {actual:?} != {wanted:?}"
                );
            }
        }
    }
}

/// Removing a stateful suffix still retains its old owner as a tail.
#[test]
fn removed_suffix_after_a_sounding_instrument_keeps_a_retained_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let old_revision = instrument_eq8_utility(1.0, false, &table);
    let candidate_revision = instrument_eq8_utility(0.25, true, &table);
    let mut previous = sounding(&old_revision);
    let mut current = SessionPlayback::new(candidate_revision).unwrap();
    current.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(current.tail_groups.iter().flatten().count(), 1);
    let mut output = [[0.0; 2]; 64];
    current.render(&mut output).unwrap();
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
}
