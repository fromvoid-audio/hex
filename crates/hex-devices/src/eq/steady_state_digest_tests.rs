//! Bit-identity pin for the flexible EQ's steady-state (non-mode-switching)
//! output. The digests cover the first 2000 frames, before the intentional
//! gain retarget; they were recorded from the unmodified pre-smoothing EQ.
//! The input is integer-generated noise (no libm), so the digest depends only
//! on IEEE f32/f64 arithmetic.
use super::*;

fn preset(mode: u8, boost_db: f32) -> EqState {
    let mut state = EqState {
        mode,
        ..EqState::default()
    };
    let mut id = 1;
    for bank in 0..5 {
        for (hz, db, kind) in [
            (1_000.0, boost_db, NodeKind::Bell),
            (4_200.0 + 300.0 * f32::from(bank), -7.5, NodeKind::HighShelf),
            (90.0, 0.0, NodeKind::LowCut),
        ] {
            let mut node = Node::new(NodeId::new(id).unwrap(), hz, db, bank).unwrap();
            node.kind = kind;
            state.add(node).unwrap();
            id += 1;
        }
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

/// Render with same-mode handoffs every block (a twin copy, a bell-gain
/// retarget and a band audition that is requested, changed and released),
/// varying blocks.
fn render(mode: u8, rate: u32) -> u64 {
    let first = runtime::PreparedEq::new(preset(mode, 12.0), rate).unwrap();
    let second = runtime::PreparedEq::new(preset(mode, 6.0), rate).unwrap();
    let lane_bank = |lane: u8| match mode {
        1 => 1 + lane,
        2 => 3 + lane,
        _ => 0,
    };
    // Node ids are 1 + 3 * bank + slot.
    let audition = |lane: u8, slot: u32| ProcessorAuditionSelection {
        index: 1 + 3 * u32::from(lane_bank(lane)) + slot,
        lane,
    };
    let mut live = first.create_runtime();
    let mut frames = noise(6_000);
    let mut offset = 0;
    for (block_index, size) in [1_usize, 7, 64, 128, 113].into_iter().cycle().enumerate() {
        if offset >= frames.len() {
            break;
        }
        let end = (offset + size).min(frames.len());
        let prepared = if offset < 2_000 { &first } else { &second };
        let mut next = prepared.create_runtime();
        assert!(next.can_copy_history_from(live.as_ref()));
        next.copy_history_from(live.as_ref());
        let selection = match block_index {
            20 => Some(Some(audition(0, 0))),
            45 => Some(Some(audition(1, 1))),
            70 => Some(None),
            _ => None,
        };
        if let Some(selection) = selection {
            if let Some(chosen) = selection {
                assert!(next.transient_audition().unwrap().can_select(chosen));
            }
            next.transient_audition().unwrap().select(selection);
        }
        next.process_block(
            &mut frames[offset..end],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        live = next;
        offset = end;
    }
    fnv(&frames[..2000])
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
    0x0c75_0d40_2083_e149,
    0x747b_6b88_a5b5_ef63,
    0xcf61_de15_0154_f583,
    0xa570_d232_57bd_9da9,
    0xd8fd_d4d6_9739_dfc5,
];
