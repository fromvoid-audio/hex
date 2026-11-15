//! Off-callback physical-input WAV writing and bounded capture-clock indexing.
//! Input timestamps retain their original stream origin; they are not an
//! Arrangement clock or evidence of physical input/output synchronization.
use super::{Block, FRAMES, InputCapture, InputEnd};
use crate::sample_recording::takes::{FRAME_BYTES, HEADER, MAX_FRAMES, write_header};
use std::{
    fs::{File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

// At most one entry per native callback. Split packets in one callback share
// an entry. A full index interrupts capture rather than losing timing silently.
const MAX_TIMING: usize = 1_048_576;

/// One original callback's accepted part in the take. `captured_at` is the
/// timestamp supplied by CPAL; `offset_frames` is relative to that callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputTiming<T = cpal::StreamInstant> {
    pub take_start: u64,
    pub frames: u32,
    pub captured_at: T,
    pub offset_frames: u32,
}

/// Why the finite take ended. Only `Stopped` indicates a normal Stop.
#[derive(Debug)]
pub enum InputTakeEnd {
    Stopped,
    Empty,
    Capture(InputEnd),
    InvalidPacket,
    TimingLimit,
    WavLimit,
    Storage(io::Error),
}

/// An already finalized WAV. An interrupted end is not a successful take;
/// its frame count and index describe only the complete prefix on disk.
pub struct InputTake {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub frames: u64,
    pub timing: Vec<InputTiming>,
    pub end: InputTakeEnd,
    /// Backend detail, when opening or starting the input failed.
    pub open_error: Option<String>,
}

/// Prepare the new file and drain a capture on a non-audio worker. If the
/// capture has not started, start it only after its WAV header is written.
/// A capture started by the caller can overflow while file setup is in progress.
/// The caller must retain its independent stop handle. `path` must not already
/// exist. Setup or finalization errors return an error; the file is not removed.
pub fn write_input_take(mut capture: InputCapture, path: &Path) -> io::Result<InputTake> {
    let rate = capture.source().1.sample_rate;
    let file = open_take(path)?;
    let result = run(&mut capture, file, rate, MAX_TIMING, MAX_FRAMES)?;
    Ok(InputTake {
        path: path.to_owned(),
        sample_rate: rate,
        frames: result.frames,
        timing: result.timing,
        end: result.end,
        open_error: capture.take_open_error_on_control(),
    })
}

fn open_take(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

trait Source {
    type Stamp: Copy + Ord;
    fn start(&mut self);
    fn read(&mut self) -> Result<Option<Block<Self::Stamp>>, InputEnd>;
    fn stop(&self);
}
impl Source for InputCapture {
    type Stamp = cpal::StreamInstant;
    fn start(&mut self) {
        loop {
            match self.phase() {
                super::InputPhase::Opening => thread::sleep(Duration::from_millis(1)),
                super::InputPhase::Ready => {
                    if self.start_on_control() {
                        break;
                    }
                }
                _ => break,
            }
        }
    }
    fn read(&mut self) -> Result<Option<Block<Self::Stamp>>, InputEnd> {
        super::read(&mut self.input, &self.shared)
    }
    fn stop(&self) {
        self.stop_on_control();
    }
}

trait Output: Write + Seek {
    fn truncate(&mut self, bytes: u64) -> io::Result<()>;
    fn sync(&mut self) -> io::Result<()>;
}
impl Output for File {
    fn truncate(&mut self, bytes: u64) -> io::Result<()> {
        self.set_len(bytes)
    }
    fn sync(&mut self) -> io::Result<()> {
        self.sync_all()
    }
}

struct ResultTake<T> {
    frames: u64,
    timing: Vec<InputTiming<T>>,
    end: InputTakeEnd,
}

fn run<S: Source, W: Output>(
    source: &mut S,
    mut file: W,
    rate: u32,
    timing_limit: usize,
    frame_limit: u64,
) -> io::Result<ResultTake<S::Stamp>> {
    if !(8_000..=192_000).contains(&rate) {
        source.stop();
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid input sample rate",
        ));
    }
    if let Err(error) = write_header(&mut file, rate, 0) {
        source.stop();
        return Err(error);
    }
    source.start();
    let mut frames = 0_u64;
    let mut sequence = 0_u64;
    let mut timing: Vec<InputTiming<S::Stamp>> = Vec::new();
    let mut encoded = [0_u8; FRAMES * FRAME_BYTES as usize];
    let end = loop {
        let block = match source.read() {
            Ok(Some(block)) => block,
            Ok(None) => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(InputEnd::Stopped) => {
                break if frames == 0 {
                    InputTakeEnd::Empty
                } else {
                    InputTakeEnd::Stopped
                };
            }
            Err(error) => break InputTakeEnd::Capture(error),
        };
        let count = block.frames;
        let valid = count > 0
            && count <= FRAMES
            && block
                .offset_frames
                .checked_add(count as u32)
                .is_some_and(|n| n <= super::MAX_CALLBACK_FRAMES as u32)
            && block.sequence == sequence
            && block.pcm[..count].iter().flatten().all(|v| v.is_finite());
        let new_callback = block.offset_frames == 0;
        let ordered = match timing.last() {
            None => new_callback,
            Some(last) if new_callback => block.stamp >= last.captured_at,
            Some(last) => {
                block.stamp == last.captured_at
                    && last.offset_frames.checked_add(last.frames) == Some(block.offset_frames)
            }
        };
        if !valid || !ordered || sequence.checked_add(1).is_none() {
            source.stop();
            break InputTakeEnd::InvalidPacket;
        }
        if frames
            .checked_add(count as u64)
            .is_none_or(|end| end > frame_limit.min(MAX_FRAMES))
        {
            source.stop();
            break InputTakeEnd::WavLimit;
        }
        if new_callback && timing.len() == timing_limit {
            source.stop();
            break InputTakeEnd::TimingLimit;
        }
        if new_callback && timing.try_reserve(1).is_err() {
            source.stop();
            break InputTakeEnd::Storage(io::Error::other("could not reserve input timing index"));
        }
        for (bytes, frame) in encoded.chunks_exact_mut(8).zip(&block.pcm[..count]) {
            bytes[..4].copy_from_slice(&frame[0].to_le_bytes());
            bytes[4..].copy_from_slice(&frame[1].to_le_bytes());
        }
        if let Err(error) = file.write_all(&encoded[..count * 8]) {
            source.stop();
            break InputTakeEnd::Storage(error);
        }
        if new_callback {
            timing.push(InputTiming {
                take_start: frames,
                frames: count as u32,
                captured_at: block.stamp,
                offset_frames: block.offset_frames,
            });
        } else if let Some(last) = timing.last_mut() {
            last.frames += count as u32;
        }
        frames += count as u64;
        sequence += 1;
    };
    file.truncate(HEADER + frames * FRAME_BYTES)?;
    file.seek(SeekFrom::Start(0))?;
    write_header(&mut file, rate, frames)?;
    file.sync()?;
    Ok(ResultTake {
        frames,
        timing,
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        collections::VecDeque,
        io::Read,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Fake {
        blocks: VecDeque<Result<Option<Block<u64>>, InputEnd>>,
        stopped: Cell<bool>,
        started: bool,
        expected_header: Option<PathBuf>,
    }
    impl Source for Fake {
        type Stamp = u64;
        fn start(&mut self) {
            if let Some(path) = &self.expected_header {
                assert_eq!(std::fs::metadata(path).unwrap().len(), HEADER);
            }
            self.started = true;
        }
        fn read(&mut self) -> Result<Option<Block<u64>>, InputEnd> {
            assert!(
                self.started,
                "the sink must start its source before reading"
            );
            self.blocks.pop_front().unwrap_or(Err(InputEnd::Stopped))
        }
        fn stop(&self) {
            self.stopped.set(true);
        }
    }
    fn block(seq: u64, stamp: u64, offset: u32, samples: &[[f32; 2]]) -> Block<u64> {
        let mut pcm = [[0.0; 2]; FRAMES];
        pcm[..samples.len()].copy_from_slice(samples);
        Block {
            stamp,
            offset_frames: offset,
            sequence: seq,
            frames: samples.len(),
            pcm,
        }
    }
    fn source(blocks: Vec<Block<u64>>, end: InputEnd) -> Fake {
        let mut blocks: VecDeque<_> = blocks.into_iter().map(|block| Ok(Some(block))).collect();
        blocks.push_back(Err(end));
        Fake {
            blocks,
            stopped: Cell::new(false),
            started: false,
            expected_header: None,
        }
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "hex-input-take-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn file(&self) -> File {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&self.0)
                .unwrap()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn writes_pcm_and_ordered_callback_timing_to_a_readable_wav() {
        let temp = Temp::new();
        let samples = [[0.25, 0.25], [0.5, -0.5], [-0.75, 0.75]];
        let mut input = source(
            vec![
                block(0, 10, 0, &samples[..2]),
                block(1, 10, 2, &samples[2..]),
                block(2, 20, 0, &samples[..1]),
            ],
            InputEnd::Stopped,
        );
        input.expected_header = Some(temp.0.clone());
        let take = run(&mut input, temp.file(), 48_000, 8, MAX_FRAMES).unwrap();
        assert!(matches!(take.end, InputTakeEnd::Stopped));
        assert_eq!(take.frames, 4);
        assert_eq!(
            take.timing,
            vec![
                InputTiming {
                    take_start: 0,
                    frames: 3,
                    captured_at: 10,
                    offset_frames: 0
                },
                InputTiming {
                    take_start: 3,
                    frames: 1,
                    captured_at: 20,
                    offset_frames: 0
                }
            ]
        );
        let mut wav = hex_media::WavFrameReader::open(
            &temp.0,
            hex_media::WavReaderLimits::new(1_024, 4, 32, 4),
        )
        .unwrap();
        assert_eq!(wav.sample_rate(), 48_000);
        assert_eq!(wav.source_frames(), 4);
        assert_eq!(
            wav.read_frames(0, 4).unwrap(),
            vec![samples[0], samples[1], samples[2], samples[0]]
        );
    }

    #[test]
    fn gaps_invalid_samples_and_index_pressure_stop_with_a_valid_prefix() {
        for (second, limit, timing_limit) in [
            (block(2, 20, 0, &[[0.4, 0.4]]), false, 8),
            (block(1, 10, 4, &[[0.4, 0.4]]), false, 8),
            (block(1, 9, 0, &[[0.4, 0.4]]), false, 8),
            (block(1, 20, 0, &[[f32::NAN, 0.4]]), false, 8),
            (block(1, 20, 0, &[[0.4, 0.4]]), true, 1),
        ] {
            let temp = Temp::new();
            let mut input = source(
                vec![block(0, 10, 0, &[[0.2, 0.2]]), second],
                InputEnd::Stopped,
            );
            let take = run(&mut input, temp.file(), 44_100, timing_limit, MAX_FRAMES).unwrap();
            assert_eq!(take.frames, 1);
            assert!(input.stopped.get());
            assert!(if limit {
                matches!(take.end, InputTakeEnd::TimingLimit)
            } else {
                matches!(take.end, InputTakeEnd::InvalidPacket)
            });
            let mut wav = hex_media::WavFrameReader::open(
                &temp.0,
                hex_media::WavReaderLimits::new(1_024, 1, 32, 1),
            )
            .unwrap();
            assert_eq!(wav.read_frames(0, 1).unwrap(), vec![[0.2, 0.2]]);
        }
    }

    #[test]
    fn device_failure_is_not_reported_as_normal_stop() {
        let temp = Temp::new();
        let mut input = source(vec![block(0, 10, 0, &[[0.3, 0.3]])], InputEnd::DeviceFailed);
        let take = run(&mut input, temp.file(), 48_000, 8, MAX_FRAMES).unwrap();
        assert_eq!(take.frames, 1);
        assert!(matches!(
            take.end,
            InputTakeEnd::Capture(InputEnd::DeviceFailed)
        ));
        assert!(!input.stopped.get());
    }

    #[test]
    fn stopping_before_input_arrives_is_an_empty_take() {
        let temp = Temp::new();
        let mut input = source(vec![], InputEnd::Stopped);
        let take = run(&mut input, temp.file(), 48_000, 8, MAX_FRAMES).unwrap();
        assert_eq!(take.frames, 0);
        assert!(matches!(take.end, InputTakeEnd::Empty));
    }

    #[test]
    fn wav_size_limit_stops_before_publishing_the_excess_packet() {
        let temp = Temp::new();
        let mut input = source(
            vec![
                block(0, 10, 0, &[[0.2, 0.2]]),
                block(1, 20, 0, &[[0.4, 0.4]]),
            ],
            InputEnd::Stopped,
        );
        let take = run(&mut input, temp.file(), 48_000, 8, 1).unwrap();
        assert_eq!(take.frames, 1);
        assert!(matches!(take.end, InputTakeEnd::WavLimit));
        assert!(input.stopped.get());
        assert_eq!(
            std::fs::metadata(&temp.0).unwrap().len(),
            HEADER + FRAME_BYTES
        );
    }

    #[test]
    fn existing_take_is_never_overwritten() {
        let temp = Temp::new();
        let mut existing = temp.file();
        existing.write_all(b"existing audio").unwrap();
        drop(existing);
        assert_eq!(
            open_take(&temp.0).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&temp.0).unwrap(), b"existing audio");
    }

    #[test]
    fn writer_retains_the_original_open_error_for_its_consumer() {
        use std::sync::{Arc, atomic::AtomicU8, mpsc};
        let temp = Temp::new();
        let (_, input) = rtrb::RingBuffer::new(super::super::PACKETS);
        let shared = Arc::new(super::super::Shared {
            phase: AtomicU8::new(super::super::FINISHED),
            stop: std::sync::atomic::AtomicBool::new(true),
            fault: AtomicU8::new(super::super::OPEN_FAILED),
            terminal: AtomicU8::new(super::super::OPEN_FAILED + 1),
        });
        let (sender, error) = mpsc::sync_channel(1);
        sender.send("microphone unavailable".into()).unwrap();
        let capture = InputCapture {
            id: format!("{}:test-input", cpal::default_host().id())
                .parse()
                .unwrap(),
            config: super::super::InputConfig {
                sample_rate: 48_000,
                channels: 1,
            },
            input,
            shared,
            worker: thread::spawn(|| {}),
            error,
        };
        let take = write_input_take(capture, &temp.0).unwrap();
        assert!(matches!(
            take.end,
            InputTakeEnd::Capture(InputEnd::OpenFailed)
        ));
        assert_eq!(take.open_error.as_deref(), Some("microphone unavailable"));
    }

    struct PartialWrite(File, bool, u64);
    impl Write for PartialWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.stream_position()? >= self.2 && !self.1 {
                self.1 = true;
                self.0.write_all(&bytes[..bytes.len() / 2])?;
                return Err(io::Error::other("simulated disk full"));
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
    impl Seek for PartialWrite {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }
    impl Output for PartialWrite {
        fn truncate(&mut self, bytes: u64) -> io::Result<()> {
            self.0.set_len(bytes)
        }
        fn sync(&mut self) -> io::Result<()> {
            self.0.sync_all()
        }
    }
    #[test]
    fn partial_disk_write_returns_an_error_and_finalizes_only_the_prior_prefix() {
        let temp = Temp::new();
        let mut input = source(
            vec![
                block(0, 10, 0, &[[0.3, 0.3]]),
                block(1, 20, 0, &[[0.6, 0.6]]),
            ],
            InputEnd::Stopped,
        );
        let take = run(
            &mut input,
            PartialWrite(temp.file(), false, HEADER + FRAME_BYTES),
            48_000,
            8,
            MAX_FRAMES,
        )
        .unwrap();
        assert_eq!(take.frames, 1);
        assert!(matches!(take.end, InputTakeEnd::Storage(_)));
        assert!(input.stopped.get());
        let mut bytes = Vec::new();
        File::open(&temp.0)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes.len(), (HEADER + FRAME_BYTES) as usize);
        assert_eq!(&bytes[40..44], &8_u32.to_le_bytes());
    }

    #[test]
    fn header_failure_stops_without_starting_input() {
        let temp = Temp::new();
        let mut input = source(vec![block(0, 10, 0, &[[0.3, 0.3]])], InputEnd::Stopped);
        let result = run(
            &mut input,
            PartialWrite(temp.file(), false, 0),
            48_000,
            8,
            MAX_FRAMES,
        );
        assert!(result.is_err());
        assert!(input.stopped.get());
        assert!(!input.started);
    }
}
