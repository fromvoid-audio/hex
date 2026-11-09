//! Effective SDK editor input carried beside one original successful PCM reply.
//! No editor receipt is consumed here and this is not host output acceptance.
use super::*;
use crate::transport::{MAX_PARAMETER_POINTS, wire::HEADER_BYTES};
use std::io::Read;

pub(crate) const EDITOR_PCM_BIT: u16 = 0x4000;
pub(crate) const EDITOR_TOUCH_BIT: u16 = 0x2000;
pub(crate) const EDITOR_GROUP_TOUCH_BIT: u16 = 0x1000;
const EDITOR_RESTORE_BYTES: usize = 24;
const EDITOR_POINT_BYTES: usize = 32;

struct Layout {
    pcm_end: usize,
    len: usize,
    editor: bool,
    touch: u16,
    restore: usize,
}
impl Layout {
    fn extent(&self) -> usize {
        self.pcm_end + self.len * EDITOR_POINT_BYTES + self.restore * EDITOR_RESTORE_BYTES
    }
}

fn layout(header: &[u8; HEADER_BYTES]) -> Result<Layout> {
    let pcm = &header[..4] == b"HXVB";
    let terminal = &header[..4] == b"HXVE";
    let frames = usize::from(u16::from_le_bytes([header[6], header[7]]));
    let wire_version = u16::from_le_bytes([header[4], header[5]]);
    let editor = wire_version & EDITOR_PCM_BIT != 0;
    let touch = wire_version & (EDITOR_TOUCH_BIT | EDITOR_GROUP_TOUCH_BIT);
    let restore = usize::from(header[13]);
    let limit = match touch {
        0 => 0,
        EDITOR_TOUCH_BIT => 1,
        EDITOR_GROUP_TOUCH_BIT => MAX_PARAMETER_EDITS,
        _ => return Err(Error::Protocol("conflicting editor touch modes")),
    };
    let version = wire_version & !(EDITOR_PCM_BIT | EDITOR_TOUCH_BIT | EDITOR_GROUP_TOUCH_BIT);
    let parameters = usize::from(u16::from_le_bytes([header[10], header[11]]));
    let len = usize::from(header[12]);
    if !(pcm || terminal)
        || frames > MAX_FRAMES
        || header[8..10] != [0; 2]
        || !matches!(version, 1 | 2)
        || (version == 1 && parameters != 0)
        || parameters > MAX_PARAMETER_POINTS
        || restore > limit
        || header[14..16] != [0; 2]
        || (touch != 0 && !editor)
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
        touch,
        restore,
    })
}

pub(crate) fn pcm_packet_extent(header: &[u8]) -> Result<usize> {
    let header = header
        .try_into()
        .map_err(|_| Error::Protocol("instance PCM header extent differs"))?;
    let layout = layout(header)?;
    Ok(layout.extent())
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn editor_pcm(
    bytes: &[u8],
    config: &Configuration,
    sequence: u64,
    previous: u64,
) -> Result<EditorPcm> {
    decode(bytes, config, sequence, previous, 0, &[], 0)
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn editor_touch_pcm(
    bytes: &[u8],
    config: &Configuration,
    sequence: u64,
    previous: u64,
    plan: EditorTouchPlan,
    previous_restoration: u64,
) -> Result<EditorPcm> {
    decode(
        bytes,
        config,
        sequence,
        previous,
        EDITOR_TOUCH_BIT,
        &[EditorTouchTarget {
            parameter: plan.parameter,
            base: plan.base,
            reading: plan.reading,
        }],
        previous_restoration,
    )
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn editor_group_touch_pcm(
    bytes: &[u8],
    config: &Configuration,
    sequence: u64,
    previous: u64,
    plan: &EditorTouchPlanSet,
    previous_restoration: u64,
) -> Result<EditorPcm> {
    decode(
        bytes,
        config,
        sequence,
        previous,
        EDITOR_GROUP_TOUCH_BIT,
        plan.targets(),
        previous_restoration,
    )
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn decode(
    bytes: &[u8],
    config: &Configuration,
    sequence: u64,
    previous: u64,
    touch: u16,
    targets: &[EditorTouchTarget],
    mut previous_restoration: u64,
) -> Result<EditorPcm> {
    let (header, payload) = bytes
        .split_first_chunk::<HEADER_BYTES>()
        .ok_or(Error::Protocol("missing instance editor PCM header"))?;
    let layout = layout(header)?;
    if !layout.editor || layout.touch != touch || bytes.len() != layout.extent() {
        return Err(Error::Protocol(
            "instance editor PCM extent or opt-in differs",
        ));
    }
    // Pass the unchanged ordinary PCM payload through the same strict decoder.
    let mut ordinary = *header;
    ordinary[5] &= !(((EDITOR_PCM_BIT | EDITOR_TOUCH_BIT | EDITOR_GROUP_TOUCH_BIT) >> 8) as u8);
    ordinary[12] = 0;
    ordinary[13] = 0;
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
        performs.push(EditorPerform {
            sequence,
            gesture,
            id,
            normalized,
        })?;
        previous = sequence;
    }
    let mut restorations = EditorRestorations::default();
    for _ in 0..layout.restore {
        let sequence = u64::from_le_bytes(reader.array()?);
        let gesture = u64::from_le_bytes(reader.array()?);
        let parameter = u32::from_le_bytes(reader.array()?);
        if sequence <= previous_restoration
            || gesture == 0
            || gesture >= sequence
            || !targets.iter().any(|target| target.parameter == parameter)
            || restorations
                .as_slice()
                .iter()
                .any(|row| row.parameter == parameter)
            || reader.array::<4>()? != [0; 4]
            || performs
                .as_slice()
                .iter()
                .any(|point| point.id == parameter)
        {
            return Err(Error::Protocol("invalid effective editor restoration"));
        }
        restorations.push(EditorRestoration {
            sequence,
            gesture,
            parameter,
        })?;
        previous_restoration = sequence;
    }
    reader.end()?;
    Ok(EditorPcm {
        block,
        performs,
        restorations,
    })
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

    #[test]
    fn restoration_reply_requires_exact_touch_mode_target_and_new_end_sequence() {
        let config = super::super::tests::config();
        let plan = EditorTouchPlan {
            scope: std::num::NonZeroU64::MIN,
            parameter: 0,
            base: 0.25,
            reading: false,
        };
        let mut bytes = packet(&config, 4, &[]);
        bytes[5] |= (EDITOR_TOUCH_BIT >> 8) as u8;
        bytes[13] = 1;
        bytes.extend(10_u64.to_le_bytes());
        bytes.extend(8_u64.to_le_bytes());
        bytes.extend(0_u32.to_le_bytes());
        bytes.extend([0; 4]);
        let reply = editor_touch_pcm(&bytes, &config, 4, 0, plan, 9).unwrap();
        assert_eq!(
            reply.restorations.as_slice(),
            [EditorRestoration {
                sequence: 10,
                gesture: 8,
                parameter: 0
            }]
        );
        assert!(reply.performs.as_slice().is_empty());
        assert!(editor_pcm(&bytes, &config, 4, 0).is_err());
        assert!(editor_touch_pcm(&bytes, &config, 4, 0, plan, 10).is_err());
        let other = EditorTouchPlan {
            parameter: 1,
            ..plan
        };
        assert!(editor_touch_pcm(&bytes, &config, 4, 0, other, 0).is_err());
        assert!(editor_touch_pcm(&bytes[..bytes.len() - 1], &config, 4, 0, plan, 0).is_err());
    }
    #[test]
    fn grouped_restoration_reply_requires_exact_unique_targets_and_order() {
        let config = super::super::tests::config();
        let scope = std::num::NonZeroU64::MIN;
        let targets = [
            EditorTouchTarget {
                parameter: 0,
                base: 0.25,
                reading: true,
            },
            EditorTouchTarget {
                parameter: 11,
                base: 0.75,
                reading: false,
            },
        ];
        let plan = EditorTouchPlanSet::new(scope, &targets).unwrap();
        assert!(EditorTouchPlanSet::new(scope, &[targets[1], targets[0]]).is_err());
        assert!(EditorTouchPlanSet::new(scope, &[targets[0]; 2]).is_err());
        assert!(EditorTouchPlanSet::new(scope, &[targets[0]]).is_err());
        let rows = [
            EditorRestoration {
                sequence: 14,
                gesture: 3,
                parameter: 11,
            },
            EditorRestoration {
                sequence: 15,
                gesture: 2,
                parameter: 0,
            },
        ];
        let encode = |rows: &[EditorRestoration]| {
            let mut bytes = packet(&config, 4, &[]);
            bytes[5] |= (EDITOR_GROUP_TOUCH_BIT >> 8) as u8;
            bytes[13] = rows.len() as u8;
            for row in rows {
                bytes.extend(row.sequence.to_le_bytes());
                bytes.extend(row.gesture.to_le_bytes());
                bytes.extend(row.parameter.to_le_bytes());
                bytes.extend([0; 4]);
            }
            bytes
        };
        let valid = encode(&rows);
        assert_eq!(
            pcm_packet_extent(&valid[..HEADER_BYTES]).unwrap(),
            valid.len()
        );
        let decode = |bytes: &[u8]| editor_group_touch_pcm(bytes, &config, 4, 0, &plan, 13);
        assert_eq!(decode(&valid).unwrap().restorations.as_slice(), rows);
        assert!(editor_pcm(&valid, &config, 4, 0).is_err());
        assert!(decode(&valid[..valid.len() - 1]).is_err());
        assert!(decode(&encode(&[rows[1], rows[0]])).is_err());
        assert!(
            decode(&encode(&[
                rows[0],
                EditorRestoration {
                    parameter: 11,
                    ..rows[1]
                }
            ]))
            .is_err()
        );
        assert!(
            decode(&encode(&[
                rows[0],
                EditorRestoration {
                    parameter: 12,
                    ..rows[1]
                }
            ]))
            .is_err()
        );
        let mut mixed = valid.clone();
        mixed[5] |= (EDITOR_TOUCH_BIT >> 8) as u8;
        assert!(decode(&mixed).is_err());
        let mut unbounded = valid;
        unbounded[13] = (MAX_PARAMETER_EDITS + 1) as u8;
        assert!(pcm_packet_extent(&unbounded[..HEADER_BYTES]).is_err());
    }
}
