//! Bit-identity pin for EQ8's steady-state (non-mode-switching) output.
//! The digests were recorded from origin/main before the Pulse #4106
//! mode-fade and audition-blend refactor; the same render must stay
//! bit-identical. The input is integer-generated noise (no libm), so the
//! digest depends only on IEEE f32/f64 arithmetic.
use super::*;

fn patch(state: NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    EQ8_DEFINITION.patch_parameter(&state, id, value).unwrap()
}

fn band(
    state: NativeDeviceState,
    bank: usize,
    band: usize,
    kind: u32,
    hz: f32,
    db: f32,
) -> NativeDeviceState {
    let state = patch(
        state,
        parameter::enabled(bank, band),
        ParameterValue::Bool(true),
    );
    let state = patch(
        state,
        parameter::kind(bank, band),
        ParameterValue::Choice(kind),
    );
    let state = patch(
        state,
        parameter::frequency(bank, band),
        ParameterValue::Float(hz),
    );
    patch(
        state,
        parameter::gain(bank, band),
        ParameterValue::Float(db),
    )
}

fn preset(mode: u32, output_db: f32) -> NativeDeviceState {
    let mut state = patch(
        EQ8_DEFINITION.default_state().unwrap(),
        parameter::MODE,
        ParameterValue::Choice(mode),
    );
    state = patch(
        state,
        parameter::OUTPUT_GAIN,
        ParameterValue::Float(output_db),
    );
    for bank in 0..5 {
        state = band(state, bank, 0, 0, 1_000.0, 12.0);
        state = band(state, bank, 3, 0, 4_200.0 + 300.0 * bank as f32, -7.5);
        state = band(state, bank, 5, 7, 180.0, 0.0);
    }
    state
}

fn noise(frames: usize) -> Vec<StereoFrame> {
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as i32) as f32 / i32::MAX as f32 * 0.5
    };
    (0..frames).map(|_| [next(), next()]).collect()
}

fn fnv(frames: &[StereoFrame]) -> u64 {
    frames
        .iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325, |hash, value| {
            value
                .to_bits()
                .to_le_bytes()
                .iter()
                .fold(hash, |hash, byte| {
                    (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
                })
        })
}

/// Render with same-mode handoffs every block (an output-gain retarget and a
/// band audition that is requested, changed and released), varying blocks.
fn render(mode: u32, rate: u32) -> u64 {
    let first = PreparedEq8::new(decode(&preset(mode, 0.0)).unwrap(), rate).unwrap();
    let second = PreparedEq8::new(decode(&preset(mode, 4.5)).unwrap(), rate).unwrap();
    let mut live = first.runtime();
    let mut frames = noise(6_000);
    let mut offset = 0;
    for (block_index, size) in [1_usize, 7, 64, 128, 113].into_iter().cycle().enumerate() {
        if offset >= frames.len() {
            break;
        }
        let end = (offset + size).min(frames.len());
        let prepared = if offset < 2_000 { &first } else { &second };
        let mut next = prepared.runtime();
        assert!(next.inherit_history_from(&live));
        let selection = match block_index {
            20 => Some(Some(Eq8AuditionSelection::new(0, 0).unwrap())),
            45 => Some(Some(Eq8AuditionSelection::new(3, 1).unwrap())),
            70 => Some(None),
            _ => None,
        };
        if let Some(selection) = selection {
            next.set_audition_selection(selection).unwrap();
        }
        next.process(&mut frames[offset..end]);
        live = next;
        offset = end;
    }
    fnv(&frames)
}

#[test]
fn steady_state_output_is_bit_identical_to_main() {
    let digests: Vec<u64> = [
        (0, 48_000),
        (1, 48_000),
        (2, 48_000),
        (2, 44_100),
        (0, 192_000),
    ]
    .into_iter()
    .map(|(mode, rate)| render(mode, rate))
    .collect();
    assert_eq!(digests, MAIN_DIGESTS, "{digests:#018x?}");
}

const MAIN_DIGESTS: [u64; 5] = [
    0x1dfb_4011_052c_be3a,
    0x9e7e_3b6b_b397_0a38,
    0x5278_814f_c3ab_6dbc,
    0xb6ed_ad94_7e69_0335,
    0x8787_212b_a83d_6f15,
];
