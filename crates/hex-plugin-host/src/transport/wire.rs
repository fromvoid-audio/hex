//! Worker-only bounded little-endian copied protocol. Never call I/O on callback.
//! Header: HXVB, version u16, frames u16, event count u16, parameter count
//! u16 (versions 2 and 3), four zero bytes,
//! root/instance/graph/epoch/sequence/first-frame u64. Then interleaved f32 PCM
//! and eight-byte MIDI records (offset u16, len u8, three bytes, two zero bytes).
//! Version2 appends sixteen-byte parameter records: offset u16, two zero bytes,
//! raw ParamID u32 and normalized f64. Empty parameter payloads retain version1.
//! Version3 extends each MIDI record with a u32 note ID (zero means absent).
//! Version4 uses those extended records and appends a 24-byte input clock:
//! Arrangement frame i64, beat Q32.32 u64, tempo micro-BPM u32 (zero absent),
//! flags u8 (playing, beat present), then three reserved zero bytes.
//! Version5 adds meter/cycle presence flags, two meter bytes, one reserved
//! zero byte, and two u64 Q32.32 cycle endpoints. Absent fields are zero.
//! Version6 uses the 40-byte context and appends one to seven context
//! changes. Header byte 12 is their count. Byte 13 is one when the initial
//! context is absent. Each change carries a u16 sample
//! offset, six reserved zero bytes, and its own 40-byte context.
//! Versions 3–6 carry up to 160 input events only when every event has a note
//! ID. The generic event and worker reply contracts remain capped at 64.
//! The request-only high bit of the version marks authored PCM. Each parameter
//! record then appends its prepared SDK step count as a u32. The one admitted
//! manual point, if present, leads the block at offset zero with its own tag. Replies and
//! stopped flushes keep the original records.
//! Request bit 0x0800 carries one additional authored lane after the timing
//! data, with raw ID and SDK step count as u32 followed by one f64 per frame.
//! Replies retain their sparse bound. Wire records alone grant no SDK access.
use super::*;
use std::io::{self, Read, Write};

pub const HEADER_BYTES: usize = 64;
const AUTHORED_PARAMETER_VERSION_BIT: u16 = 0x8000;
const DENSE_PARAMETER_VERSION_BIT: u16 = 0x0800;
const DENSE_PARAMETER_HEADER_BYTES: usize = 8;
const TIMING_BYTES: usize = 24;
const EXTENDED_TIMING_BYTES: usize = 40;
const TIMING_CHANGE_BYTES: usize = 8 + EXTENDED_TIMING_BYTES;
pub const MAX_PACKET_BYTES: usize = HEADER_BYTES
    + MAX_FRAMES * 8
    + MAX_IDENTIFIED_INPUT_NOTES * 12
    + MAX_PARAMETER_POINTS * 20
    + EXTENDED_TIMING_BYTES
    + MAX_TIMING_CHANGES * TIMING_CHANGE_BYTES;
pub const MAX_DENSE_PACKET_BYTES: usize =
    MAX_PACKET_BYTES + DENSE_PARAMETER_HEADER_BYTES + MAX_FRAMES * 8;

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

pub fn write(writer: &mut impl Write, block: &Block) -> io::Result<()> {
    write_with_parameters(writer, block, None, None, None)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn write_authored(
    writer: &mut impl Write,
    block: &Block,
    manual_parameter: Option<ParameterPoint>,
    step_count: &dyn Fn(u32) -> Option<u32>,
) -> io::Result<()> {
    write_with_parameters(writer, block, Some(step_count), manual_parameter, None)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn write_dense_authored(
    writer: &mut impl Write,
    block: &Block,
    lane: DenseParameterLane<'_>,
    manual_parameter: Option<ParameterPoint>,
    step_count: &dyn Fn(u32) -> Option<u32>,
) -> io::Result<()> {
    write_with_parameters(
        writer,
        block,
        Some(step_count),
        manual_parameter,
        Some(lane),
    )
}

fn write_with_parameters(
    writer: &mut impl Write,
    block: &Block,
    step_count: Option<&dyn Fn(u32) -> Option<u32>>,
    manual_parameter: Option<ParameterPoint>,
    dense: Option<DenseParameterLane<'_>>,
) -> io::Result<()> {
    if block.frames == 0 || usize::from(block.frames) > MAX_FRAMES {
        return Err(invalid());
    }
    block
        .valid(block.binding, block.frames, block.sequence)
        .map_err(|_| invalid())?;
    if [
        block.binding.root,
        block.binding.instance,
        block.binding.graph,
        block.binding.epoch,
    ]
    .contains(&0)
    {
        return Err(invalid());
    }
    let mut packet = [0_u8; MAX_PACKET_BYTES];
    packet[..4].copy_from_slice(b"HXVB");
    let dense_steps = dense
        .map(|lane| {
            if !lane.valid(usize::from(block.frames))
                || block
                    .parameters
                    .as_slice()
                    .iter()
                    .any(|point| point.id == lane.id)
            {
                return Err(invalid());
            }
            step_count
                .and_then(|lookup| lookup(lane.id))
                .filter(|&steps| steps <= i32::MAX as u32)
                .ok_or_else(invalid)
        })
        .transpose()?;
    let changes = block.timing_changes.as_slice();
    let version = if !changes.is_empty() {
        6_u16
    } else if block
        .timing
        .is_some_and(|timing| timing.meter.is_some() || timing.cycle_beats_raw.is_some())
    {
        5_u16
    } else if block.timing.is_some() {
        4_u16
    } else if block
        .events
        .as_slice()
        .iter()
        .any(|event| event.note_id.is_some())
    {
        3_u16
    } else if block.parameters.as_slice().is_empty() && dense.is_none() {
        1_u16
    } else {
        2_u16
    };
    let points = block.parameters.as_slice();
    if let Some(manual) = manual_parameter
        && (step_count.is_none()
            || manual.frame != 0
            || points.first() != Some(&manual)
            || points[1..].iter().any(|point| point.id == manual.id))
    {
        return Err(invalid());
    }
    let authored = step_count.is_some() && (!points.is_empty() || dense.is_some());
    let wire_version = version
        | if authored {
            AUTHORED_PARAMETER_VERSION_BIT
        } else {
            0
        }
        | if dense.is_some() {
            DENSE_PARAMETER_VERSION_BIT
        } else {
            0
        };
    packet[4..6].copy_from_slice(&wire_version.to_le_bytes());
    packet[6..8].copy_from_slice(&block.frames.to_le_bytes());
    packet[8..10].copy_from_slice(&(block.events.len as u16).to_le_bytes());
    packet[10..12].copy_from_slice(&(block.parameters.as_slice().len() as u16).to_le_bytes());
    packet[12] = changes.len() as u8;
    if version == 6 {
        packet[13] = u8::from(block.timing.is_none());
    }
    for (index, value) in [
        block.binding.root,
        block.binding.instance,
        block.binding.graph,
        block.binding.epoch,
        block.sequence,
        block.first_frame,
    ]
    .iter()
    .enumerate()
    {
        packet[16 + index * 8..24 + index * 8].copy_from_slice(&value.to_le_bytes());
    }
    let mut offset = HEADER_BYTES;
    for frame in &block.pcm[..usize::from(block.frames)] {
        for sample in frame {
            packet[offset..offset + 4].copy_from_slice(&sample.to_le_bytes());
            offset += 4;
        }
    }
    for event in block.events.as_slice() {
        packet[offset..offset + 2].copy_from_slice(&event.frame.to_le_bytes());
        packet[offset + 2] = event.len;
        packet[offset + 3..offset + 6].copy_from_slice(&event.bytes);
        offset += 8;
        if version >= 3 {
            let id = event.note_id.map_or(0, std::num::NonZeroU32::get);
            packet[offset..offset + 4].copy_from_slice(&id.to_le_bytes());
            offset += 4;
        }
    }
    for (index, point) in points.iter().enumerate() {
        let manual = index == 0 && manual_parameter.is_some();
        packet[offset + 2] = u8::from(manual);
        packet[offset..offset + 2].copy_from_slice(&point.frame.to_le_bytes());
        packet[offset + 4..offset + 8].copy_from_slice(&point.id.to_le_bytes());
        packet[offset + 8..offset + 16].copy_from_slice(&point.normalized.to_le_bytes());
        offset += 16;
        if let Some(step_count) = step_count {
            let steps = if manual {
                0
            } else {
                step_count(point.id)
                    .filter(|&steps| steps <= i32::MAX as u32)
                    .ok_or_else(invalid)?
            };
            packet[offset..offset + 4].copy_from_slice(&steps.to_le_bytes());
            offset += 4;
        }
    }
    if let Some(timing) = block.timing {
        let width = timing_width(version);
        encode_timing(&mut packet[offset..offset + width], timing);
        offset += width;
    }
    for change in changes {
        packet[offset..offset + 2].copy_from_slice(&change.frame.to_le_bytes());
        encode_timing(
            &mut packet[offset + 8..offset + TIMING_CHANGE_BYTES],
            change.timing,
        );
        offset += TIMING_CHANGE_BYTES;
    }
    writer.write_all(&packet[..offset])?;
    if let Some((lane, steps)) = dense.zip(dense_steps) {
        let mut payload = [0_u8; DENSE_PARAMETER_HEADER_BYTES + MAX_FRAMES * 8];
        payload[..4].copy_from_slice(&lane.id.to_le_bytes());
        payload[4..DENSE_PARAMETER_HEADER_BYTES].copy_from_slice(&steps.to_le_bytes());
        let mut offset = DENSE_PARAMETER_HEADER_BYTES;
        for value in lane.normalized {
            payload[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            offset += 8;
        }
        writer.write_all(&payload[..offset])?;
    }
    writer.flush()
}

fn timing_width(version: u16) -> usize {
    if version >= 5 {
        EXTENDED_TIMING_BYTES
    } else {
        TIMING_BYTES
    }
}

fn encode_timing(bytes: &mut [u8], timing: ProcessTiming) {
    bytes[..8].copy_from_slice(&timing.arrangement_frame.to_le_bytes());
    bytes[8..16].copy_from_slice(&timing.arrangement_beat_raw.unwrap_or(0).to_le_bytes());
    bytes[16..20].copy_from_slice(
        &timing
            .tempo_micro_bpm
            .map_or(0, std::num::NonZeroU32::get)
            .to_le_bytes(),
    );
    bytes[20] = u8::from(timing.playing) | (u8::from(timing.arrangement_beat_raw.is_some()) << 1);
    if let Some((numerator, denominator)) = timing.meter {
        bytes[20] |= 4;
        bytes[21] = numerator;
        bytes[22] = denominator;
    }
    if let Some((start, end)) = timing.cycle_beats_raw {
        bytes[20] |= 8;
        bytes[24..32].copy_from_slice(&start.to_le_bytes());
        bytes[32..40].copy_from_slice(&end.to_le_bytes());
    }
}

fn read_timing(reader: &mut impl Read, version: u16) -> io::Result<ProcessTiming> {
    let mut bytes = [0_u8; EXTENDED_TIMING_BYTES];
    reader.read_exact(&mut bytes[..timing_width(version)])?;
    let frame = i64::from_le_bytes(bytes[..8].try_into().map_err(|_| invalid())?);
    let beat = u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| invalid())?);
    let tempo = u32::from_le_bytes(bytes[16..20].try_into().map_err(|_| invalid())?);
    let flags = bytes[20];
    let start = u64::from_le_bytes(bytes[24..32].try_into().map_err(|_| invalid())?);
    let end = u64::from_le_bytes(bytes[32..40].try_into().map_err(|_| invalid())?);
    if flags & !15 != 0
        || (version < 5 && flags & 12 != 0)
        || (version == 5 && flags & 12 == 0)
        || bytes[23] != 0
        || (flags & 2 == 0 && beat != 0)
        || (flags & 4 == 0 && bytes[21..23] != [0; 2])
        || (flags & 8 == 0 && (start != 0 || end != 0))
    {
        return Err(invalid());
    }
    Ok(ProcessTiming {
        arrangement_frame: frame,
        playing: flags & 1 != 0,
        arrangement_beat_raw: (flags & 2 != 0).then_some(beat),
        tempo_micro_bpm: std::num::NonZeroU32::new(tempo),
        meter: (flags & 4 != 0).then_some((bytes[21], bytes[22])),
        cycle_beats_raw: (flags & 8 != 0).then_some((start, end)),
    })
}

/// Read exactly one packet, without allocating or trusting a peer's extent.
/// EOF at a packet boundary is a clean end; truncated header/body is an error.
pub fn read(reader: &mut impl Read) -> io::Result<Option<Block>> {
    let mut header = [0_u8; HEADER_BYTES];
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    reader.read_exact(&mut header[1..])?;
    let version = u16::from_le_bytes([header[4], header[5]]);
    let change_count = if version == 6 {
        usize::from(header[12])
    } else {
        0
    };
    let reserved = match version {
        1 => 10,
        6 => 14,
        _ => 12,
    };
    if &header[..4] != b"HXVB"
        || !matches!(version, 1..=6)
        || (version == 6 && (!(1..=MAX_TIMING_CHANGES).contains(&change_count) || header[13] > 1))
        || header[reserved..16].iter().any(|b| *b != 0)
    {
        return Err(invalid());
    }
    let frames = u16::from_le_bytes([header[6], header[7]]);
    let count = usize::from(u16::from_le_bytes([header[8], header[9]]));
    let parameters = usize::from(u16::from_le_bytes([header[10], header[11]]));
    if frames == 0
        || usize::from(frames) > MAX_FRAMES
        || count
            > if version >= 3 {
                MAX_IDENTIFIED_INPUT_NOTES
            } else {
                MAX_EVENTS
            }
        || (version == 3 && count == 0)
        || parameters > MAX_PARAMETER_POINTS
    {
        return Err(invalid());
    }
    let mut values = [0_u64; 6];
    for (index, value) in values.iter_mut().enumerate() {
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&header[16 + index * 8..24 + index * 8]);
        *value = u64::from_le_bytes(bytes);
    }
    if values[..4].contains(&0) {
        return Err(invalid());
    }
    let binding = Binding {
        root: values[0],
        instance: values[1],
        graph: values[2],
        epoch: values[3],
    };
    let mut block = Block::empty(binding, frames);
    block.sequence = values[4];
    block.first_frame = values[5];
    for frame in &mut block.pcm[..usize::from(frames)] {
        for sample in frame {
            let mut bytes = [0_u8; 4];
            reader.read_exact(&mut bytes)?;
            *sample = f32::from_le_bytes(bytes);
        }
    }
    for _ in 0..count {
        let mut bytes = [0_u8; 8];
        reader.read_exact(&mut bytes)?;
        if bytes[6..].iter().any(|b| *b != 0) {
            return Err(invalid());
        }
        let note_id = if version >= 3 {
            let mut id = [0_u8; 4];
            reader.read_exact(&mut id)?;
            std::num::NonZeroU32::new(u32::from_le_bytes(id))
        } else {
            None
        };
        block
            .events
            .push(MidiEvent {
                frame: u16::from_le_bytes([bytes[0], bytes[1]]),
                len: bytes[2],
                bytes: [bytes[3], bytes[4], bytes[5]],
                note_id,
            })
            .map_err(|_| invalid())?;
    }
    for _ in 0..parameters {
        let mut bytes = [0_u8; 16];
        reader.read_exact(&mut bytes)?;
        if bytes[2..4] != [0; 2] {
            return Err(invalid());
        }
        let mut value = [0; 8];
        value.copy_from_slice(&bytes[8..16]);
        block
            .parameters
            .push(ParameterPoint {
                frame: u16::from_le_bytes([bytes[0], bytes[1]]),
                id: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                normalized: f64::from_le_bytes(value),
            })
            .map_err(|_| invalid())?;
    }
    if version >= 4 && header[13] == 0 {
        block.timing = Some(read_timing(reader, version)?);
    }
    for _ in 0..change_count {
        let mut prefix = [0; 8];
        reader.read_exact(&mut prefix)?;
        if prefix[2..] != [0; 6] {
            return Err(invalid());
        }
        block
            .add_timing_change(
                u16::from_le_bytes([prefix[0], prefix[1]]),
                read_timing(reader, version)?,
            )
            .map_err(|_| invalid())?;
    }
    block
        .valid(binding, frames, block.sequence)
        .map_err(|_| invalid())?;
    Ok(Some(block))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timed_wire_preserves_notes_parameters_and_optional_clock_facts() {
        let mut block = Block::empty(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            8,
        );
        let timing = ProcessTiming {
            arrangement_frame: 48_000,
            playing: true,
            arrangement_beat_raw: Some(5 << 31),
            tempo_micro_bpm: std::num::NonZeroU32::new(123_125_000),
            meter: None,
            cycle_beats_raw: None,
        };
        block.timing = Some(timing);
        let event = MidiEvent {
            frame: 3,
            bytes: [0x90, 60, 100],
            len: 3,
            note_id: std::num::NonZeroU32::new(123),
        };
        let point = ParameterPoint {
            frame: 2,
            id: 7,
            normalized: 0.25,
        };
        block.events.push(event).unwrap();
        block.parameters.push(point).unwrap();
        let mut bytes = Vec::new();
        write(&mut bytes, &block).unwrap();
        assert_eq!(&bytes[4..6], &[4, 0]);
        let decoded = read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(decoded.timing, Some(timing));
        assert_eq!(decoded.events.as_slice(), &[event]);
        assert_eq!(decoded.parameters.as_slice(), &[point]);
        let extended = ProcessTiming {
            meter: Some((7, 8)),
            cycle_beats_raw: Some((5 << 30, 11 << 31)),
            ..timing
        };
        block.timing = Some(extended);
        let mut with_meter = Vec::new();
        write(&mut with_meter, &block).unwrap();
        assert_eq!(&with_meter[4..6], &[5, 0]);
        assert_eq!(
            read(&mut with_meter.as_slice()).unwrap().unwrap().timing,
            Some(extended)
        );
        let meter_offset = with_meter.len() - EXTENDED_TIMING_BYTES + 22;
        with_meter[meter_offset] = 3;
        assert!(read(&mut with_meter.as_slice()).is_err());
        let flags = bytes.len() - 4;
        bytes[flags] = 1; // Beat payload without its presence flag.
        assert!(read(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn parameter_wire_preserves_precision_offsets_and_rejects_truncation_or_bad_points() {
        let mut block = Block::empty(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            8,
        );
        for point in [
            ParameterPoint {
                frame: 0,
                id: 0,
                normalized: 0.1234567890123456,
            },
            ParameterPoint {
                frame: 7,
                id: 0x7fff_ffff,
                normalized: 1.0,
            },
        ] {
            block.parameters.push(point).unwrap();
        }
        let mut bytes = Vec::new();
        write(&mut bytes, &block).unwrap();
        assert_eq!(bytes.len(), HEADER_BYTES + 8 * 8 + 2 * 16);
        assert_eq!(&bytes[4..6], &2_u16.to_le_bytes());
        let decoded = read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(decoded.parameters, block.parameters);
        for end in 1..bytes.len() {
            assert!(read(&mut &bytes[..end]).is_err(), "truncation {end}");
        }
        let start = HEADER_BYTES + 8 * 8;
        for (index, value) in [
            (10, (MAX_PARAMETER_POINTS + 1) as u8),
            (12, 1),
            (start, 8),
            (start + 2, 1),
            (start + 7, 128),
        ] {
            let mut bad = bytes.clone();
            bad[index] = value;
            assert!(read(&mut bad.as_slice()).is_err(), "invalid byte {index}");
        }
        let mut bad = bytes.clone();
        bad[start + 8..start + 16].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(read(&mut bad.as_slice()).is_err());
    }

    #[test]
    fn identified_notes_round_trip_with_byte_only_notes_and_parameters() {
        let mut block = Block::empty(
            Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            8,
        );
        for (frame, note_id) in [(1, std::num::NonZeroU32::new(70_001)), (4, None)] {
            block
                .events
                .push(MidiEvent {
                    frame,
                    bytes: [0x90, 60, 100],
                    len: 3,
                    note_id,
                })
                .unwrap();
        }
        block
            .parameters
            .push(ParameterPoint {
                frame: 2,
                id: 7,
                normalized: 0.25,
            })
            .unwrap();
        let mut packet = Vec::new();
        write(&mut packet, &block).unwrap();
        assert_eq!(&packet[4..6], &3_u16.to_le_bytes());
        let decoded = read(&mut packet.as_slice()).unwrap().unwrap();
        assert_eq!(decoded.events.as_slice(), block.events.as_slice());
        assert_eq!(decoded.parameters, block.parameters);

        let event_start = HEADER_BYTES + 8 * 8;
        for end in event_start..packet.len() {
            assert!(read(&mut &packet[..end]).is_err(), "truncation {end}");
        }
        let mut malformed = packet.clone();
        malformed[event_start + 8..event_start + 12]
            .copy_from_slice(&0x8000_0000_u32.to_le_bytes());
        assert!(read(&mut malformed.as_slice()).is_err());
        packet[event_start + 3] = 0xb0;
        assert!(read(&mut packet.as_slice()).is_err());
    }

    #[test]
    fn canonical_wire_roundtrip_and_all_truncations_are_bounded() {
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let mut block = Block::empty(binding, 4);
        block.sequence = 5;
        block.first_frame = 20;
        block.pcm[..4].fill([0.25, -0.5]);
        block
            .events
            .push(MidiEvent {
                frame: 3,
                bytes: [0x90, 64, 100],
                len: 3,
                note_id: None,
            })
            .unwrap();
        let mut bytes = Vec::new();
        write(&mut bytes, &block).unwrap();
        assert_eq!(bytes.len(), 64 + 32 + 8);
        assert!(read(&mut &[][..]).unwrap().is_none());
        for end in 1..bytes.len() {
            assert!(read(&mut &bytes[..end]).is_err(), "truncation {end}");
        }
        let decoded = read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(decoded.binding, binding);
        assert_eq!(decoded.sequence, 5);
        assert_eq!(decoded.first_frame, 20);
        assert_eq!(decoded.frames, 4);
        assert_eq!(decoded.pcm[..4], block.pcm[..4]);
        assert_eq!(decoded.events.as_slice(), block.events.as_slice());
        for index in [0, 4, 10, 15, 16, 56, 102, 103] {
            let mut malformed = bytes.clone();
            malformed[index] ^= 0xff;
            // Identity fields may be structurally valid but must still be
            // checked against the pending exchange by the receiving endpoint.
            if matches!(index, 16) {
                assert_ne!(
                    read(&mut malformed.as_slice()).unwrap().unwrap().binding,
                    binding
                );
            } else {
                assert!(read(&mut malformed.as_slice()).is_err(), "byte {index}");
            }
        }
    }
}
