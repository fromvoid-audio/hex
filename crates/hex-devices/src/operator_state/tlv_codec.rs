//! Version 18 structured TLV-framed state codec for `hex.instrument.operator` (Vector).
//!
//! Replaces the positional v1..v17 byte layout with an explicitly framed section
//! container (`HXOP` magic + format version + section count + strictly ascending
//! `(tag: u16, len: u32, payload)` entries). Historical v1..v17 payloads continue
//! to decode losslessly through `OperatorSettings::from_legacy_positional`.
use super::*;

pub(super) const MAGIC: [u8; 4] = *b"HXOP";
pub(super) const FORMAT_VERSION: u16 = 1;
const HEADER_BYTES: usize = 8;
const ENTRY_HEADER_BYTES: usize = 6;
const MAX_TLV_BYTES: usize = 16 * 1024;

pub(super) const TAG_CORE_FIELDS: u16 = 1;
pub(super) const TAG_WAVEFORMS: u16 = 2;
pub(super) const TAG_TUNING: u16 = 3;
pub(super) const TAG_AMPLITUDE_VELOCITY: u16 = 4;
pub(super) const TAG_FEEDBACK: u16 = 5;
pub(super) const TAG_LFO: u16 = 6;
pub(super) const TAG_ENABLED: u16 = 7;
pub(super) const TAG_FILTER: u16 = 8;
pub(super) const TAG_HARMONICS: u16 = 9;
pub(super) const TAG_COMPANION: u16 = 10;
pub(super) const TAG_GLOBAL: u16 = 11;
pub(super) const TAG_MODULATION: u16 = 12;
pub(super) const TAG_FINE: u16 = 13;
pub(super) const TAG_KEYTRACK: u16 = 14;
pub(super) const TAG_PERFORMANCE: u16 = 15;
pub(super) const TAG_PITCH_PERF: u16 = 16;
const V18_SECTION_COUNT: u16 = 14;
const V19_SECTION_COUNT: u16 = 15;
const SECTION_COUNT: u16 = 16;
#[cfg(test)]
pub(super) const CANONICAL_BYTES: usize = HEADER_BYTES
    + SECTION_COUNT as usize * ENTRY_HEADER_BYTES
    + PAYLOAD_BYTES
    + performance::BYTES
    + pitch_perf::BYTES;

pub(super) fn encode(settings: &OperatorSettings) -> Result<Vec<u8>, DeviceContractError> {
    let total_bytes = HEADER_BYTES
        + usize::from(SECTION_COUNT) * ENTRY_HEADER_BYTES
        + PAYLOAD_BYTES
        + performance::BYTES
        + pitch_perf::BYTES;
    if total_bytes > MAX_TLV_BYTES {
        return Err(DeviceContractError::StateTooLarge);
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total_bytes)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&SECTION_COUNT.to_le_bytes());
    write_v18_sections(&mut out, settings);
    write_section(&mut out, TAG_PERFORMANCE, performance::BYTES, |buf| {
        settings.performance.append(buf);
    });
    write_section(&mut out, TAG_PITCH_PERF, pitch_perf::BYTES, |buf| {
        settings.pitch_perf.append(buf);
    });
    debug_assert_eq!(out.len(), total_bytes);
    Ok(out)
}

#[cfg(test)]
pub(super) fn encode_v19(settings: &OperatorSettings) -> Result<Vec<u8>, DeviceContractError> {
    let total_bytes = HEADER_BYTES
        + usize::from(V19_SECTION_COUNT) * ENTRY_HEADER_BYTES
        + PAYLOAD_BYTES
        + performance::BYTES;
    let mut out = Vec::new();
    out.try_reserve_exact(total_bytes)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&V19_SECTION_COUNT.to_le_bytes());
    write_v18_sections(&mut out, settings);
    write_section(&mut out, TAG_PERFORMANCE, performance::BYTES, |buf| {
        settings.performance.append(buf);
    });
    debug_assert_eq!(out.len(), total_bytes);
    Ok(out)
}

#[cfg(test)]
pub(super) fn encode_v18(settings: &OperatorSettings) -> Result<Vec<u8>, DeviceContractError> {
    let total_bytes =
        HEADER_BYTES + usize::from(V18_SECTION_COUNT) * ENTRY_HEADER_BYTES + PAYLOAD_BYTES;
    let mut out = Vec::new();
    out.try_reserve_exact(total_bytes)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&V18_SECTION_COUNT.to_le_bytes());
    write_v18_sections(&mut out, settings);
    debug_assert_eq!(out.len(), total_bytes);
    Ok(out)
}

fn write_v18_sections(out: &mut Vec<u8>, settings: &OperatorSettings) {
    write_section(out, TAG_CORE_FIELDS, LEGACY_PAYLOAD_BYTES, |buf| {
        for field in settings.fields {
            buf.extend_from_slice(&field.to_le_bytes());
        }
    });
    write_section(
        out,
        TAG_WAVEFORMS,
        WAVEFORM_COUNT * size_of::<u32>(),
        |buf| {
            for waveform in settings.waveforms {
                buf.extend_from_slice(&waveform.choice().to_le_bytes());
            }
        },
    );
    write_section(out, TAG_TUNING, tuning::BYTES, |buf| {
        settings.tuning.append(buf);
    });
    write_section(
        out,
        TAG_AMPLITUDE_VELOCITY,
        VELOCITY_SENSITIVITY_COUNT * size_of::<f32>(),
        |buf| {
            for amount in settings.amplitude_velocity {
                buf.extend_from_slice(&amount.to_le_bytes());
            }
        },
    );
    write_section(
        out,
        TAG_FEEDBACK,
        FEEDBACK_COUNT * size_of::<f32>(),
        |buf| {
            for amount in settings.feedback {
                buf.extend_from_slice(&amount.to_le_bytes());
            }
        },
    );
    write_section(out, TAG_LFO, lfo::BYTES, |buf| {
        settings.lfo.append(buf);
    });
    write_section(out, TAG_ENABLED, enable::BYTES, |buf| {
        settings.enabled.append(buf);
    });
    write_section(out, TAG_FILTER, crate::operator_filter::BYTES, |buf| {
        settings.filter.append(buf);
    });
    write_section(out, TAG_HARMONICS, harmonics::BYTES, |buf| {
        settings.harmonics.append(buf);
    });
    write_section(out, TAG_COMPANION, companion::BYTES, |buf| {
        settings.companion.append(buf);
    });
    write_section(out, TAG_GLOBAL, global::BYTES, |buf| {
        settings.global.append(buf);
    });
    write_section(out, TAG_MODULATION, modulation::BYTES, |buf| {
        settings.modulation.append(buf);
    });
    write_section(out, TAG_FINE, fine::BYTES, |buf| {
        settings.fine.append(buf);
    });
    write_section(out, TAG_KEYTRACK, keytrack::BYTES, |buf| {
        settings.pitch_keytrack.append(buf);
    });
}

fn write_section(
    out: &mut Vec<u8>,
    tag: u16,
    expected_len: usize,
    writer: impl FnOnce(&mut Vec<u8>),
) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&(expected_len as u32).to_le_bytes());
    let start = out.len();
    writer(out);
    debug_assert_eq!(out.len() - start, expected_len);
}

pub(super) fn decode(
    bytes: &[u8],
    state_version: u32,
) -> Result<OperatorSettings, DeviceContractError> {
    let max_sections = match state_version {
        TLV_STATE_VERSION => V18_SECTION_COUNT,
        PERFORMANCE_STATE_VERSION => V19_SECTION_COUNT,
        PITCH_PERF_STATE_VERSION => SECTION_COUNT,
        _ => return Err(DeviceContractError::UnsupportedStateVersion),
    };
    if bytes.len() < HEADER_BYTES || bytes.len() > MAX_TLV_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    if bytes[..4] != MAGIC {
        return Err(DeviceContractError::MalformedState);
    }
    let format = u16::from_le_bytes([bytes[4], bytes[5]]);
    if format != FORMAT_VERSION {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let section_count = u16::from_le_bytes([bytes[6], bytes[7]]);
    if section_count == 0 || section_count > max_sections {
        return Err(DeviceContractError::MalformedState);
    }

    let mut cursor = HEADER_BYTES;
    let mut last_tag: u16 = 0;
    let mut saw_core = false;
    let mut saw_keytrack = false;

    let mut fields = [0.0_f32; FIELD_COUNT];
    let mut waveforms = [OperatorWaveform::Sine; OPERATORS];
    let mut tuning = tuning::TuningState::default();
    let mut amplitude_velocity = [0.0_f32; OPERATORS];
    let mut feedback = [0.0_f32; OPERATORS];
    let mut lfo = lfo::LfoSettings::default();
    let mut enabled = enable::EnableSettings::default();
    let mut filter = crate::operator_filter::Settings::default();
    let mut harmonics = harmonics::Harmonics::default();
    let mut companion = companion::Settings::default();
    let mut global = global::Settings::default();
    let mut modulation = modulation::Settings::default();
    let mut fine = fine::Settings::default();
    let mut pitch_keytrack = keytrack::Settings::default();
    let mut performance = performance::Settings::default();
    let mut pitch_perf = pitch_perf::Settings::default();

    for _ in 0..section_count {
        let header = bytes
            .get(cursor..cursor + ENTRY_HEADER_BYTES)
            .ok_or(DeviceContractError::MalformedState)?;
        let tag = u16::from_le_bytes([header[0], header[1]]);
        let len = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        if tag <= last_tag {
            return Err(DeviceContractError::MalformedState);
        }
        last_tag = tag;
        cursor = cursor
            .checked_add(ENTRY_HEADER_BYTES)
            .ok_or(DeviceContractError::MalformedState)?;
        let end = cursor
            .checked_add(len)
            .ok_or(DeviceContractError::MalformedState)?;
        let section = bytes
            .get(cursor..end)
            .ok_or(DeviceContractError::MalformedState)?;
        cursor = end;

        match tag {
            TAG_CORE_FIELDS => {
                if section.len() != LEGACY_PAYLOAD_BYTES {
                    return Err(DeviceContractError::MalformedState);
                }
                for (dst, chunk) in fields
                    .iter_mut()
                    .zip(section.chunks_exact(size_of::<f32>()))
                {
                    let raw: [u8; 4] = chunk
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    *dst = f32::from_le_bytes(raw);
                }
                saw_core = true;
            }
            TAG_WAVEFORMS => {
                if section.len() != WAVEFORM_COUNT * size_of::<u32>() {
                    return Err(DeviceContractError::MalformedState);
                }
                for (dst, chunk) in waveforms
                    .iter_mut()
                    .zip(section.chunks_exact(size_of::<u32>()))
                {
                    let raw: [u8; 4] = chunk
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    *dst = OperatorWaveform::from_choice(u32::from_le_bytes(raw))
                        .ok_or(DeviceContractError::MalformedState)?;
                }
            }
            TAG_TUNING => {
                tuning = tuning::TuningState::decode(section)?;
            }
            TAG_AMPLITUDE_VELOCITY => {
                if section.len() != VELOCITY_SENSITIVITY_COUNT * size_of::<f32>() {
                    return Err(DeviceContractError::MalformedState);
                }
                for (dst, chunk) in amplitude_velocity
                    .iter_mut()
                    .zip(section.chunks_exact(size_of::<f32>()))
                {
                    let raw: [u8; 4] = chunk
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    *dst = f32::from_le_bytes(raw);
                }
            }
            TAG_FEEDBACK => {
                if section.len() != FEEDBACK_COUNT * size_of::<f32>() {
                    return Err(DeviceContractError::MalformedState);
                }
                for (dst, chunk) in feedback
                    .iter_mut()
                    .zip(section.chunks_exact(size_of::<f32>()))
                {
                    let raw: [u8; 4] = chunk
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?;
                    *dst = f32::from_le_bytes(raw);
                }
            }
            TAG_LFO => {
                lfo = lfo::LfoSettings::decode(section)?;
            }
            TAG_ENABLED => {
                enabled = enable::EnableSettings::decode(section)?;
            }
            TAG_FILTER => {
                filter = crate::operator_filter::Settings::decode(section)?;
            }
            TAG_HARMONICS => {
                harmonics = harmonics::Harmonics::decode(section)?;
            }
            TAG_COMPANION => {
                companion = companion::Settings::decode(section)?;
            }
            TAG_GLOBAL => {
                global = global::Settings::decode(section)?;
            }
            TAG_MODULATION => {
                modulation = modulation::Settings::decode(section)?;
            }
            TAG_FINE => {
                fine = fine::Settings::decode(section)?;
            }
            TAG_KEYTRACK => {
                pitch_keytrack = keytrack::Settings::decode(section)?;
                saw_keytrack = true;
            }
            TAG_PERFORMANCE if state_version >= PERFORMANCE_STATE_VERSION => {
                performance = performance::Settings::decode(section)?;
            }
            TAG_PITCH_PERF if state_version >= PITCH_PERF_STATE_VERSION => {
                pitch_perf = pitch_perf::Settings::decode(section)?;
            }
            _ => return Err(DeviceContractError::MalformedState),
        }
    }

    if cursor != bytes.len() || !saw_core {
        return Err(DeviceContractError::MalformedState);
    }
    if !saw_keytrack {
        pitch_keytrack = keytrack::Settings::from_fixed(tuning.fixed_flags());
    }
    if tuning.fixed_flags() != pitch_keytrack.fixed_flags() {
        return Err(DeviceContractError::MalformedState);
    }
    let result = OperatorSettings {
        fields,
        waveforms,
        tuning,
        amplitude_velocity,
        feedback,
        lfo,
        enabled,
        harmonics,
        filter,
        companion,
        global,
        modulation,
        fine,
        pitch_keytrack,
        performance,
        pitch_perf,
    };
    result.kernel_parts()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich_settings() -> OperatorSettings {
        let mut state = OperatorSettings::defaults().to_state().unwrap();
        for (id, value) in [
            (parameter::oscillator_ratio(1), ParameterValue::Float(2.5)),
            (parameter::oscillator_level(1), ParameterValue::Float(0.85)),
            (parameter::oscillator_level(2), ParameterValue::Float(0.6)),
            (parameter::waveform(2), ParameterValue::Choice(3)),
            (parameter::fixed_frequency(3), ParameterValue::Float(330.0)),
            (parameter::amplitude_velocity(1), ParameterValue::Float(0.4)),
            (parameter::feedback(4), ParameterValue::Float(0.35)),
            (parameter::LFO_ENABLED, ParameterValue::Bool(true)),
            (parameter::LFO_RATE, ParameterValue::Float(6.25)),
            (parameter::LFO_AMOUNT, ParameterValue::Float(18.0)),
            (parameter::enabled(3), ParameterValue::Bool(false)),
            (ParameterId::from_static(20_001), ParameterValue::Bool(true)),
            (automation::FILTER_FREQUENCY, ParameterValue::Float(2_400.0)),
            (automation::FILTER_RESONANCE, ParameterValue::Float(35.0)),
            (parameter::fine_offset(1), ParameterValue::Float(-12.5)),
            (parameter::pitch_keytrack(2), ParameterValue::Float(50.0)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, id, value)
                .unwrap();
        }
        OperatorSettings::from_state(&state).unwrap()
    }

    #[test]
    fn tlv_roundtrip_and_v1_through_v18_lossless_migration() {
        let rich = rich_settings();
        let v19 = rich.to_state().unwrap();
        assert_eq!(v19.state_version(), OPERATOR_STATE_VERSION);
        assert_eq!(&v19.payload()[..4], &MAGIC);
        assert_eq!(v19.payload().len(), CANONICAL_BYTES);
        assert_eq!(OperatorSettings::from_state(&v19).unwrap(), rich);

        // Historical v18 TLV migrates cleanly to v19 with neutral performance defaults.
        let v18 = NativeDeviceState::new(
            OPERATOR_TYPE_ID,
            TLV_STATE_VERSION,
            encode_v18(&rich).unwrap(),
        )
        .unwrap();
        let migrated_v18 = OPERATOR_METADATA.migrate_state(&v18).unwrap();
        assert_eq!(migrated_v18, v19);
        assert_eq!(OperatorSettings::from_state(&migrated_v18).unwrap(), rich);
        // Tag 15 is rejected if presented inside a v18 state.
        assert!(decode(v19.payload(), TLV_STATE_VERSION).is_err());

        // Lossless v17 -> v19 migration preserves every field and exact PCM.
        let v17 = rich.to_v17_state().unwrap();
        assert_eq!(v17.state_version(), KEYTRACK_STATE_VERSION);
        let migrated = OPERATOR_METADATA.migrate_state(&v17).unwrap();
        assert_eq!(migrated, v19);
        assert_eq!(OperatorSettings::from_state(&migrated).unwrap(), rich);

        // Historical v1..v17 prefixes all migrate cleanly into canonical v19 TLV.
        let base_v17 = OperatorSettings::from_state(&super::super::tests::legacy_state())
            .unwrap()
            .to_v17_state()
            .unwrap();
        for (version, len) in [
            (1, LEGACY_PAYLOAD_BYTES),
            (2, WAVEFORM_PAYLOAD_BYTES),
            (3, TUNING_PAYLOAD_BYTES),
            (4, VELOCITY_PAYLOAD_BYTES),
            (5, FEEDBACK_PAYLOAD_BYTES),
            (6, FEEDBACK_PAYLOAD_BYTES),
            (7, LFO_PAYLOAD_BYTES),
            (8, ENABLE_PAYLOAD_BYTES),
            (11, FILTER_PAYLOAD_BYTES),
            (12, HARMONIC_PAYLOAD_BYTES),
            (13, HARMONIC_PAYLOAD_BYTES),
            (14, COMPANION_PAYLOAD_BYTES),
            (15, MODULATION_PAYLOAD_BYTES),
            (16, FINE_PAYLOAD_BYTES),
            (17, PAYLOAD_BYTES),
        ] {
            let historical = NativeDeviceState::new(
                OPERATOR_TYPE_ID,
                version,
                base_v17.payload()[..len].to_vec(),
            )
            .unwrap();
            let decoded_old = OperatorSettings::from_state(&historical).unwrap();
            let migrated = OPERATOR_METADATA.migrate_state(&historical).unwrap();
            assert_eq!(migrated.state_version(), OPERATOR_STATE_VERSION);
            assert_eq!(
                OperatorSettings::from_state(&migrated).unwrap(),
                decoded_old
            );
        }
    }

    #[test]
    fn tlv_rejects_bad_magic_version_order_duplicates_unknown_tags_and_truncation() {
        let valid = OperatorSettings::defaults().to_state().unwrap();
        let bytes = valid.payload().to_vec();

        // Bad magic
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'Z';
        assert!(decode(&bad_magic, OPERATOR_STATE_VERSION).is_err());

        // Unsupported TLV container format version
        let mut bad_fmt = bytes.clone();
        bad_fmt[4..6].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            decode(&bad_fmt, OPERATOR_STATE_VERSION).unwrap_err(),
            DeviceContractError::UnsupportedStateVersion
        );

        // Truncated header & truncated payload
        assert!(decode(&bytes[..4], OPERATOR_STATE_VERSION).is_err());
        assert!(decode(&bytes[..bytes.len() - 1], OPERATOR_STATE_VERSION).is_err());

        // Trailing garbage byte
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing, OPERATOR_STATE_VERSION).is_err());

        // Unknown tag
        let mut unknown = Vec::new();
        unknown.extend_from_slice(&MAGIC);
        unknown.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        unknown.extend_from_slice(&2_u16.to_le_bytes());
        // Valid core section followed by unknown tag 99.
        unknown.extend_from_slice(
            &bytes[HEADER_BYTES..HEADER_BYTES + ENTRY_HEADER_BYTES + LEGACY_PAYLOAD_BYTES],
        );
        unknown.extend_from_slice(&99_u16.to_le_bytes());
        unknown.extend_from_slice(&4_u32.to_le_bytes());
        unknown.extend_from_slice(&0_u32.to_le_bytes());
        assert!(decode(&unknown, OPERATOR_STATE_VERSION).is_err());

        // Duplicate tag
        let mut dup = Vec::new();
        dup.extend_from_slice(&MAGIC);
        dup.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        dup.extend_from_slice(&2_u16.to_le_bytes());
        let core_entry =
            &bytes[HEADER_BYTES..HEADER_BYTES + ENTRY_HEADER_BYTES + LEGACY_PAYLOAD_BYTES];
        dup.extend_from_slice(core_entry);
        dup.extend_from_slice(core_entry);
        assert!(decode(&dup, OPERATOR_STATE_VERSION).is_err());

        // Missing required core section (starts at tag 2)
        let mut no_core = Vec::new();
        no_core.extend_from_slice(&MAGIC);
        no_core.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        no_core.extend_from_slice(&1_u16.to_le_bytes());
        no_core.extend_from_slice(&TAG_WAVEFORMS.to_le_bytes());
        no_core.extend_from_slice(&16_u32.to_le_bytes());
        no_core.extend_from_slice(&[0_u8; 16]);
        assert!(decode(&no_core, OPERATOR_STATE_VERSION).is_err());
    }
}
