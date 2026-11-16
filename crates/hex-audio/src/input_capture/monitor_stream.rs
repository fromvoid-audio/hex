//! Bounded packet adaptation for a single live input monitor.
//!
//! The output owner fills at most 256 frames per call. A fixed 512-frame
//! buffer is primed before sound starts. Each call stages at most 32 packets
//! and 512 input frames, plus bounded output copying and compaction. Sequence
//! and within-callback offsets are checked; timestamps from distinct input
//! callbacks do not prove physical continuity. A post-start shortage or
//! sequence discontinuity latches a terminal instead of emitting a partial
//! block. Input and output nominal rates must match; independent clocks can
//! still drift. This is not resampling or proof of physical latency. The owner
//! must destroy this stream on control only after output callback quiescence.
use super::{FRAMES, InputMonitor, InputMonitorEnd, InputPacket, PACKETS};
use hex_engine::Frame;

const BUFFER_FRAMES: usize = FRAMES * 2;

/// A complete output chunk is available, or the fixed startup buffer is filling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorBlock {
    Priming,
    Ready,
}

/// A terminal reason is latched except for an invalid output extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorStreamError {
    InvalidBlock,
    Input(InputMonitorEnd),
    Discontinuity,
    Underrun,
}

/// Bounded callback reader retaining the original monitor endpoint.
pub struct MonitorStream {
    monitor: InputMonitor,
    pending: Option<InputPacket>,
    pending_offset: usize,
    buffer: [Frame; BUFFER_FRAMES],
    buffered: usize,
    next_sequence: Option<u64>,
    previous_packet: Option<(cpal::StreamInstant, u32, u32)>,
    started: bool,
    terminal: Option<MonitorStreamError>,
}

impl MonitorStream {
    /// Construct on control using the output graph's sample rate. A mismatch
    /// returns the exact input owner; no sample-rate conversion is performed.
    /// The first unread packet must begin an input callback (offset zero).
    pub fn new_on_control(monitor: InputMonitor, output_rate: u32) -> Result<Self, InputMonitor> {
        if monitor.source().1.sample_rate != output_rate {
            return Err(monitor);
        }
        Ok(Self {
            monitor,
            pending: None,
            pending_offset: 0,
            buffer: [[0.0; 2]; BUFFER_FRAMES],
            buffered: 0,
            next_sequence: None,
            previous_packet: None,
            started: false,
            terminal: None,
        })
    }

    /// The original device identity and nominal input format.
    pub fn source(&self) -> (&crate::input_devices::InputDeviceId, super::InputConfig) {
        self.monitor.source()
    }

    /// Return the exact endpoint on control after output callback quiescence.
    pub fn into_monitor_on_control(self) -> InputMonitor {
        self.monitor
    }

    /// Produce at most 256 frames without allocation, blocking or owner drops.
    /// Priming and terminal errors silence valid output. An oversized block is
    /// refused untouched; the caller owns its error handling. Empty output
    /// consumes no packets.
    pub fn read_block(&mut self, output: &mut [Frame]) -> Result<MonitorBlock, MonitorStreamError> {
        if output.len() > FRAMES {
            return Err(MonitorStreamError::InvalidBlock);
        }
        output.fill([0.0; 2]);
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if let Err(reason) = self.monitor.status() {
            return self.end(MonitorStreamError::Input(reason));
        }
        if output.is_empty() {
            return Ok(if self.started {
                MonitorBlock::Ready
            } else {
                MonitorBlock::Priming
            });
        }
        let mut popped = 0;
        while self.buffered < BUFFER_FRAMES {
            if self.pending.is_none() {
                if popped == PACKETS {
                    break;
                }
                let packet = match self.monitor.try_read() {
                    Ok(Some(packet)) => packet,
                    Ok(None) => break,
                    Err(reason) => return self.end(MonitorStreamError::Input(reason)),
                };
                popped += 1;
                let count = packet.frames().len();
                let valid_order = match self.previous_packet {
                    None => packet.offset_frames == 0,
                    Some((stamp, _, _)) if packet.offset_frames == 0 => packet.captured_at >= stamp,
                    Some((stamp, offset, frames)) => {
                        packet.captured_at == stamp
                            && offset.checked_add(frames) == Some(packet.offset_frames)
                    }
                };
                if count == 0
                    || count > FRAMES
                    || !valid_order
                    || packet
                        .offset_frames
                        .checked_add(count as u32)
                        .is_none_or(|end| end > (FRAMES * PACKETS) as u32)
                    || self
                        .next_sequence
                        .is_some_and(|next| next != packet.sequence)
                {
                    return self.end(MonitorStreamError::Discontinuity);
                }
                self.previous_packet =
                    Some((packet.captured_at, packet.offset_frames, count as u32));
                let Some(next) = packet.sequence.checked_add(1) else {
                    return self.end(MonitorStreamError::Discontinuity);
                };
                self.next_sequence = Some(next);
                self.pending = Some(packet);
                self.pending_offset = 0;
            }
            if let Some(packet) = self.pending.as_ref() {
                let remaining = &packet.frames()[self.pending_offset..];
                let count = remaining.len().min(BUFFER_FRAMES - self.buffered);
                self.buffer[self.buffered..self.buffered + count]
                    .copy_from_slice(&remaining[..count]);
                self.buffered += count;
                self.pending_offset += count;
                if self.pending_offset == packet.frames().len() {
                    self.pending = None;
                }
            }
        }
        if let Err(reason) = self.monitor.status() {
            return self.end(MonitorStreamError::Input(reason));
        }
        if !self.started && self.buffered < BUFFER_FRAMES {
            return Ok(MonitorBlock::Priming);
        }
        if self.buffered < output.len() {
            return self.end(MonitorStreamError::Underrun);
        }
        self.started = true;
        output.copy_from_slice(&self.buffer[..output.len()]);
        self.buffer.copy_within(output.len()..self.buffered, 0);
        self.buffered -= output.len();
        Ok(MonitorBlock::Ready)
    }

    fn end<T>(&mut self, error: MonitorStreamError) -> Result<T, MonitorStreamError> {
        self.terminal = Some(error);
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DEVICE_FAILED, NO_FAULT, read, tests::monitored_channel};
    use super::*;
    use std::sync::atomic::Ordering::{Acquire, Release};

    #[test]
    fn primes_and_repartitions_mono_and_stereo_without_loss() {
        for channels in [1, 2] {
            let (mut writer, mut recording, monitor, shared) = monitored_channel(channels);
            let monitor = match MonitorStream::new_on_control(monitor, 44_100) {
                Err(original) => original,
                Ok(_) => panic!("rate mismatch must retain the owner"),
            };
            assert_eq!(monitor.source().1.sample_rate, 48_000);
            let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
            let input: Vec<f32> = (0..512 * channels).map(|n| n as f32 / 2048.0).collect();
            let stamp = cpal::StreamInstant::new(1, 0);
            writer.submit(&input[..200 * channels], stamp);
            let recorded = read(&mut recording, &shared).unwrap().unwrap();
            assert_eq!(recorded.frames, 200);
            let mut first = [[1.0; 2]; 64];
            assert_eq!(stream.read_block(&mut first), Ok(MonitorBlock::Priming));
            assert_eq!(first, [[0.0; 2]; 64]);
            writer.submit(&input[200 * channels..], stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
            assert!(read(&mut recording, &shared).unwrap().is_some());
            let mut second = [[0.0; 2]; 256];
            let mut third = [[0.0; 2]; 192];
            assert_eq!(stream.read_block(&mut first), Ok(MonitorBlock::Ready));
            assert_eq!(stream.read_block(&mut second), Ok(MonitorBlock::Ready));
            assert_eq!(stream.read_block(&mut third), Ok(MonitorBlock::Ready));
            let actual: Vec<_> = first.into_iter().chain(second).chain(third).collect();
            let expected: Vec<_> = input
                .chunks_exact(channels)
                .map(|frame| {
                    if channels == 1 {
                        [frame[0], frame[0]]
                    } else {
                        [frame[0], frame[1]]
                    }
                })
                .collect();
            assert_eq!(actual, expected);
            assert_eq!(shared.fault.load(Acquire), NO_FAULT);
        }
    }

    #[test]
    fn extent_stop_failure_and_underrun_are_explicit_and_do_not_stop_recording() {
        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        assert_eq!(stream.read_block(&mut []), Ok(MonitorBlock::Priming));
        let mut oversized = [[1.0; 2]; 257];
        assert_eq!(
            stream.read_block(&mut oversized),
            Err(MonitorStreamError::InvalidBlock)
        );
        assert_eq!(oversized, [[1.0; 2]; 257]);
        let stamp = cpal::StreamInstant::new(1, 0);
        writer.submit(&[0.5; 512], stamp);
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert!(read(&mut recording, &shared).unwrap().is_some());
        let mut block = [[0.0; 2]; 256];
        assert_eq!(stream.read_block(&mut block), Ok(MonitorBlock::Ready));
        assert_eq!(stream.read_block(&mut block), Ok(MonitorBlock::Ready));
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Underrun)
        );
        assert_eq!(block, [[0.0; 2]; 256]);
        writer.submit(&[0.25; 256], stamp);
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Underrun)
        );
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);
        assert!(!shared.stop.load(Acquire));

        for failure in [false, true] {
            let (mut writer, _recording, monitor, shared) = monitored_channel(1);
            let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
            writer.submit(&[0.5; 512], stamp);
            assert_eq!(stream.read_block(&mut block), Ok(MonitorBlock::Ready));
            if failure {
                shared.fail(DEVICE_FAILED);
            } else {
                shared.stop.store(true, Release);
            }
            let reason = if failure {
                InputMonitorEnd::InputFailure(super::super::InputEnd::DeviceFailed)
            } else {
                InputMonitorEnd::StopRequested
            };
            assert_eq!(
                stream.read_block(&mut block),
                Err(MonitorStreamError::Input(reason))
            );
            assert_eq!(block, [[0.0; 2]; 256]);
        }
    }

    #[test]
    fn packet_gap_and_queue_overrun_latch_without_cancelling_recording() {
        let stamp = cpal::StreamInstant::new(1, 0);
        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        writer.submit(&[0.5; 256], stamp);
        writer.sequence += 1; // Inject a missing monitor packet.
        writer.submit(&[0.5; 256], stamp);
        let mut block = [[1.0; 2]; 64];
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Discontinuity)
        );
        assert_eq!(block, [[0.0; 2]; 64]);
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Discontinuity)
        );
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);

        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        writer.submit(&[0.5; 256], stamp);
        writer
            .monitor
            .as_mut()
            .unwrap()
            .output
            .push(super::super::Block {
                stamp,
                offset_frames: 128,
                sequence: 1,
                frames: 256,
                pcm: [[0.5; 2]; 256],
            })
            .ok()
            .unwrap();
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Discontinuity)
        );
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);

        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        for _ in 0..=PACKETS {
            writer.submit(&[0.5; 256], stamp);
            assert!(read(&mut recording, &shared).unwrap().is_some());
        }
        assert_eq!(
            stream.read_block(&mut block),
            Err(MonitorStreamError::Input(InputMonitorEnd::Overrun))
        );
        assert_eq!(block, [[0.0; 2]; 64]);
        writer.submit(&[0.25; 256], stamp);
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert_eq!(shared.fault.load(Acquire), NO_FAULT);
    }

    #[test]
    fn buffered_physical_packets_feed_the_prepared_track() {
        use hex_project::{Command, Created, Editor, Project};
        use std::{collections::BTreeMap, sync::Arc};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "input".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut playback =
            hex_engine::playback::Playback::try_from_prepared_with_session(revision).unwrap();
        playback.apply(hex_engine::PlaybackCommand::Play);
        let (mut writer, mut recording, monitor, shared) = monitored_channel(1);
        writer.submit(&[0.25; 512], cpal::StreamInstant::new(1, 0));
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        let mut input = [[0.0; 2]; 64];
        assert_eq!(stream.read_block(&mut input), Ok(MonitorBlock::Ready));
        let mut output = [[0.0; 2]; 64];
        playback
            .render_with_live_track_audio(
                &mut output,
                track,
                &input,
                None,
                None,
                None,
                &mut hex_lattice::RefuseForeign,
            )
            .unwrap();
        assert!(
            output
                .iter()
                .all(|frame| frame[0] > 0.0 && frame[0] == frame[1])
        );
        assert!(read(&mut recording, &shared).unwrap().is_some());
        assert!(read(&mut recording, &shared).unwrap().is_some());
    }

    #[test]
    #[cfg(feature = "allocation-probe")]
    fn callback_reads_and_returns_monitor_owner_without_allocating_or_freeing() {
        let (mut writer, _recording, monitor, _shared) = monitored_channel(1);
        writer.submit(&[0.25; 512], cpal::StreamInstant::new(1, 0));
        let stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        let (mut guard, mut terminal) = crate::native_callback_owner::reserve(stream);
        let mut output = [[0.0; 2]; 256];
        let mut result = Ok(MonitorBlock::Priming);
        let counts = allocation_counter::measure(|| {
            result = guard.owner_mut().unwrap().read_block(&mut output);
            guard.close();
        });
        assert_eq!(result, Ok(MonitorBlock::Ready));
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(terminal.ready());
        let stream = terminal.take().unwrap();
        let original = stream.into_monitor_on_control();
        drop(original);

        let (mut writer, _recording, monitor, _shared) = monitored_channel(1);
        writer.submit(&[0.25; 512], cpal::StreamInstant::new(1, 0));
        let mut stream = MonitorStream::new_on_control(monitor, 48_000).ok().unwrap();
        assert_eq!(stream.read_block(&mut output), Ok(MonitorBlock::Ready));
        assert_eq!(stream.read_block(&mut output), Ok(MonitorBlock::Ready));
        let (mut guard, mut terminal) = crate::native_callback_owner::reserve(stream);
        let mut result = Ok(MonitorBlock::Ready);
        let counts = allocation_counter::measure(|| {
            result = guard.owner_mut().unwrap().read_block(&mut output);
            guard.close();
        });
        assert_eq!(result, Err(MonitorStreamError::Underrun));
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(terminal.ready());
        drop(terminal.take());
    }
}
