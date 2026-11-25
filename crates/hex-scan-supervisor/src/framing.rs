//! Shared bounded little-endian packet framing for scanner offline protocols.

/// Magic plus little-endian length preceding every scanner protocol payload.
pub const FRAME_HEADER: usize = 12;

#[derive(Default)]
pub struct PacketWriter {
    bytes: Vec<u8>,
    overflowed: bool,
}

impl PacketWriter {
    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes.extend(value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes.extend(value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes.extend(value.to_le_bytes());
    }

    pub fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    pub fn try_blob(&mut self, value: &[u8]) -> Result<(), String> {
        let len = u32::try_from(value.len()).map_err(|_| {
            self.overflowed = true;
            "offline field capacity refused".to_owned()
        })?;
        self.u32(len);
        self.bytes.extend(value);
        Ok(())
    }

    pub fn blob(&mut self, value: &[u8]) {
        let _ = self.try_blob(value);
    }

    pub fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn into_packet(self, magic: &[u8; 8], max_packet: usize) -> Result<Vec<u8>, String> {
        if self.overflowed {
            return Err("offline field capacity refused".into());
        }
        frame_packet(magic, self.bytes, max_packet)
    }
}

pub fn frame_packet(magic: &[u8; 8], bytes: Vec<u8>, max_packet: usize) -> Result<Vec<u8>, String> {
    if bytes.len() > max_packet {
        return Err("offline packet capacity refused".into());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "offline packet capacity refused")?;
    let mut framed = Vec::with_capacity(FRAME_HEADER + bytes.len());
    framed.extend_from_slice(magic);
    framed.extend(len.to_le_bytes());
    framed.extend(bytes);
    Ok(framed)
}

pub struct PacketReader<'a> {
    bytes: &'a [u8],
    at: usize,
    reject_nul: bool,
}

impl<'a> PacketReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            reject_nul: false,
        }
    }

    pub fn packet(bytes: &'a [u8], magic: &[u8; 8], max_packet: usize) -> Result<Self, String> {
        let Some(frame_limit) = max_packet.checked_add(FRAME_HEADER) else {
            return Err("offline packet header/length refused".into());
        };
        if bytes.len() < FRAME_HEADER
            || &bytes[..8] != magic
            || bytes.len() > frame_limit
            || u32::from_le_bytes(bytes[8..FRAME_HEADER].try_into().map_err(|_| "length")?) as usize
                != bytes.len() - FRAME_HEADER
        {
            return Err("offline packet header/length refused".into());
        }
        Ok(Self::new(&bytes[FRAME_HEADER..]))
    }

    pub fn reject_nul(mut self) -> Self {
        self.reject_nul = true;
        self
    }

    pub fn is_eof(&self) -> bool {
        self.at == self.bytes.len()
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(count).ok_or("offset overflow")?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or("truncated offline packet")?;
        self.at = end;
        Ok(slice)
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| "u16")?,
        ))
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| "u32")?,
        ))
    }

    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| "u64")?,
        ))
    }

    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub fn blob(&mut self, max: usize) -> Result<Vec<u8>, String> {
        let count = self.u32()? as usize;
        if count > max {
            return Err("offline field capacity refused".into());
        }
        Ok(self.take(count)?.to_vec())
    }

    pub fn text(&mut self, max: usize) -> Result<String, String> {
        let value =
            String::from_utf8(self.blob(max)?).map_err(|_| "offline UTF8 refused".to_owned())?;
        if self.reject_nul && value.contains('\0') {
            return Err("offline NUL refused".into());
        }
        Ok(value)
    }

    pub fn strings(&mut self, max: usize) -> Result<Vec<String>, String> {
        let count = self.u32()? as usize;
        if count > max {
            return Err("offline record capacity refused".into());
        }
        (0..count).map(|_| self.text(512)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_round_trip_and_bounds() {
        let mut writer = PacketWriter::default();
        writer.u8(7);
        writer.u16(0x1234);
        writer.u32(0x89ab_cdef);
        writer.u64(0x0102_0304_0506_0708);
        writer.f64(0.75);
        writer.blob(b"hello");
        writer.u32(2);
        writer.blob(b"one");
        writer.blob(b"two");
        let framed = writer.into_packet(b"TESTPK01", 1024).unwrap();

        let mut reader = PacketReader::packet(&framed, b"TESTPK01", 1024)
            .unwrap()
            .reject_nul();
        assert_eq!(reader.take(1).unwrap(), &[7]);
        assert_eq!(reader.u16().unwrap(), 0x1234);
        assert_eq!(reader.u32().unwrap(), 0x89ab_cdef);
        assert_eq!(reader.u64().unwrap(), 0x0102_0304_0506_0708);
        assert_eq!(reader.f64().unwrap(), 0.75);
        assert_eq!(reader.text(16).unwrap(), "hello");
        assert_eq!(reader.strings(4).unwrap(), vec!["one", "two"]);
        assert!(reader.is_eof());
        assert!(reader.u16().is_err());

        assert!(PacketReader::packet(&framed, b"WRONG001", 1024).is_err());
        assert!(PacketReader::packet(&framed, b"TESTPK01", 8).is_err());
        assert!(frame_packet(b"TESTPK01", vec![0; 17], 16).is_err());
    }

    #[test]
    fn reject_nul_only_rejects_embedded_nul_when_enabled() {
        let mut writer = PacketWriter::default();
        writer.blob(b"a\0b");
        let framed = writer.into_packet(b"TESTPK01", 64).unwrap();

        let mut permissive = PacketReader::packet(&framed, b"TESTPK01", 64).unwrap();
        assert_eq!(permissive.text(8).unwrap(), "a\0b");

        let mut strict = PacketReader::packet(&framed, b"TESTPK01", 64)
            .unwrap()
            .reject_nul();
        assert_eq!(strict.text(8).unwrap_err(), "offline NUL refused");
    }

    #[test]
    fn try_blob_and_overflow_flag_fail_packet_framing_cleanly() {
        let mut writer = PacketWriter::default();
        writer.try_blob(b"ok").unwrap();
        let framed = writer.into_packet(b"TESTPK01", 64).unwrap();
        let mut reader = PacketReader::packet(&framed, b"TESTPK01", 64).unwrap();
        assert_eq!(reader.text(8).unwrap(), "ok");

        let overflowed = PacketWriter {
            overflowed: true,
            ..Default::default()
        };
        assert!(overflowed.into_packet(b"TESTPK01", 64).is_err());
    }
}
