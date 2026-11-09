//! Effective SDK editor input carried beside one original successful PCM reply.
//! No editor receipt is consumed here and this is not host output acceptance.
use super::*;
use crate::transport::{MAX_PARAMETER_POINTS, wire::HEADER_BYTES};
use std::io::Read;

pub(crate) const EDITOR_PCM_BIT: u16 = 0x4000;
const EDITOR_POINT_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorPerform {
    pub sequence: u64,
    pub gesture: u64,
    pub id: u32,
    pub normalized: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EditorPerforms {
    points: [EditorPerform; MAX_PARAMETER_EDITS],
    len: usize,
}

impl EditorPerforms {
    pub fn as_slice(&self) -> &[EditorPerform] {
        &self.points[..self.len]
    }
}

/// One successful SDK reply, including the exact original binding and quantum.
/// Multiple staged editor callbacks to the same parameter at offset zero
/// report only the final one that actually entered the processor.
pub struct EditorPcm {
    pub block: Block,
    pub performs: EditorPerforms,
}

struct Layout {
    pcm_end: usize,
    len: usize,
    editor: bool,
}

fn layout(header: &[u8; HEADER_BYTES]) -> Result<Layout> {
    let pcm = &header[..4] == b"HXVB";
    let terminal = &header[..4] == b"HXVE";
    let frames = usize::from(u16::from_le_bytes([header[6], header[7]]));
    let wire_version = u16::from_le_bytes([header[4], header[5]]);
    let editor = wire_version & EDITOR_PCM_BIT != 0;
    let version = wire_version & !EDITOR_PCM_BIT;
    let parameters = usize::from(u16::from_le_bytes([header[10], header[11]]));
    let len = usize::from(header[12]);
    if !(pcm || terminal)
        || frames > MAX_FRAMES
        || header[8..10] != [0; 2]
        || !matches!(version, 1 | 2)
        || (version == 1 && parameters != 0)
        || parameters > MAX_PARAMETER_POINTS
        || header[13..16] != [0; 3]
        || (parameters != 0 && (frames == 0 || !pcm))
        || (editor && (!pcm || frames == 0))
        || len > MAX_PARAMETER_EDITS
        || (!editor && len != 0)
    {
        return Err(Error::Protocol("unsupported instance PCM extent or events"));
    }
    Ok(Layout {
        pcm_end: HEADER_BYTES + frames * 8 + parameters * 16,
        len,
        editor,
    })
}

pub(crate) fn pcm_packet_extent(header: &[u8]) -> Result<usize> {
    let header = header
        .try_into()
        .map_err(|_| Error::Protocol("instance PCM header extent differs"))?;
    let layout = layout(header)?;
    Ok(layout.pcm_end + layout.len * EDITOR_POINT_BYTES)
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn editor_pcm(
    bytes: &[u8],
    config: &Configuration,
    sequence: u64,
    previous: u64,
) -> Result<EditorPcm> {
    let (header, payload) = bytes
        .split_first_chunk::<HEADER_BYTES>()
        .ok_or(Error::Protocol("missing instance editor PCM header"))?;
    let layout = layout(header)?;
    if !layout.editor || bytes.len() != layout.pcm_end + layout.len * EDITOR_POINT_BYTES {
        return Err(Error::Protocol(
            "instance editor PCM extent or opt-in differs",
        ));
    }
    // Pass the unchanged ordinary PCM payload through the same strict decoder.
    let mut ordinary = *header;
    ordinary[5] &= !((EDITOR_PCM_BIT >> 8) as u8);
    ordinary[12] = 0;
    let ordinary_reader = ordinary
        .as_slice()
        .chain(&payload[..layout.pcm_end - HEADER_BYTES]);
    let block = read_pcm(ordinary_reader, config, sequence)?;
    let mut reader = Reader::new(&bytes[layout.pcm_end..]);
    let mut performs = EditorPerforms::default();
    let mut previous = previous;
    for _ in 0..layout.len {
        let sequence = u64::from_le_bytes(reader.array()?);
        let gesture = u64::from_le_bytes(reader.array()?);
        let normalized = f64::from_le_bytes(reader.array()?);
        let id = u32::from_le_bytes(reader.array()?);
        let reserved = reader.array::<4>()?;
        if sequence <= previous
            || gesture == 0
            || gesture >= sequence
            || id > i32::MAX as u32
            || !(0.0..=1.0).contains(&normalized)
            || reserved != [0; 4]
            || performs.as_slice().iter().any(|point| point.id == id)
        {
            return Err(Error::Protocol("invalid effective editor PCM input"));
        }
        performs.points[performs.len] = EditorPerform {
            sequence,
            gesture,
            id,
            normalized,
        };
        performs.len += 1;
        previous = sequence;
    }
    reader.end()?;
    Ok(EditorPcm { block, performs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(config: &Configuration, sequence: u64, points: &[EditorPerform]) -> Vec<u8> {
        let pcm = Block {
            binding: config.binding,
            sequence,
            first_frame: sequence * u64::from(config.quantum),
            frames: config.quantum,
            pcm: [[0.0; 2]; MAX_FRAMES],
            events: Default::default(),
            parameters: Default::default(),
            timing: None,
            timing_changes: Default::default(),
        };
        let mut bytes = Vec::new();
        crate::transport::wire::write(&mut bytes, &pcm).unwrap();
        bytes[5] = (EDITOR_PCM_BIT >> 8) as u8;
        bytes[12] = u8::try_from(points.len()).unwrap();
        for point in points {
            bytes.extend(point.sequence.to_le_bytes());
            bytes.extend(point.gesture.to_le_bytes());
            bytes.extend(point.normalized.to_le_bytes());
            bytes.extend(point.id.to_le_bytes());
            bytes.extend([0; 4]);
        }
        bytes
    }

    #[test]
    fn editor_reply_admits_exact_raw_zero_and_refuses_malformed_or_replayed_points() {
        let config = super::super::tests::config();
        let point = EditorPerform {
            sequence: 5,
            gesture: 1,
            id: 0,
            normalized: 0.875,
        };
        let next = EditorPerform {
            sequence: 6,
            gesture: 2,
            id: 7,
            normalized: 0.125,
        };
        let valid = packet(&config, 4, &[point, next]);
        assert_eq!(
            pcm_packet_extent(&valid[..HEADER_BYTES]).unwrap(),
            valid.len()
        );
        assert!(
            pcm(&valid, &config, 4).is_err(),
            "ordinary mode must refuse an editor tail"
        );
        let decoded = editor_pcm(&valid, &config, 4, 3).unwrap();
        assert_eq!(decoded.block.binding, config.binding);
        assert_eq!(decoded.block.first_frame, 32);
        assert_eq!(decoded.performs.as_slice(), [point, next]);
        assert!(editor_pcm(&valid, &config, 5, 3).is_err());
        assert!(editor_pcm(&valid, &config, 4, 5).is_err());
        assert!(editor_pcm(&valid[..valid.len() - 1], &config, 4, 3).is_err());
        let mut wrong_source = config.clone();
        wrong_source.binding.epoch += 1;
        assert!(editor_pcm(&valid, &wrong_source, 4, 3).is_err());
        for bad in [
            [point, EditorPerform { id: 0, ..next }],
            [next, point],
            [
                point,
                EditorPerform {
                    normalized: f64::NAN,
                    ..next
                },
            ],
            [
                point,
                EditorPerform {
                    normalized: 1.01,
                    ..next
                },
            ],
            [
                point,
                EditorPerform {
                    gesture: next.sequence,
                    ..next
                },
            ],
        ] {
            assert!(editor_pcm(&packet(&config, 4, &bad), &config, 4, 3).is_err());
        }
        let mut too_many = valid.clone();
        too_many[12] = (MAX_PARAMETER_EDITS + 1) as u8;
        assert!(pcm_packet_extent(&too_many[..HEADER_BYTES]).is_err());
        let mut unknown = valid.clone();
        unknown[5] = 0;
        assert!(pcm_packet_extent(&unknown[..HEADER_BYTES]).is_err());
        let mut reserved = valid;
        *reserved.last_mut().unwrap() = 1;
        assert!(editor_pcm(&reserved, &config, 4, 3).is_err());
    }
}
