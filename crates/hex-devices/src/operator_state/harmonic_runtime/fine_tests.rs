use super::*;

#[test]
fn fine_offset_retarget_preserves_phase_envelope_and_occurrence_but_updates_pitch() {
    for global_active in [false, true] {
        let mut settings = OperatorSettings::defaults();
        settings.global.active = global_active;
        let make = |settings: OperatorSettings| {
            Factory::new(
                settings,
                settings.prepare_configuration(48_000).unwrap(),
                crate::shared_sine_table(),
                48_000,
            )
            .unwrap()
        };
        let factory = make(settings);
        let mut source = factory.create_runtime();
        let events = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 19,
            pitch: 69,
            velocity: 127,
        }];
        source
            .preflight_events(&events, 64, &hex_lattice::EMPTY_DEVICE_RESOURCES)
            .unwrap();
        source.process_block(
            &events,
            &mut [[0.0; 2]; 64],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        settings
            .fine
            .patch(parameter::fine_offset(1), ParameterValue::Float(150.0))
            .unwrap();
        let next = make(settings);
        let InstrumentHistoryRetargetPreparation::Prepared(proof) =
            next.prepare_history_retarget_target().unwrap()
        else {
            panic!("retarget proof")
        };
        let mut target = next.create_runtime();
        assert!(!target.can_copy_history_from(source.as_ref()));
        assert!(target.can_retarget_history_from(source.as_ref(), proof.as_ref()));
        target.retarget_history_from(source.as_ref(), proof.as_ref());
        let old = source.as_any().downcast_ref::<Runtime>().unwrap();
        let new = target.as_any().downcast_ref::<Runtime>().unwrap();
        let a = old.voices[0];
        let b = new.voices[0];
        assert_eq!(a.id, b.id);
        assert_eq!(a.age, b.age);
        assert_eq!(a.envelope_age, b.envelope_age);
        assert_eq!(a.phases, b.phases);
        assert_eq!(a.gate, b.gate);
        let before = PreparedOperatorConfiguration::lfo_steps(&a.note);
        let after = PreparedOperatorConfiguration::lfo_steps(&b.note);
        assert_ne!(before[0], after[0]);
        assert_eq!(before[1..], after[1..]);
        assert_eq!(
            after,
            PreparedOperatorConfiguration::lfo_steps(
                &new.factory.configuration.prepare_note(69, 127).unwrap()
            )
        );
    }
}
