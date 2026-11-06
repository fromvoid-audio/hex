//! Off-callback WAV sink. A new directory reserves the take's namespace. Each
//! successful write advances all files together. A write failure preserves the
//! last common prefix, which is finalized as a readable interrupted take.

use super::{End, FRAMES, Reader, Source};
use std::{
    fs::{File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

const HEADER: u64 = 44;
const FRAME_BYTES: u64 = 8;
const MAX_FRAMES: u64 = (u32::MAX as u64 - (HEADER - 8)) / FRAME_BYTES;

#[derive(Clone, Debug)]
pub struct Take {
    pub source: Source,
    pub path: PathBuf,
    pub frames: u64,
    pub sample_rate: u32,
}

#[derive(Debug)]
pub struct Takes {
    pub takes: Vec<Take>,
    pub end: End,
    /// A sink write failed or the WAV size limit was reached. Each returned
    /// take still contains the same verified-length prefix.
    pub write_error: Option<io::Error>,
}

/// Run on a recording worker, with a dedicated directory that must not exist.
/// The caller can Stop through the paired control endpoint. If preparation or
/// finalization fails, the newly created directory remains available for
/// recovery and no earlier take is modified.
pub fn write_takes(mut reader: Reader, directory: &Path) -> io::Result<Takes> {
    std::fs::create_dir(directory)?;
    let rate = reader.sample_rate();
    let mut files = Vec::with_capacity(reader.sources().len());
    for &source in reader.sources() {
        let name = match source {
            Source::Track(id) => format!("track-{id}.wav"),
            Source::Return(id) => format!("return-{}.wav", id.value()),
            Source::Main => "main.wav".into(),
        };
        let path = directory.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        write_header(&mut file, rate, 0)?;
        files.push((source, path, file));
    }
    reader.ready();
    let mut written = 0;
    let mut write_error = None;
    let mut encoded = [0; FRAMES * FRAME_BYTES as usize];
    let end = loop {
        let terminal = reader.drain(|frames, _, pcm| {
            if write_error.is_some() {
                return;
            }
            let end = written + frames as u64;
            if end > MAX_FRAMES {
                write_error = Some(io::Error::other(
                    "sample recording exceeded the WAV size limit",
                ));
                return;
            }
            for (layer, (_, _, file)) in files.iter_mut().enumerate() {
                let Some(samples) = Reader::source_frames(pcm, frames, layer) else {
                    write_error = Some(io::Error::other("sample recording packet is incomplete"));
                    return;
                };
                for (bytes, frame) in encoded.chunks_exact_mut(8).zip(samples) {
                    bytes[..4].copy_from_slice(&frame[0].to_le_bytes());
                    bytes[4..].copy_from_slice(&frame[1].to_le_bytes());
                }
                if let Err(error) = file.write_all(&encoded[..frames * 8]) {
                    write_error = Some(error);
                    return;
                }
            }
            written = end;
        });
        if let Some(end) = terminal {
            break end;
        }
        if write_error.is_some() {
            reader.stop();
            break End::StorageFailed;
        }
        thread::sleep(Duration::from_millis(1));
    };
    let mut takes = Vec::with_capacity(files.len());
    for (source, path, mut file) in files {
        file.set_len(HEADER + written * FRAME_BYTES)?;
        file.seek(SeekFrom::Start(0))?;
        write_header(&mut file, rate, written)?;
        file.sync_all()?;
        if written != 0 {
            takes.push(Take {
                source,
                path,
                frames: written,
                sample_rate: rate,
            });
        }
    }
    Ok(Takes {
        takes,
        end,
        write_error,
    })
}

fn write_header(file: &mut File, rate: u32, frames: u64) -> io::Result<()> {
    let bytes = u32::try_from(frames * FRAME_BYTES)
        .map_err(|_| io::Error::other("invalid sample recording WAV size"))?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?; // IEEE float
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 8).to_le_bytes())?;
    file.write_all(&8u16.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())
}
