//! Exact direction receipts, including known zero beside the SDK unknown sentinel.
use super::*;

#[test]
fn mixed_directions_preserve_known_zero_and_reject_changed_receipts() {
    let config = config();
    for (plan, mode, expected) in [
        (PresentationPlan::KnownInput(0), 3, Presentation::InputKnown),
        (
            PresentationPlan::KnownInput(77),
            3,
            Presentation::InputKnown,
        ),
        (
            PresentationPlan::KnownOutput(0),
            4,
            Presentation::OutputKnown,
        ),
        (
            PresentationPlan::KnownOutput(91),
            4,
            Presentation::OutputKnown,
        ),
    ] {
        let value = plan.wire_values().unwrap();
        let wire = presentation_request(config.binding, plan).unwrap();
        assert_eq!(wire.len(), 55);
        assert_eq!(wire[46], mode);
        assert_eq!(&wire[47..51], value.input_frames.to_le_bytes());
        assert_eq!(&wire[51..55], value.output_frames.to_le_bytes());
        let opposite = if mode == 3 {
            PresentationPlan::KnownOutput(0)
        } else {
            PresentationPlan::KnownInput(0)
        };
        let mut ready = ready_body(&config);
        ready[70..72].copy_from_slice(&[mode, mode]);
        ready[72..76].copy_from_slice(&value.input_frames.to_le_bytes());
        ready[76..80].copy_from_slice(&value.output_frames.to_le_bytes());
        assert_eq!(
            Ready::decode(&ready, &config, plan).unwrap().presentation,
            expected
        );
        assert!(Ready::decode(&ready, &config, PresentationPlan::Known(value)).is_err());
        assert!(Ready::decode(&ready, &config, opposite).is_err());
        for offset in [0, 70, 71, 72, 76] {
            let mut bad = ready.clone();
            bad[offset] ^= 0x80;
            assert!(Ready::decode(&bad, &config, plan).is_err());
        }
        for end in 70..80 {
            assert!(Ready::decode(&ready[..end], &config, plan).is_err());
        }
        let mut trailing = ready.clone();
        trailing.push(0);
        assert!(Ready::decode(&trailing, &config, plan).is_err());
        let next = Binding {
            epoch: config.binding.epoch + 1,
            ..config.binding
        };
        let request = resume_request(&config, next, plan).unwrap();
        assert_eq!(request.len(), 72);
        assert_eq!(request[10], mode);
        assert_eq!(&request[64..], &wire[47..]);
        let mut resumed_config = config.clone();
        resumed_config.binding = next;
        let mut ack = epoch_body(&resumed_config, 3);
        let facts = quiescent_facts(&ack, &resumed_config, 3).unwrap();
        let at = ack.len();
        ack.extend([mode, mode]);
        ack.extend(&wire[47..]);
        assert_eq!(
            resumed(&ack, &resumed_config, 3, facts, plan).unwrap(),
            expected
        );
        for offset in [0, at, at + 1, at + 2, at + 6] {
            let mut bad = ack.clone();
            bad[offset] ^= 0x80;
            assert!(resumed(&bad, &resumed_config, 3, facts, plan).is_err());
        }
        let mut changed = facts;
        changed.algorithmic_latency_frames += 1;
        assert!(resumed(&ack, &resumed_config, 3, changed, plan).is_err());
        assert!(resumed(&ack, &resumed_config, 2, facts, plan).is_err());
        ack.push(0);
        assert!(resumed(&ack, &resumed_config, 3, facts, plan).is_err());
    }
}
