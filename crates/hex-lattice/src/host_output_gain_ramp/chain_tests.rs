//! Exercise the real host stage, not a second test-only ramp implementation.
use crate::*;
use hex_lattice_contract::{DeviceAutomationTarget::HostOutputGain, HostGain};
use std::sync::Arc;
const INPUT: StereoFrame = [0.25, -0.125];
fn chain(rate: u32, db: f32, owner: ProcessorChainOwner) -> NativeRuntimeChain {
    let kind = DeviceTypeId::from_static("test.host-gain");
    NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner,
        device_id: 7,
        device_type: kind,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: rate,
        configuration: Some(Arc::new(
            NativeDeviceState::new(kind, 1, vec![1])
                .unwrap()
                .with_output_gain_db(db)
                .unwrap(),
        )),
        definition: None,
    }])
    .unwrap()
}
fn render(chain: &mut NativeRuntimeChain, count: usize, block: usize) -> Vec<StereoFrame> {
    let mut pcm = vec![INPUT; count];
    for chunk in pcm.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(chunk).unwrap();
        chain.commit_block();
    }
    pcm
}
fn edited(rate: u32, owner: ProcessorChainOwner) -> NativeRuntimeChain {
    let mut old = chain(rate, 0.0, owner);
    render(&mut old, 9, 7);
    let mut new = chain(rate, -6.0, owner);
    new.transfer_history_from(&mut old);
    new
}
#[test]
fn canonical_effect_output_trim_has_exact_pcm_partition_and_twin_continuation_laws() {
    for owner in [
        ProcessorChainOwner::Track(1),
        ProcessorChainOwner::Return(2),
        ProcessorChainOwner::Main,
    ] {
        for rate in [8000_u32, 44100, 48000, 192000] {
            let n = rate.div_ceil(200) as usize;
            let gain = HostGain::from_db(-6.0).unwrap().linear();
            let expected = (1..=n + 17)
                .map(|i| {
                    let g = if i >= n {
                        gain
                    } else {
                        (1.0 + (f64::from(gain) - 1.0) * i as f64 / n as f64) as f32
                    };
                    [INPUT[0] * g, INPUT[1] * g]
                })
                .collect::<Vec<_>>();
            for block in [1, 7, 64, 512] {
                assert_eq!(
                    render(&mut edited(rate, owner), n + 17, block),
                    expected,
                    "{owner:?} rate{rate} block{block}"
                );
            }
            let mut first = edited(rate, owner);
            let mut pcm = render(&mut first, n / 2, 7);
            let mut twin = chain(rate, -6.0, owner);
            twin.inherit_from(&first);
            pcm.extend(render(&mut twin, n + 17 - n / 2, 64));
            assert_eq!(pcm, expected);
        }
    }
}
#[test]
fn rollback_reset_and_frozen_tail_keep_committed_gain_and_trajectory() {
    let owner = ProcessorChainOwner::Track(1);
    let mut rolled = edited(48000, owner);
    rolled.begin_block();
    rolled.process_block(&mut [INPUT; 73]).unwrap();
    assert!(rolled.process_range(1..2, &mut [INPUT; 1]).is_err());
    rolled.rollback_block();
    assert_eq!(
        rolled.entries[0].scratch_output_gain_ramp,
        rolled.entries[0].committed_output_gain_ramp
    );
    assert_eq!(
        render(&mut rolled, 81, 7),
        render(&mut edited(48000, owner), 81, 7)
    );
    rolled.reset();
    let target = HostGain::from_db(-6.0).unwrap().linear();
    assert!(
        render(&mut rolled, 64, 64)
            .iter()
            .all(|p| *p == [INPUT[0] * target, INPUT[1] * target])
    );
    let mut tail = edited(48000, owner);
    let pcm = render(&mut tail, 73, 7);
    let last = pcm[72];
    tail.begin_output_tail();
    assert!(render(&mut tail, 512, 64).iter().all(|p| *p == last));
}
#[test]
fn unproved_donors_and_output_programs_apply_the_existing_exact_law() {
    let owner = ProcessorChainOwner::Track(1);
    let target = HostGain::from_db(-6.0).unwrap().linear();
    for case in 0..8 {
        let mut old = chain(48000, 0.0, owner);
        render(&mut old, 8, 8);
        let mut new = chain(48000, -6.0, owner);
        match case {
            0 => new.entries[0].device_id = 8,
            1 => new.entries[0].configuration = None,
            2 => new.entries[0].input_gain = 0.5,
            3 => new.entries[0].matrix = StereoMatrix::new([[0.5, 0.0], [0.0, 0.5]]).unwrap(),
            4 => new.entries[0].sample_rate = 44100,
            5 => {
                new.transfer_history_excluding_old_range(&mut old, 0..1)
                    .unwrap();
            }
            6 => {
                let program = PreparedParameterAutomation::new_with_host(
                    Vec::new(),
                    vec![HostAutomationLane {
                        target: HostOutputGain,
                        points: 0..1,
                    }],
                    vec![ParameterAutomationPoint {
                        beat_raw: 0,
                        normalized: HostGain::from_db(-6.0).unwrap().normalized(),
                        step: true,
                    }],
                )
                .unwrap();
                old.entries[0].automation = program.clone();
                new.entries[0].automation = program;
            }
            7 => {
                let mut program = PreparedParameterAutomation::default();
                program.arm_host_gain_input(HostOutputGain).unwrap();
                old.entries[0].automation = program.clone();
                new.entries[0].automation = program;
            }
            _ => panic!("case"),
        }
        if case != 5 {
            new.transfer_history_from(&mut old);
        }
        assert!(
            !new.entries[0].committed_output_gain_ramp.active(),
            "case{case}"
        );
        if case == 0 || case == 1 || case == 4 || case == 5 || case == 7 {
            assert!(
                render(&mut new, 8, 7)
                    .iter()
                    .all(|p| *p == [INPUT[0] * target, INPUT[1] * target]),
                "case{case}"
            );
        }
    }
}

#[test]
fn wet_output_trajectory_survives_bypass_and_reversal_without_a_new_gain_step() {
    let owner = ProcessorChainOwner::Track(1);
    let mut first = edited(48000, owner);
    let first_pcm = render(&mut first, 73, 7);
    let mut bypassed = chain(48000, -6.0, owner);
    bypassed.entries[0].bypassed = true;
    assert!(bypassed.configuration_matches_range(&first, 0..1));
    bypassed.transfer_history_from(&mut first);
    let bypass_pcm = render(&mut bypassed, 41, 7);
    let mut enabled = chain(48000, -6.0, owner);
    enabled.transfer_history_from(&mut bypassed);
    let enabled_pcm = render(&mut enabled, 512, 64);
    let mut previous = first_pcm[72];
    for sample in bypass_pcm.into_iter().chain(enabled_pcm.iter().copied()) {
        assert!(
            (sample[0] - previous[0]).abs() < 0.005,
            "{previous:?}->{sample:?}"
        );
        previous = sample;
    }
    let gain = HostGain::from_db(-6.0).unwrap().linear();
    assert_eq!(
        *enabled_pcm.last().unwrap(),
        [INPUT[0] * gain, INPUT[1] * gain]
    );
}

#[test]
fn authored_output_gain_step_stays_sample_accurate_across_a_canonical_state_edit() {
    struct Clock(u64);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.checked_add(offset as u64)
        }
        fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
            self.beat_at_offset(offset)
        }
    }
    let program = PreparedParameterAutomation::new_with_host(
        Vec::new(),
        vec![HostAutomationLane {
            target: HostOutputGain,
            points: 0..2,
        }],
        vec![
            ParameterAutomationPoint {
                beat_raw: 0,
                normalized: HostGain::ZERO.normalized(),
                step: true,
            },
            ParameterAutomationPoint {
                beat_raw: 100,
                normalized: HostGain::from_db(-12.0).unwrap().normalized(),
                step: true,
            },
        ],
    )
    .unwrap();
    let owner = ProcessorChainOwner::Track(1);
    let mut old = chain(48000, 0.0, owner);
    old.entries[0].automation = program.clone();
    old.begin_block();
    old.process_range_with_clock(0..1, &mut [INPUT; 64], Some(&Clock(0)), Some(&Clock(0)))
        .unwrap();
    old.commit_block();
    let mut new = chain(48000, -6.0, owner);
    new.entries[0].automation = program;
    new.transfer_history_from(&mut old);
    let mut pcm = [INPUT; 256];
    new.begin_block();
    new.process_range_with_clock(0..1, &mut pcm, Some(&Clock(64)), Some(&Clock(64)))
        .unwrap();
    new.commit_block();
    let gain = HostGain::from_db(-12.0).unwrap().linear();
    for (i, p) in pcm.iter().enumerate() {
        assert_eq!(
            *p,
            if i < 36 {
                INPUT
            } else {
                [INPUT[0] * gain, INPUT[1] * gain]
            },
            "frame{}",
            i + 64
        );
    }
}
