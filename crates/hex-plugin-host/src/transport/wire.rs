//! Worker-only bounded little-endian copied protocol. Never call I/O on callback.
//! Header: HXVB, version u16, frames u16, event count u16, parameter count
//! u16 (version2 only), four zero bytes,
//! root/instance/graph/epoch/sequence/first-frame u64. Then interleaved f32 PCM
//! and eight-byte MIDI records (offset u16, len u8, three bytes, two zero bytes).
//! Version2 appends sixteen-byte parameter records: offset u16, two zero bytes,
//! raw ParamID u32 and normalized f64. Empty parameter payloads retain version1.
//! These records carry transport data; they do not promise SDK event admission.
use super::*;
use std::io::{self, Read, Write};

pub const HEADER_BYTES: usize = 64;
pub const MAX_PACKET_BYTES: usize =
    HEADER_BYTES + MAX_FRAMES * 8 + MAX_EVENTS * 8 + MAX_PARAMETER_POINTS * 16;

fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

pub fn write(writer: &mut impl Write, block: &Block) -> io::Result<()> {
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
    let version = if block.parameters.as_slice().is_empty() {
        1_u16
    } else {
        2_u16
    };
    packet[4..6].copy_from_slice(&version.to_le_bytes());
    packet[6..8].copy_from_slice(&block.frames.to_le_bytes());
    packet[8..10].copy_from_slice(&(block.events.len as u16).to_le_bytes());
    packet[10..12].copy_from_slice(&(block.parameters.as_slice().len() as u16).to_le_bytes());
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
    }
    for point in block.parameters.as_slice() {
        packet[offset..offset + 2].copy_from_slice(&point.frame.to_le_bytes());
        packet[offset + 4..offset + 8].copy_from_slice(&point.id.to_le_bytes());
        packet[offset + 8..offset + 16].copy_from_slice(&point.normalized.to_le_bytes());
        offset += 16;
    }
    writer.write_all(&packet[..offset])?;
    writer.flush()
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
    if &header[..4] != b"HXVB"
        || !matches!(version, 1 | 2)
        || header[if version == 1 { 10 } else { 12 }..16]
            .iter()
            .any(|b| *b != 0)
    {
        return Err(invalid());
    }
    let frames = u16::from_le_bytes([header[6], header[7]]);
    let count = usize::from(u16::from_le_bytes([header[8], header[9]]));
    let parameters = usize::from(u16::from_le_bytes([header[10], header[11]]));
    if frames == 0
        || usize::from(frames) > MAX_FRAMES
        || count > MAX_EVENTS
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
        block
            .events
            .push(MidiEvent {
                frame: u16::from_le_bytes([bytes[0], bytes[1]]),
                len: bytes[2],
                bytes: [bytes[3], bytes[4], bytes[5]],
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
    block
        .valid(binding, frames, block.sequence)
        .map_err(|_| invalid())?;
    Ok(Some(block))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            (10, 65),
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
