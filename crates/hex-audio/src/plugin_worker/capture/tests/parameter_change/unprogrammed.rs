//! A newly armed target owns a hold before any program mentions it. Both
//! ordinary cleanup and atomic base restoration use its original worker.
use super::*;
use crate::plugin_worker::NativeParameterHold;
use hex_plugin_host::transport::{
    AuthoredParameterProgram, AuthoredSegment, SegmentTiming, TimingChanges,
};
use std::num::NonZeroU64;

struct OtherLane;
impl AuthoredParameterProgram for OtherLane {
    fn quantum_program_key(&self) -> NonZeroU64 {
        NonZeroU64::MIN
    }
    fn render_quantum(
        &self,
        _: &[Option<u64>],
        _: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault> {
        output.push(ParameterPoint {
            frame: 0,
            id: 11,
            normalized: 0.5,
        })
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn holds_restore_and_release_before_the_first_target_programme() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::unprogrammed::holds_restore_and_release_before_the_first_target_programme";
    if fixture_child(144, NAME) {
        return;
    }
    // A real, writable SDK parameter alone is insufficient: its metadata must
    // have been armed on this exact reader before a persistent hold is allowed.
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let unarmed = owner
        .parameter_hold_on_control(NativeParameterHold::Begin {
            id: 7,
            normalized: 0.75,
        })
        .unwrap();
    callback(&owner, &mut endpoint, 0, 1.0, &[]);
    assert_eq!(
        terminal(&mut owner, &unarmed),
        NativeWorkerParameterProgress::Refused
    );
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    for other_lane in [false, true] {
        let mut instance = original_instance();
        let class = instance.configuration().identity.class;
        let metadata = instance
            .read_authored_parameter_metadata_on_control(class, &[7, 11])
            .unwrap();
        instance
            .arm_authored_parameter_metadata_on_control(metadata)
            .unwrap();
        let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
        let mut render = |owner: &NativeWorker, sequence: u64, primary: f32, marker| {
            let mut output = [[9.0; 2]; 8];
            let mut accepted = None;
            let stage = || {
                endpoint.begin_physical_callback(8).unwrap();
                if other_lane {
                    endpoint
                        .stage_segment_with_authored_parameters(
                            0,
                            &[[0.5, -0.25]; 8],
                            &[],
                            &mut output,
                            SegmentTiming {
                                playing: None,
                                at: |_| None,
                            },
                            AuthoredSegment {
                                program: &OtherLane,
                                origin: &|frame| Some(sequence * 8 + frame as u64),
                            },
                        )
                        .unwrap();
                } else {
                    endpoint
                        .stage_segment_with_parameters(0, &[[0.5, -0.25]; 8], &[], &[], &mut output)
                        .unwrap();
                }
                endpoint.finish(true).unwrap();
                accepted = endpoint.take_accepted_manual_parameter();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let counts = allocation_counter::measure(stage);
                assert_eq!(
                    (
                        counts.count_total,
                        counts.bytes_total,
                        counts.count_current,
                        counts.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            {
                let mut stage = stage;
                stage();
            }
            let until = Instant::now() + Duration::from_secs(3);
            while owner.published_blocks_on_control() <= sequence {
                assert!(
                    Instant::now() < until,
                    "{:?}",
                    owner.take_failure_diagnostic_on_control()
                );
                thread::yield_now();
            }
            let expected = if sequence < 2 {
                0.0
            } else {
                primary * if other_lane { 4.0 } else { 2.0 }
            };
            assert_eq!(output, [[expected, -expected / 2.0]; 8]);
            assert_eq!(
                accepted.map(|receipt| {
                    assert_eq!(
                        (receipt.sequence, receipt.first_frame),
                        (sequence - 2, (sequence - 2) * 8)
                    );
                    assert_eq!(receipt.callback_offset, 0);
                    assert_eq!(receipt.input.parameter(), 7);
                    (
                        receipt.input.operation.get(),
                        receipt.input.point().map(|point| point.normalized),
                    )
                }),
                marker
            );
        };
        let admit = |owner: &mut NativeWorker, command| {
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                match owner.parameter_hold_on_control(command) {
                    Ok(token) => break token,
                    Err("native parameter mailbox is busy") => thread::yield_now(),
                    Err(reason) => panic!("{reason}"),
                }
                assert!(Instant::now() < until);
            }
        };
        let processed =
            |owner: &mut NativeWorker, token: &NativeWorkerParameterChange, expected| {
                let NativeWorkerParameterProgress::Processed(point) = terminal(owner, token) else {
                    panic!("validated target must hold even without its first programme");
                };
                assert_eq!(point.point.normalized, expected);
            };
        render(&owner, 0, 0.25, None);
        let hold = admit(
            &mut owner,
            NativeParameterHold::Begin {
                id: 7,
                normalized: 0.75,
            },
        );
        render(&owner, 1, 0.25, None);
        processed(&mut owner, &hold, 0.75);
        render(&owner, 2, 0.25, None);
        let change = admit(
            &mut owner,
            NativeParameterHold::Change {
                hold: &hold,
                normalized: 0.5,
            },
        );
        render(&owner, 3, 0.75, Some((1, Some(0.75))));
        processed(&mut owner, &change, 0.5);
        // Exercise existing cleanup and the atomic recording end. Another
        // lane keeps supplying its independent value during restoration.
        let release = if other_lane {
            admit(
                &mut owner,
                NativeParameterHold::RestoreAndRelease {
                    hold: &hold,
                    normalized: 0.25,
                },
            )
        } else {
            let restore = admit(
                &mut owner,
                NativeParameterHold::Change {
                    hold: &hold,
                    normalized: 0.25,
                },
            );
            render(&owner, 4, 0.75, None);
            processed(&mut owner, &restore, 0.25);
            admit(&mut owner, NativeParameterHold::Release { hold: &hold })
        };
        let release_sequence = if other_lane { 4 } else { 5 };
        if other_lane {
            render(&owner, 4, 0.75, None);
        }
        render(&owner, 5, 0.5, Some((2, Some(0.5))));
        assert!(matches!(
            terminal(&mut owner, &release),
            NativeWorkerParameterProgress::Released { sequence, first_frame }
                if sequence == release_sequence && first_frame == release_sequence * 8
        ));
        if other_lane {
            render(&owner, 6, 0.25, Some((3, None)));
            render(&owner, 7, 0.25, None);
        } else {
            render(&owner, 6, 0.25, Some((3, Some(0.25))));
            render(&owner, 7, 0.25, Some((4, None)));
        }
        let stale = admit(
            &mut owner,
            NativeParameterHold::RestoreAndRelease {
                hold: &hold,
                normalized: 0.75,
            },
        );
        render(&owner, 8, 0.25, None);
        assert_eq!(
            terminal(&mut owner, &stale),
            NativeWorkerParameterProgress::Refused
        );
        let unknown = admit(
            &mut owner,
            NativeParameterHold::Begin {
                id: 99,
                normalized: 0.75,
            },
        );
        render(&owner, 9, 0.25, None);
        assert_eq!(
            terminal(&mut owner, &unknown),
            NativeWorkerParameterProgress::Refused
        );
        owner.revoke_on_control();
        let retired = owner.retire_on_control().unwrap();
        assert!(retired.failure.is_none() && retired.native.status.success());
    }
}
