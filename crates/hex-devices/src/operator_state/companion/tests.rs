use super::*;
use crate::fm4::PreparedOperatorNote;
#[test]
fn suffix_migration_preserves_every_prefix_and_catalog_version_boundary() {
    let current = crate::operator_state::tests::legacy_state();
    assert_eq!(current.payload().len(), 1744);
    assert_eq!(OPERATOR_METADATA.descriptors().len(), 428);
    for (version, length) in [
        (1, 224),
        (2, 240),
        (3, 288),
        (4, 304),
        (5, 320),
        (6, 320),
        (7, 364),
        (8, 380),
        (11, 416),
        (12, 1488),
        (13, 1488),
        (14, 1616),
    ] {
        let old = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            version,
            current.payload()[..length].to_vec(),
        )
        .unwrap();
        let migrated = OPERATOR_METADATA.migrate_state(&old).unwrap();
        assert_eq!(&migrated.payload()[..length], old.payload());
        assert_eq!(migrated, current);
    }
    let catalog = OPERATOR_METADATA
        .patch_parameter(&current, parameter::waveform(1), ParameterValue::Choice(22))
        .unwrap();
    let v13 =
        NativeDeviceState::new(OPERATOR_TYPE_ID, 13, catalog.payload()[..1488].to_vec()).unwrap();
    assert_eq!(
        OPERATOR_METADATA
            .read_parameter(
                &OPERATOR_METADATA.migrate_state(&v13).unwrap(),
                parameter::waveform(1)
            )
            .unwrap(),
        ParameterValue::Choice(22)
    );
    let v12 = NativeDeviceState::new(OPERATOR_TYPE_ID, 12, v13.payload().to_vec()).unwrap();
    assert!(OPERATOR_METADATA.migrate_state(&v12).is_err());
    for length in [1615, 1617] {
        let mut bytes = current.payload().to_vec();
        bytes.resize(length, 0);
        assert!(
            OPERATOR_METADATA
                .migrate_state(&NativeDeviceState::new(OPERATOR_TYPE_ID, 14, bytes).unwrap())
                .is_err()
        );
    }
    for (field, word) in [
        (0, 2),
        (1, f32::NAN.to_bits()),
        (2, 2),
        (3, f32::INFINITY.to_bits()),
        (4, 1.01_f32.to_bits()),
        (5, 5),
        (6, 0),
        (7, 11),
    ] {
        let mut bytes = current.payload()[..1616].to_vec();
        let offset = 1488 + field * 4;
        bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        assert!(
            OPERATOR_METADATA
                .migrate_state(&NativeDeviceState::new(OPERATOR_TYPE_ID, 14, bytes).unwrap())
                .is_err()
        );
    }
}
#[test]
fn key_scales_and_velocity_pitch_quantization_match_independent_oracles() {
    let mut settings = OperatorSettings::defaults();
    for i in 0..4 {
        settings.fields[i * 3] = 1.0 / 128.0;
    }
    settings.amplitude_velocity[0] = 0.375;
    settings.companion.words[0][4] = 1.0_f32.to_bits();
    let configuration = settings.prepare_configuration(48000).unwrap();
    for (pitch, velocity) in [(0, 1), (60, 64), (84, 127), (127, 127)] {
        let note = configuration.prepare_note(pitch, velocity).unwrap();
        let payload = note.into_shared_payload();
        let expected = ((1.0 - f64::from(settings.amplitude_velocity[0])
            + f64::from(settings.amplitude_velocity[0]) * f64::from(velocity) / 127.0)
            * 2.0_f64
                .powf((f64::from(pitch) - 60.0) / 12.0)
                .clamp(1.0 / 64.0, 8.0)) as f32;
        assert_eq!(f32::from_bits(payload[6] as u32), expected);
        let decoded = PreparedOperatorNote::from_shared_payload(&payload).unwrap();
        assert!(configuration.note_matches_configuration(&decoded));
        assert!(
            !OperatorSettings::defaults()
                .prepare_configuration(48000)
                .unwrap()
                .note_matches_configuration(&decoded)
        );
        assert_ne!(payload[5] & (1 << 30), 0);
        if expected > 1.0 {
            let mut unmarked = payload;
            unmarked[5] &= !(1 << 30);
            assert!(PreparedOperatorNote::from_shared_payload(&unmarked).is_none());
        }
        let mut overflow = payload;
        overflow[6] = (overflow[6] & !u64::from(u32::MAX)) | u64::from(8.01_f32.to_bits());
        assert!(PreparedOperatorNote::from_shared_payload(&overflow).is_none());
        let mut wrong_operator = payload;
        wrong_operator[7] =
            (wrong_operator[7] & !u64::from(u32::MAX)) | u64::from(4.0_f32.to_bits());
        assert!(!configuration.note_matches_configuration(
            &PreparedOperatorNote::from_shared_payload(&wrong_operator).unwrap()
        ));
        assert_eq!(payload.len(), 8);
    }
    settings = OperatorSettings::defaults();
    settings.fields[0] = 1.25;
    let neutral = settings
        .prepare_configuration(48000)
        .unwrap()
        .prepare_note(60, 127)
        .unwrap()
        .into_shared_payload();
    settings.companion.words[0][2] = 1;
    assert_eq!(
        neutral,
        settings
            .prepare_configuration(48000)
            .unwrap()
            .prepare_note(60, 127)
            .unwrap()
            .into_shared_payload(),
        "q with zero osc<vel preserves the exact path"
    );
    settings.companion.words[0][1] = 0.25_f32.to_bits();
    let note = settings
        .prepare_configuration(48000)
        .unwrap()
        .prepare_note(60, 127)
        .unwrap();
    let hz = 440.0 * 2.0_f64.powf((60.0 - 69.0) / 12.0) * 3.0;
    assert_eq!(
        note.into_shared_payload()[0],
        (hz / 48000.0 * crate::PHASE_SCALE).round() as u64
    );
    settings
        .tuning
        .patch(ParameterId::from_static(62), ParameterValue::Bool(true))
        .unwrap();
    settings
        .tuning
        .patch(ParameterId::from_static(63), ParameterValue::Float(222.0))
        .unwrap();
    settings
        .tuning
        .patch(ParameterId::from_static(64), ParameterValue::Float(1.25))
        .unwrap();
    assert_eq!(
        settings
            .prepare_configuration(48000)
            .unwrap()
            .prepare_note(60, 127)
            .unwrap()
            .into_shared_payload()[0],
        (666.0 / 48000.0 * crate::PHASE_SCALE).round() as u64
    );
    settings = OperatorSettings::defaults();
    settings.fields[0] = 16.0;
    settings.companion.words[0][1] = 1.0_f32.to_bits();
    assert!(
        settings
            .prepare_configuration(48000)
            .unwrap()
            .prepare_note(69, 127)
            .is_ok()
    );
    settings.fields[0] = 128.0;
    settings.companion.words[0][1] = (-1.0_f32).to_bits();
    assert!(
        settings
            .prepare_configuration(48000)
            .unwrap()
            .prepare_note(69, 127)
            .is_ok(),
        "validate frequency after downward modulation"
    );
    let native = settings.prepare_note(48000, 69, 127).unwrap();
    assert!(
        native
            .sample(crate::shared_sine_table(), 1, u64::MAX)
            .is_finite()
    );
}
#[test]
fn time_velocity_keeps_zero_stages_and_clamps_authored_stage_maxima() {
    let mut settings = OperatorSettings::defaults();
    settings.fields[34] = 0.4;
    settings.fields[35] = 0.01;
    settings.fields[36] = 0.02;
    settings.fields[37] = 0.03;
    settings.companion.words[0][3] = 1.0_f32.to_bits();
    let envelopes = settings.companion.envelopes(&settings.fields, 127).unwrap();
    let prepared = envelopes[0].prepare(8000).unwrap();
    assert_eq!(
        (
            prepared.attack_frames(),
            prepared.decay_frames(),
            prepared.release_frames()
        ),
        (5, 10, 15)
    );
    for gate in [0, 2, 5, 8, 15, 30] {
        for age in 0..50 {
            let held = |at: u64| {
                if at < 5 {
                    at as f64 / 5.0
                } else if at < 15 {
                    0.4 + 0.6 * 0.00001_f64.powf((at - 5) as f64 / 10.0)
                } else {
                    0.4
                }
            };
            let expected = if age < gate {
                held(age)
            } else if age - gate >= 15 {
                0.0
            } else {
                held(gate) * 0.00001_f64.powf((age - gate) as f64 / 15.0)
            };
            assert!((prepared.level(age, gate) - expected).abs() < 1e-8);
        }
    }
    settings.fields[35] = 0.0;
    settings.fields[36] = 60.0;
    settings.fields[37] = 60.0;
    settings.companion.words[0][3] = (-1.0_f32).to_bits();
    let prepared = settings.companion.envelopes(&settings.fields, 127).unwrap()[0]
        .prepare(8000)
        .unwrap();
    assert_eq!(
        (
            prepared.attack_frames(),
            prepared.decay_frames(),
            prepared.release_frames()
        ),
        (0, 480000, 480000)
    );
    settings.companion.words[0][0] = 0;
    assert!(matches!(
        settings.prepare_note(48000, 60, 127),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
    settings.companion.words[0][0] = 1;
    settings.companion.words[0][5] = 4;
    assert!(matches!(
        settings.prepare_note(48000, 60, 127),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
}
