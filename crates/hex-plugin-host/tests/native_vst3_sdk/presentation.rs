//! Actual SDK ports and original-instance renewal for mixed presentation plans.
use super::*;
use crate::protocol::{Presentation, PresentationPlan};

fn ports(state: &NativeDeviceState) -> (u32, u32) {
    let state = decode(state).unwrap();
    let bytes = state.component.unwrap();
    (
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
        u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
    )
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn mixed_directions_reach_sdk_startup_and_same_child_renewal() {
    for (initial, expected, first_ports, next, next_expected, next_ports) in [
        (
            PresentationPlan::KnownInput(77),
            Presentation::InputKnown,
            (77, 0),
            PresentationPlan::KnownOutput(91),
            Presentation::OutputKnown,
            (0, 91),
        ),
        (
            PresentationPlan::KnownOutput(91),
            Presentation::OutputKnown,
            (0, 91),
            PresentationPlan::KnownInput(77),
            Presentation::InputKnown,
            (77, 0),
        ),
        (
            PresentationPlan::KnownInput(0),
            Presentation::InputKnown,
            (0, 0),
            PresentationPlan::KnownOutput(0),
            Presentation::OutputKnown,
            (0, 0),
        ),
        (
            PresentationPlan::KnownOutput(0),
            Presentation::OutputKnown,
            (0, 0),
            PresentationPlan::KnownInput(0),
            Presentation::InputKnown,
            (0, 0),
        ),
    ] {
        let case = Case::new();
        let mut instance = Pending::prepare(
            &build(),
            source(),
            options().preparation,
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        )
        .unwrap()
        .complete_with_plan_on_control(initial)
        .unwrap();
        let child = instance.child_id_on_control();
        assert_eq!(instance.ready().presentation, expected);
        assert_eq!(ports(&instance.ready().initial_state), first_ports);
        let block = request(instance.configuration(), 0);
        assert_eq!(
            instance.exchange_on_worker(block).unwrap().pcm[0],
            [block.pcm[0][0] * 0.5, block.pcm[0][1] * 0.5]
        );
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let binding = stopped.configuration().binding;
        let mut instance = stopped
            .resume_retained_on_control(
                Binding {
                    epoch: binding.epoch + 1,
                    ..binding
                },
                next,
            )
            .unwrap();
        assert_eq!(instance.child_id_on_control(), child);
        assert_eq!(instance.ready().presentation, next_expected);
        let block = request(instance.configuration(), 1);
        assert_eq!(
            instance.exchange_on_worker(block).unwrap().pcm[0],
            [block.pcm[0][0] * 0.625, block.pcm[0][1] * 0.625]
        );
        let (stopped, captured) = instance
            .quiesce_retained_on_control()
            .unwrap()
            .capture_state_retained_on_control()
            .unwrap();
        assert_eq!(ports(&captured), next_ports);
        let retired = stopped.retire_on_control().unwrap();
        assert!(retired.status.success());
        assert_eq!(retired.terminal.unwrap().blocks, 2);
        assert_eq!(
            case.observation().unwrap()[2],
            1,
            "original component must not be recreated"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn mixed_directions_preserve_optional_absence_and_setter_refusal() {
    for plan in [
        PresentationPlan::KnownInput(77),
        PresentationPlan::KnownOutput(91),
    ] {
        for variant in [112, 17] {
            let case = Case::new();
            let pending = Pending::prepare(
                &build(),
                source(),
                options().preparation,
                PreparationCancellation::new_on_control().unwrap(),
                case.setup(variant),
            )
            .unwrap();
            if variant == 112 {
                let instance = pending.complete_with_plan_on_control(plan).unwrap();
                assert_eq!(instance.ready().presentation, Presentation::Absent);
                let stopped = instance.quiesce_retained_on_control().unwrap();
                let binding = stopped.configuration().binding;
                let instance = stopped
                    .resume_retained_on_control(
                        Binding {
                            epoch: binding.epoch + 1,
                            ..binding
                        },
                        plan,
                    )
                    .unwrap();
                assert_eq!(instance.ready().presentation, Presentation::Absent);
                assert!(
                    instance
                        .quiesce_retained_on_control()
                        .unwrap()
                        .retire_on_control()
                        .unwrap()
                        .status
                        .success()
                );
            } else {
                assert!(pending.complete_with_plan_on_control(plan).is_err());
                let observed = case.observation().unwrap();
                assert_eq!((observed[8], observed[10]), (0, 0));
            }
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn mixed_direction_wire_refuses_bad_mode_values_binding_and_extent_before_sdk() {
    for corruption in 0..5 {
        let case = Case::new();
        let mut pending = Pending::prepare(
            &build(),
            source(),
            options().preparation,
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        )
        .unwrap();
        let plan = if corruption == 1 {
            PresentationPlan::KnownOutput(91)
        } else {
            PresentationPlan::KnownInput(77)
        };
        let mut wire =
            protocol::presentation_request(pending.configuration().binding, plan).unwrap();
        match corruption {
            0 => wire[51] = 1,
            1 => wire[47] = 1,
            2 => wire[46] = 5,
            3 => wire[14] ^= 1,
            _ => {
                wire.push(0);
                wire[8..12].copy_from_slice(&44_u32.to_le_bytes());
            }
        }
        let until = Instant::now() + Duration::from_secs(3);
        pending
            .lease
            .channel
            .send(&wire, until, &mut || Ok(()))
            .unwrap();
        pending
            .lease
            .channel
            .read(&mut pending.packet, until, &mut || Ok(()))
            .unwrap();
        assert_eq!(pending.packet.magic(), b"HXVIER01");
        assert_eq!(
            protocol::error(
                pending.packet.body().unwrap(),
                pending.configuration().binding
            )
            .unwrap(),
            8 // protocol refusal precedes any SDK setter
        );
        assert!(
            !pending
                .cancel_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
        let observed = case.observation().unwrap();
        assert_eq!((observed[8], observed[10]), (0, 0));
    }
}
