//! Concrete File encoding for productive control-work chunks. No renderer or
//! runtime owner is stored here, and no implicit BufWriter allocation is used.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Writing,
    WritePending,
    Finished,
    Failed,
}

pub(super) struct BufferedWavFile {
    file: File,
    encoding: WavEncoding,
    frames: u64,
    written: u64,
    phase: Phase,
    // Float32 stereo is the largest supported encoding: 256 * 2 * 4 bytes.
    encoded: [u8; FRAMES_PER_BLOCK * 8],
}
impl BufferedWavFile {
    pub(super) fn new(
        mut file: File,
        sample_rate: u32,
        frames: u64,
        data_bytes: u32,
        byte_rate: u32,
        encoding: WavEncoding,
    ) -> Result<Self, ExportError> {
        write_header_encoded(&mut file, sample_rate, data_bytes, byte_rate, encoding)?;
        Ok(Self {
            file,
            encoding,
            frames,
            written: 0,
            phase: Phase::Writing,
            encoded: [0; FRAMES_PER_BLOCK * 8],
        })
    }

    pub(super) fn sampler_fields(
        &self,
    ) -> Result<hex_engine::sampler_host_fields::WavWriterFields<'_>, hex_engine::AudioStorageError>
    {
        let Self {
            file,
            encoding: _,
            frames: _,
            written,
            phase,
            encoded,
        } = self;
        if *phase != Phase::Writing || *written != 0 {
            return Err(hex_engine::AudioStorageError::UnknownStorage);
        }
        Ok(hex_engine::sampler_host_fields::WavWriterFields {
            body: hex_engine::sampler_host_fields::BodyRef::of(self),
            file,
            encoded,
        })
    }

    pub(super) fn written(&self) -> u64 {
        self.written
    }

    /// Offset correlation rejects a duplicate/wrong chunk before touching File.
    /// A failed/partial write is sticky: callers must abandon this staged file,
    /// rather than retrying a block whose OS write may already have progressed.
    pub(super) fn append(
        &mut self,
        expected_offset: u64,
        block: &[hex_engine::Frame],
    ) -> Result<(), ExportError> {
        if self.phase != Phase::Writing || expected_offset != self.written {
            return Err(ExportError::Invalid("staged wav block is not current"));
        }
        if block.is_empty() || block.len() > FRAMES_PER_BLOCK {
            return Err(ExportError::Invalid("invalid staged wav block size"));
        }
        let end = self
            .written
            .checked_add(block.len() as u64)
            .filter(|end| *end <= self.frames)
            .ok_or(ExportError::Invalid("staged wav block exceeds frame range"))?;
        let mut offset = 0;
        for frame in block {
            for sample in frame {
                if !sample.is_finite() {
                    self.phase = Phase::Failed;
                    return Err(ExportError::Invalid("render produced a non-finite sample"));
                }
                match self.encoding {
                    WavEncoding::Pcm16 => {
                        let pcm = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                        self.encoded[offset..offset + 2].copy_from_slice(&pcm.to_le_bytes());
                        offset += 2;
                    }
                    WavEncoding::Float32 => {
                        self.encoded[offset..offset + 4].copy_from_slice(&sample.to_le_bytes());
                        offset += 4;
                    }
                }
            }
        }
        self.phase = Phase::WritePending;
        if let Err(error) = write_all(&mut self.file, &self.encoded[..offset], "write wav samples")
        {
            self.phase = Phase::Failed;
            return Err(error);
        }
        self.written = end;
        self.phase = Phase::Writing;
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), ExportError> {
        if self.phase == Phase::Finished {
            return Ok(());
        }
        if self.phase != Phase::Writing || self.written != self.frames {
            return Err(ExportError::Invalid("staged wav is incomplete or failed"));
        }
        self.phase = Phase::WritePending;
        let result = self
            .file
            .flush()
            .map_err(|source| ExportError::Io {
                operation: "flush the temporary wav",
                source,
            })
            .and_then(|()| {
                self.file.sync_all().map_err(|source| ExportError::Io {
                    operation: "synchronize the temporary wav",
                    source,
                })
            });
        self.phase = if result.is_ok() {
            Phase::Finished
        } else {
            Phase::Failed
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hex-explicit-block-writer-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn writer(&self, name: &str, frames: u64, encoding: WavEncoding) -> BufferedWavFile {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.0.join(name))
                .unwrap();
            BufferedWavFile::new(
                file,
                48_000,
                frames,
                u32::try_from(frames * u64::from(encoding.frame_bytes())).unwrap(),
                48_000 * encoding.frame_bytes(),
                encoding,
            )
            .unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn actual_file_irregular_chunks_encode_pcm16_and_float32_with_exact_offsets() {
        let dir = Scratch::new();
        for (name, encoding, sample_bytes) in [
            ("pcm.wav", WavEncoding::Pcm16, &[0x00, 0x40, 0x00, 0xc0][..]),
            (
                "float.wav",
                WavEncoding::Float32,
                &[0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00, 0xbf][..],
            ),
        ] {
            let mut writer = dir.writer(name, 300, encoding);
            let frames = [[0.5, -0.5]; FRAMES_PER_BLOCK];
            assert!(writer.finish().is_err(), "unfinished file cannot publish");
            let mut offset = 0;
            for count in [1, 17, 239, 43] {
                assert!(writer.append(offset + 1, &frames[..count]).is_err());
                assert_eq!(writer.written(), offset);
                writer.append(offset, &frames[..count]).unwrap();
                assert!(writer.append(offset, &frames[..count]).is_err());
                offset += count as u64;
                assert_eq!(writer.written(), offset);
            }
            writer.finish().unwrap();
            writer.finish().unwrap();
            assert!(writer.append(offset, &frames[..1]).is_err());
            drop(writer);
            let bytes = fs::read(dir.0.join(name)).unwrap();
            assert_eq!(bytes.len(), 44 + 300 * sample_bytes.len());
            assert!(
                bytes[44..]
                    .chunks_exact(sample_bytes.len())
                    .all(|sample| sample == sample_bytes)
            );
            assert_eq!(
                u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
                300 * encoding.frame_bytes()
            );
            assert_eq!(
                u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
                match encoding {
                    WavEncoding::Pcm16 => 1,
                    WavEncoding::Float32 => 3,
                }
            );
        }
    }

    #[test]
    fn nonfinite_block_fails_before_file_write_and_cannot_be_retried() {
        let dir = Scratch::new();
        for encoding in [WavEncoding::Pcm16, WavEncoding::Float32] {
            let name = match encoding {
                WavEncoding::Pcm16 => "pcm.wav",
                WavEncoding::Float32 => "float.wav",
            };
            let mut writer = dir.writer(name, 2, encoding);
            assert!(writer.append(0, &[[0.5, 0.5], [f32::NAN, 0.0]]).is_err());
            assert_eq!(writer.written(), 0);
            assert_eq!(fs::metadata(dir.0.join(name)).unwrap().len(), 44);
            assert!(writer.append(0, &[[0.0; 2]; 2]).is_err());
            assert!(writer.finish().is_err());
            assert!(writer.phase == Phase::Failed);
        }
    }

    #[test]
    fn real_file_write_failure_latches_and_zero_frame_file_finishes_once() {
        let dir = Scratch::new();
        let mut writer = dir.writer("readonly.wav", 1, WavEncoding::Pcm16);
        // Replace only this private test's actual File with a read-only handle;
        // write_all must fail through the real File path, not a fake sink.
        writer.file = File::open(dir.0.join("readonly.wav")).unwrap();
        assert!(matches!(
            writer.append(0, &[[0.0; 2]]),
            Err(ExportError::Io { .. })
        ));
        assert!(writer.phase == Phase::Failed);
        assert_eq!(writer.written(), 0);
        assert!(writer.append(0, &[[0.0; 2]]).is_err());
        assert!(writer.finish().is_err());
        let mut empty = dir.writer("empty.wav", 0, WavEncoding::Pcm16);
        assert!(empty.append(0, &[]).is_err());
        assert!(empty.append(0, &[[0.0; 2]]).is_err());
        empty.finish().unwrap();
        empty.finish().unwrap();
        assert_eq!(fs::metadata(dir.0.join("empty.wav")).unwrap().len(), 44);
    }
}
