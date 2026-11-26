//! Steinberg VST3 `.vstpreset` container codec and canonical state conversion.
//!
//! Pure byte-level control-side codec with no filesystem or SDK ABI dependency.
//! Refuses malformed, truncated, duplicate-chunk, oversize, or class-mismatched
//! presets without mutating caller state or substituting defaults.

use crate::{ClassId, MAX_OPAQUE_BYTES, State, decode, encode};
use hex_lattice_contract::NativeDeviceState;
use std::fmt;

const MAGIC_VST3: &[u8; 4] = b"VST3";
const MAGIC_LIST: &[u8; 4] = b"List";
const CHUNK_COMP: &[u8; 4] = b"Comp";
const CHUNK_CONT: &[u8; 4] = b"Cont";
const CHUNK_INFO: &[u8; 4] = b"Info";

/// Current Steinberg `.vstpreset` container format version.
pub const VSTPRESET_VERSION: i32 = 1;
const HEADER_BYTES: usize = 48;
const LIST_HEADER_BYTES: usize = 8;
const LIST_ENTRY_BYTES: usize = 20;
const MAX_PRESET_CHUNKS: usize = 64;
/// Bounded limit for optional UTF-8 `Info` XML metadata inside a `.vstpreset`.
pub const MAX_PRESET_INFO_BYTES: usize = 256 * 1024;

/// Decoded or to-be-encoded Steinberg `.vstpreset` payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VstPreset<'a> {
    pub class: ClassId,
    pub component: Option<&'a [u8]>,
    pub controller: Option<&'a [u8]>,
    pub info_xml: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VstPresetError {
    Malformed,
    UnsupportedVersion(i32),
    InvalidClassId,
    ClassMismatch,
    DuplicateChunk,
    TooLarge,
    InvalidInfoUtf8,
    MissingComponent,
    StateError(crate::Error),
}

impl fmt::Display for VstPresetError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => out.write_str("vstpreset file is malformed"),
            Self::UnsupportedVersion(_) => out.write_str("vstpreset format version is unsupported"),
            Self::InvalidClassId => out.write_str("vstpreset class id is invalid"),
            Self::ClassMismatch => out.write_str("vstpreset belongs to a different plugin"),
            Self::DuplicateChunk => out.write_str("vstpreset contains duplicate chunks"),
            Self::TooLarge => out.write_str("vstpreset exceeds the bounded state limit"),
            Self::InvalidInfoUtf8 => out.write_str("vstpreset info metadata is not valid utf-8"),
            Self::MissingComponent => out.write_str("vstpreset is missing component state"),
            Self::StateError(error) => fmt::Display::fmt(error, out),
        }
    }
}

impl std::error::Error for VstPresetError {}

impl From<crate::Error> for VstPresetError {
    fn from(error: crate::Error) -> Self {
        Self::StateError(error)
    }
}

fn encode_class_id_hex(class: ClassId) -> [u8; 32] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = [0u8; 32];
    for (index, byte) in class.bytes().iter().enumerate() {
        out[index * 2] = HEX[usize::from(byte >> 4)];
        out[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
    }
    out
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn decode_class_id_hex(bytes: &[u8]) -> Result<ClassId, VstPresetError> {
    if bytes.len() != 32 {
        return Err(VstPresetError::InvalidClassId);
    }
    let mut raw = [0u8; 16];
    for (index, slot) in raw.iter_mut().enumerate() {
        let hi = decode_hex_nibble(bytes[index * 2]).ok_or(VstPresetError::InvalidClassId)?;
        let lo = decode_hex_nibble(bytes[index * 2 + 1]).ok_or(VstPresetError::InvalidClassId)?;
        *slot = (hi << 4) | lo;
    }
    ClassId::new(raw).map_err(|_| VstPresetError::InvalidClassId)
}

/// Encode a validated `VstPreset` into Steinberg's `.vstpreset` binary format.
pub fn encode_vstpreset(preset: VstPreset<'_>) -> Result<Vec<u8>, VstPresetError> {
    let component_len = preset.component.map_or(0, <[u8]>::len);
    let controller_len = preset.controller.map_or(0, <[u8]>::len);
    let state_bytes = component_len
        .checked_add(controller_len)
        .ok_or(VstPresetError::TooLarge)?;
    if state_bytes > MAX_OPAQUE_BYTES {
        return Err(VstPresetError::TooLarge);
    }
    let info_bytes = preset.info_xml.map(str::as_bytes);
    let info_len = info_bytes.map_or(0, <[u8]>::len);
    if info_len > MAX_PRESET_INFO_BYTES {
        return Err(VstPresetError::TooLarge);
    }

    let mut entries: [([u8; 4], usize, usize); 3] = [([0; 4], 0, 0); 3];
    let mut entry_count = 0usize;
    let mut cursor = HEADER_BYTES;

    if let Some(comp) = preset.component {
        entries[entry_count] = (*CHUNK_COMP, cursor, comp.len());
        entry_count += 1;
        cursor = cursor
            .checked_add(comp.len())
            .ok_or(VstPresetError::TooLarge)?;
    }
    if let Some(cont) = preset.controller {
        entries[entry_count] = (*CHUNK_CONT, cursor, cont.len());
        entry_count += 1;
        cursor = cursor
            .checked_add(cont.len())
            .ok_or(VstPresetError::TooLarge)?;
    }
    if let Some(info) = info_bytes {
        entries[entry_count] = (*CHUNK_INFO, cursor, info.len());
        entry_count += 1;
        cursor = cursor
            .checked_add(info.len())
            .ok_or(VstPresetError::TooLarge)?;
    }

    let chunk_list_offset = cursor;
    let total_bytes = chunk_list_offset
        .checked_add(LIST_HEADER_BYTES + entry_count * LIST_ENTRY_BYTES)
        .ok_or(VstPresetError::TooLarge)?;
    let mut out = Vec::with_capacity(total_bytes);

    out.extend_from_slice(MAGIC_VST3);
    out.extend_from_slice(&VSTPRESET_VERSION.to_le_bytes());
    out.extend_from_slice(&encode_class_id_hex(preset.class));
    out.extend_from_slice(&(chunk_list_offset as i64).to_le_bytes());

    if let Some(comp) = preset.component {
        out.extend_from_slice(comp);
    }
    if let Some(cont) = preset.controller {
        out.extend_from_slice(cont);
    }
    if let Some(info) = info_bytes {
        out.extend_from_slice(info);
    }

    out.extend_from_slice(MAGIC_LIST);
    out.extend_from_slice(&(entry_count as i32).to_le_bytes());
    for (id, offset, size) in &entries[..entry_count] {
        out.extend_from_slice(id);
        out.extend_from_slice(&(*offset as i64).to_le_bytes());
        out.extend_from_slice(&(*size as i64).to_le_bytes());
    }

    Ok(out)
}

/// Strictly decode a `.vstpreset` binary payload without allocating or copying chunks.
pub fn decode_vstpreset(bytes: &[u8]) -> Result<VstPreset<'_>, VstPresetError> {
    let header = bytes.get(..HEADER_BYTES).ok_or(VstPresetError::Malformed)?;
    if header.get(..4) != Some(MAGIC_VST3) {
        return Err(VstPresetError::Malformed);
    }
    let version = i32::from_le_bytes(
        header[4..8]
            .try_into()
            .map_err(|_| VstPresetError::Malformed)?,
    );
    if version != VSTPRESET_VERSION {
        return Err(VstPresetError::UnsupportedVersion(version));
    }
    let class = decode_class_id_hex(&header[8..40])?;
    let list_offset_i64 = i64::from_le_bytes(
        header[40..48]
            .try_into()
            .map_err(|_| VstPresetError::Malformed)?,
    );
    if list_offset_i64 < HEADER_BYTES as i64 {
        return Err(VstPresetError::Malformed);
    }
    let list_offset = usize::try_from(list_offset_i64).map_err(|_| VstPresetError::Malformed)?;
    let list_header = bytes
        .get(
            list_offset
                ..list_offset
                    .checked_add(LIST_HEADER_BYTES)
                    .ok_or(VstPresetError::Malformed)?,
        )
        .ok_or(VstPresetError::Malformed)?;
    if list_header.get(..4) != Some(MAGIC_LIST) {
        return Err(VstPresetError::Malformed);
    }
    let count_i32 = i32::from_le_bytes(
        list_header[4..8]
            .try_into()
            .map_err(|_| VstPresetError::Malformed)?,
    );
    if count_i32 < 0 {
        return Err(VstPresetError::Malformed);
    }
    let count = usize::try_from(count_i32).map_err(|_| VstPresetError::Malformed)?;
    if count > MAX_PRESET_CHUNKS {
        return Err(VstPresetError::TooLarge);
    }
    let entries_start = list_offset
        .checked_add(LIST_HEADER_BYTES)
        .ok_or(VstPresetError::Malformed)?;
    let entries_bytes = count
        .checked_mul(LIST_ENTRY_BYTES)
        .ok_or(VstPresetError::Malformed)?;
    let list_end = entries_start
        .checked_add(entries_bytes)
        .ok_or(VstPresetError::Malformed)?;
    if list_end > bytes.len() {
        return Err(VstPresetError::Malformed);
    }

    let mut component: Option<&[u8]> = None;
    let mut controller: Option<&[u8]> = None;
    let mut info_xml: Option<&str> = None;

    for index in 0..count {
        let entry_start = entries_start + index * LIST_ENTRY_BYTES;
        let entry = &bytes[entry_start..entry_start + LIST_ENTRY_BYTES];
        let chunk_id: [u8; 4] = entry[0..4]
            .try_into()
            .map_err(|_| VstPresetError::Malformed)?;
        let offset_i64 = i64::from_le_bytes(
            entry[4..12]
                .try_into()
                .map_err(|_| VstPresetError::Malformed)?,
        );
        let size_i64 = i64::from_le_bytes(
            entry[12..20]
                .try_into()
                .map_err(|_| VstPresetError::Malformed)?,
        );
        if offset_i64 < HEADER_BYTES as i64 || size_i64 < 0 {
            return Err(VstPresetError::Malformed);
        }
        let offset = usize::try_from(offset_i64).map_err(|_| VstPresetError::Malformed)?;
        let size = usize::try_from(size_i64).map_err(|_| VstPresetError::Malformed)?;
        let end = offset.checked_add(size).ok_or(VstPresetError::TooLarge)?;
        if end > bytes.len() {
            return Err(VstPresetError::Malformed);
        }
        // A chunk must not overlap the chunk list table itself.
        if size > 0 && offset < list_end && end > list_offset {
            return Err(VstPresetError::Malformed);
        }
        let slice = &bytes[offset..end];
        match &chunk_id {
            CHUNK_COMP => {
                if component.is_some() {
                    return Err(VstPresetError::DuplicateChunk);
                }
                component = Some(slice);
            }
            CHUNK_CONT => {
                if controller.is_some() {
                    return Err(VstPresetError::DuplicateChunk);
                }
                controller = Some(slice);
            }
            CHUNK_INFO => {
                if info_xml.is_some() {
                    return Err(VstPresetError::DuplicateChunk);
                }
                if slice.len() > MAX_PRESET_INFO_BYTES {
                    return Err(VstPresetError::TooLarge);
                }
                info_xml =
                    Some(std::str::from_utf8(slice).map_err(|_| VstPresetError::InvalidInfoUtf8)?);
            }
            _ => {}
        }
    }

    let state_bytes = component
        .map_or(0, <[u8]>::len)
        .checked_add(controller.map_or(0, <[u8]>::len))
        .ok_or(VstPresetError::TooLarge)?;
    if state_bytes > MAX_OPAQUE_BYTES {
        return Err(VstPresetError::TooLarge);
    }

    Ok(VstPreset {
        class,
        component,
        controller,
        info_xml,
    })
}

/// Export an existing canonical VST3 `NativeDeviceState` to `.vstpreset` bytes.
pub fn export_state_to_vstpreset(
    state: &NativeDeviceState,
    info_xml: Option<&str>,
) -> Result<Vec<u8>, VstPresetError> {
    let decoded = decode(state)?;
    if decoded.component.is_none() {
        return Err(VstPresetError::MissingComponent);
    }
    encode_vstpreset(VstPreset {
        class: decoded.identity.class,
        component: decoded.component,
        controller: decoded.controller,
        info_xml,
    })
}

/// Apply a `.vstpreset` payload to a canonical VST3 `NativeDeviceState`,
/// restoring both component and controller chunks while retaining the device's
/// verified binary identity and host input/output gain metadata.
///
/// Refuses wrong-class, missing-component, malformed, or oversize presets
/// without mutating `current`.
pub fn apply_vstpreset_to_state(
    current: &NativeDeviceState,
    preset_bytes: &[u8],
) -> Result<NativeDeviceState, VstPresetError> {
    let existing = decode(current)?;
    let preset = decode_vstpreset(preset_bytes)?;
    if preset.class != existing.identity.class {
        return Err(VstPresetError::ClassMismatch);
    }
    if preset.component.is_none() {
        return Err(VstPresetError::MissingComponent);
    }
    let next = encode(State {
        identity: existing.identity,
        component: preset.component,
        controller: preset.controller,
    })?;
    let next = next
        .with_input_gain_millidb(current.input_gain_millidb())
        .map_err(|_| VstPresetError::Malformed)?;
    let next = next
        .with_output_gain_millidb(current.output_gain_millidb())
        .map_err(|_| VstPresetError::Malformed)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BinaryIdentity, Identity};

    fn sample_identity() -> Identity {
        Identity {
            class: ClassId::new([
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10,
            ])
            .unwrap(),
            binary: BinaryIdentity {
                executable_sha256: [0xaa; 32],
                info_sha256: [0xbb; 32],
            },
        }
    }

    fn sample_state(component: Option<&[u8]>, controller: Option<&[u8]>) -> NativeDeviceState {
        encode(State {
            identity: sample_identity(),
            component,
            controller,
        })
        .unwrap()
        .with_input_gain_millidb(-1200)
        .unwrap()
        .with_output_gain_millidb(600)
        .unwrap()
    }

    #[test]
    fn vstpreset_wire_layout_and_round_trip_preserve_component_controller_and_info() {
        let state = sample_state(Some(b"processor-chunk"), Some(b"controller-chunk"));
        let xml = "<MetaInfo><Attribute id=\"MediaType\" value=\"VstPreset\"/></MetaInfo>";
        let bytes = export_state_to_vstpreset(&state, Some(xml)).unwrap();

        assert_eq!(&bytes[0..4], b"VST3");
        assert_eq!(i32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
        assert_eq!(&bytes[8..40], b"0123456789ABCDEFFEDCBA9876543210");

        let decoded = decode_vstpreset(&bytes).unwrap();
        assert_eq!(decoded.class, sample_identity().class);
        assert_eq!(decoded.component, Some(&b"processor-chunk"[..]));
        assert_eq!(decoded.controller, Some(&b"controller-chunk"[..]));
        assert_eq!(decoded.info_xml, Some(xml));

        let original = sample_state(Some(b"old-comp"), Some(b"old-cont"));
        let applied = apply_vstpreset_to_state(&original, &bytes).unwrap();
        let applied_state = decode(&applied).unwrap();
        assert_eq!(applied_state.identity, sample_identity());
        assert_eq!(applied_state.component, Some(&b"processor-chunk"[..]));
        assert_eq!(applied_state.controller, Some(&b"controller-chunk"[..]));
        assert_eq!(applied.input_gain_millidb(), -1200);
        assert_eq!(applied.output_gain_millidb(), 600);
    }

    #[test]
    fn lowercase_class_id_hex_and_component_only_presets_decode_cleanly() {
        let mut bytes = encode_vstpreset(VstPreset {
            class: sample_identity().class,
            component: Some(b"only-comp"),
            controller: None,
            info_xml: None,
        })
        .unwrap();
        for byte in &mut bytes[8..40] {
            byte.make_ascii_lowercase();
        }
        let decoded = decode_vstpreset(&bytes).unwrap();
        assert_eq!(decoded.class, sample_identity().class);
        assert_eq!(decoded.component, Some(&b"only-comp"[..]));
        assert_eq!(decoded.controller, None);
        assert_eq!(decoded.info_xml, None);

        let existing = sample_state(Some(b"prev-comp"), Some(b"prev-cont"));
        let applied = apply_vstpreset_to_state(&existing, &bytes).unwrap();
        let applied_state = decode(&applied).unwrap();
        assert_eq!(applied_state.component, Some(&b"only-comp"[..]));
        assert_eq!(applied_state.controller, None);
    }

    #[test]
    fn malformed_wrong_class_missing_component_and_duplicate_chunks_refuse_cleanly() {
        let state = sample_state(Some(b"comp"), Some(b"cont"));
        let good = export_state_to_vstpreset(&state, None).unwrap();

        // Every truncation prefix fails cleanly.
        for len in 0..good.len() {
            assert!(decode_vstpreset(&good[..len]).is_err(), "len {len}");
        }

        // Bad magic & unsupported version.
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(decode_vstpreset(&bad_magic), Err(VstPresetError::Malformed));

        let mut bad_version = good.clone();
        bad_version[4..8].copy_from_slice(&2_i32.to_le_bytes());
        assert_eq!(
            decode_vstpreset(&bad_version),
            Err(VstPresetError::UnsupportedVersion(2))
        );

        // Class mismatch leaves current state untouched.
        let other_class = ClassId::new([0x99; 16]).unwrap();
        let foreign = encode_vstpreset(VstPreset {
            class: other_class,
            component: Some(b"other"),
            controller: None,
            info_xml: None,
        })
        .unwrap();
        assert_eq!(
            apply_vstpreset_to_state(&state, &foreign),
            Err(VstPresetError::ClassMismatch)
        );

        // Missing component chunk refuses apply and export.
        let no_comp = encode_vstpreset(VstPreset {
            class: sample_identity().class,
            component: None,
            controller: Some(b"only-cont"),
            info_xml: None,
        })
        .unwrap();
        assert_eq!(
            apply_vstpreset_to_state(&state, &no_comp),
            Err(VstPresetError::MissingComponent)
        );
        let no_comp_state = sample_state(None, Some(b"cont"));
        assert_eq!(
            export_state_to_vstpreset(&no_comp_state, None),
            Err(VstPresetError::MissingComponent)
        );

        // Duplicate Comp chunk refuses.
        let mut dup = Vec::new();
        dup.extend_from_slice(MAGIC_VST3);
        dup.extend_from_slice(&1_i32.to_le_bytes());
        dup.extend_from_slice(&encode_class_id_hex(sample_identity().class));
        dup.extend_from_slice(&(52_i64).to_le_bytes());
        dup.extend_from_slice(b"abcd");
        dup.extend_from_slice(MAGIC_LIST);
        dup.extend_from_slice(&2_i32.to_le_bytes());
        for _ in 0..2 {
            dup.extend_from_slice(CHUNK_COMP);
            dup.extend_from_slice(&48_i64.to_le_bytes());
            dup.extend_from_slice(&4_i64.to_le_bytes());
        }
        assert_eq!(decode_vstpreset(&dup), Err(VstPresetError::DuplicateChunk));

        // Invalid UTF-8 in Info chunk refuses.
        let mut bad_utf8 = Vec::new();
        bad_utf8.extend_from_slice(MAGIC_VST3);
        bad_utf8.extend_from_slice(&1_i32.to_le_bytes());
        bad_utf8.extend_from_slice(&encode_class_id_hex(sample_identity().class));
        bad_utf8.extend_from_slice(&(50_i64).to_le_bytes());
        bad_utf8.extend_from_slice(&[0xff, 0xfe]);
        bad_utf8.extend_from_slice(MAGIC_LIST);
        bad_utf8.extend_from_slice(&1_i32.to_le_bytes());
        bad_utf8.extend_from_slice(CHUNK_INFO);
        bad_utf8.extend_from_slice(&48_i64.to_le_bytes());
        bad_utf8.extend_from_slice(&2_i64.to_le_bytes());
        assert_eq!(
            decode_vstpreset(&bad_utf8),
            Err(VstPresetError::InvalidInfoUtf8)
        );
    }
}
