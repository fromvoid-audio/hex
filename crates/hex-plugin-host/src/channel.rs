//! Bounded control/worker socket I/O with kernel wakeups. Never used on the audio callback.
use crate::{Error, Result, protocol};
use std::{
    io::{Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    time::{Duration, Instant},
};

// An arbitrary caller-intent check has no wakeup descriptor. Cap each idle
// control-side poll timeout at 10 ms so intent is revisited without extending
// the original deadline. This adds up to 100 timer wakeups per second; OS
// scheduling may delay them and readiness or signals may wake the poll sooner.
const CHECK_INTERVAL: Duration = Duration::from_millis(10);

// Dense PCM requests with stereo sidechain key and a grouped editor touch are
// the largest steady-state audio exchange. Reserve only `PCM_BYTES` upfront so
// steady-state instances stay ~12-20 KiB instead of 16 MiB per lane; rare large
// control/state-capture frames grow fallibly on demand and shrink back after
// decode.
pub(crate) const PCM_BYTES: usize = crate::transport::wire::MAX_DENSE_PACKET_BYTES
    + crate::transport::MAX_FRAMES * 8
    + protocol::EditorTouchPlanSet::WIRE_BYTES;
const _: () = assert!(PCM_BYTES <= protocol::PACKET_LIMIT);
const _: () = assert!(PCM_BYTES <= hex_shm_ring::SLOT_CAPACITY);

pub(crate) struct Packet {
    bytes: Vec<u8>,
    pub len: usize,
}
impl Packet {
    pub fn new() -> Result<Self> {
        let mut bytes = protocol::buffer(PCM_BYTES)?;
        bytes.resize(PCM_BYTES, 0);
        Ok(Self { bytes, len: 0 })
    }

    /// Release transient large state-capture storage and return this packet to
    /// its steady-state `PCM_BYTES` footprint.
    pub fn shrink_to_pcm(&mut self) {
        if self.bytes.capacity() > PCM_BYTES {
            if let Ok(mut compact) = protocol::buffer(PCM_BYTES) {
                compact.resize(PCM_BYTES, 0);
                self.bytes = compact;
            } else {
                self.bytes.truncate(PCM_BYTES);
                self.bytes.shrink_to(PCM_BYTES);
            }
        } else if self.bytes.len() > PCM_BYTES {
            self.bytes.truncate(PCM_BYTES);
        }
        self.len = 0;
    }

    pub fn storage_bytes(&self) -> usize {
        self.bytes.capacity()
    }
    /// Exchange requests and replies use the same fixed storage in sequence.
    /// A request encoder must send only the prefix it wrote.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn request_buffer(&mut self) -> Result<&mut [u8]> {
        self.len = 0;
        self.bytes.get_mut(..PCM_BYTES).ok_or(Error::Protocol(
            "instance request extent exceeds packet storage",
        ))
    }

    pub fn data(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn magic(&self) -> &[u8] {
        &self.bytes[..self.len.min(8)]
    }
    pub fn body(&self) -> Result<&[u8]> {
        self.data()
            .get(12..)
            .ok_or(Error::Protocol("truncated control header"))
    }
}
pub(crate) struct Channel {
    stream: UnixStream,
}
impl Channel {
    pub fn new(stream: UnixStream) -> Result<Self> {
        stream
            .set_nonblocking(true)
            .map_err(|e| Error::IoAt("socket blocking mode", e))?;
        Ok(Self { stream })
    }
    /// Create a bounded shared-memory PCM ring, transfer its descriptor once
    /// over `SCM_RIGHTS` before foreign module entry, and immediately close the
    /// transient descriptor so steady-state instances hold zero extra FDs.
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub fn create_and_send_shm_fd(
        &mut self,
        until: Instant,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<hex_shm_ring::HostRing> {
        let (ring, fd) = hex_shm_ring::HostRing::create()
            .map_err(|e| Error::IoAt("create shared-memory audio ring", e))?;
        loop {
            check()?;
            deadline(until)?;
            match hex_shm_ring::send_shm_fd(&self.stream, fd.as_fd()) {
                Ok(()) => {
                    drop(fd);
                    check()?;
                    return Ok(ring);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.wait(rustix::event::PollFlags::OUT, until)?;
                }
                Err(e) => return Err(Error::IoAt("send shared-memory descriptor", e)),
            }
        }
    }
    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
    /// Write one audio-quantum PCM request into the bounded shared-memory ring
    /// and signal the worker with a 1-byte doorbell write (1 syscall, zero socket
    /// payload copy). Falls back to direct socket send when SHM is not attached.
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub fn send_pcm(
        &mut self,
        bytes: &[u8],
        shm: Option<&mut hex_shm_ring::HostRing>,
        until: Instant,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        if let Some(shm) = shm {
            shm.write_request(bytes)
                .map_err(|_| Error::Protocol("shared-memory request ring refused"))?;
            let doorbell = [hex_shm_ring::DOORBELL_BYTE];
            self.send(&doorbell, until, check)
        } else {
            self.send(bytes, until, check)
        }
    }
    /// A complete direct reap does not require EOF from inherited descriptors,
    /// but bytes already queued after a terminal are never accepted as benign.
    pub fn reject_queued_after_reap(&mut self) -> Result<()> {
        let mut byte = [0];
        match self.stream.read(&mut byte) {
            Ok(0) => Ok(()),
            Ok(_) => Err(Error::Protocol("trailing data after instance terminal")),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(Error::IoAt("terminal socket read", e)),
        }
    }
    pub fn send(
        &mut self,
        bytes: &[u8],
        until: Instant,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut at = 0;
        while at < bytes.len() {
            check()?;
            deadline(until)?;
            match self.stream.write(&bytes[at..]) {
                Ok(0) => return Err(Error::Protocol("instance channel closed while writing")),
                Ok(n) => at += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.wait(rustix::event::PollFlags::OUT, until)?
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
        check()
    }
    pub fn read(
        &mut self,
        out: &mut Packet,
        until: Instant,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        self.read_with_shm(out, None, until, check)
    }

    /// Read either a 1-byte SHM doorbell reply (`0x01`) or a regular socket
    /// control/error frame (`'H'...`).
    pub fn read_with_shm(
        &mut self,
        out: &mut Packet,
        shm: Option<&mut hex_shm_ring::HostRing>,
        until: Instant,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        out.len = 0;
        if let Some(shm) = shm {
            let mut marker = [0_u8; 1];
            loop {
                check()?;
                deadline(until)?;
                match self.stream.read(&mut marker) {
                    Ok(0) => {
                        check()?;
                        return Err(Error::Protocol("instance channel ended"));
                    }
                    Ok(1) => break,
                    Ok(_) => return Err(Error::Protocol("unexpected marker read length")),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        self.wait(rustix::event::PollFlags::IN, until)?
                    }
                    Err(e) => return Err(Error::Io(e)),
                }
            }
            if marker[0] == hex_shm_ring::DOORBELL_BYTE {
                let reply_buf = out.bytes.get_mut(..PCM_BYTES).ok_or(Error::Protocol(
                    "shared-memory reply exceeds initialized packet storage",
                ))?;
                let len = shm
                    .read_reply(reply_buf)
                    .map_err(|_| Error::Protocol("shared-memory reply ring refused"))?;
                if len < crate::transport::wire::HEADER_BYTES
                    || &out.bytes[..4] != b"HXVB"
                    || protocol::pcm_packet_extent(&out.bytes[..64])? != len
                {
                    return Err(Error::Protocol("malformed shared-memory pcm reply"));
                }
                out.len = len;
                return check();
            }
            if marker[0] != b'H' {
                return Err(Error::Protocol("unknown instance control frame"));
            }
            out.bytes[0] = b'H';
            out.len = 1;
        }
        let mut wanted = 4;
        loop {
            check()?;
            deadline(until)?;
            if out.len == wanted {
                if wanted == 4 {
                    wanted = if &out.bytes[..4] == b"HXVB" || &out.bytes[..4] == b"HXVE" {
                        64
                    } else {
                        12
                    };
                    continue;
                }
                if wanted == 12 {
                    let magic = &out.bytes[..8];
                    if ![
                        b"HXVIFA05",
                        b"HXVIRD06",
                        b"HXVIQF04",
                        b"HXVIAR03",
                        b"HXVIRR04",
                        b"HXVICS02",
                        &protocol::CAPTURE_REFUSAL_MAGIC,
                        b"HXVIFL01",
                        b"HXVIGE01",
                        b"HXVIGE02",
                        b"HXVIMD03",
                        b"HXVLCA02",
                        b"HXVIER01",
                        b"HXVITR01",
                    ]
                    .iter()
                    .any(|m| magic == *m)
                    {
                        return Err(Error::Protocol("unknown instance control frame"));
                    }
                    let n = u32::from_le_bytes(
                        out.bytes[8..12]
                            .try_into()
                            .map_err(|_| Error::Protocol("control length"))?,
                    ) as usize;
                    if n > protocol::control_limit(magic) {
                        return Err(Error::Protocol("instance control extent exceeds bound"));
                    }
                    wanted = 12 + n;
                    if wanted > out.bytes.len() {
                        if wanted > protocol::PACKET_LIMIT {
                            return Err(Error::Protocol(
                                "control frame exceeds packet reservation",
                            ));
                        }
                        if wanted > out.bytes.capacity() {
                            out.bytes
                                .try_reserve_exact(wanted - out.bytes.len())
                                .map_err(|_| Error::Protocol("control frame allocation refused"))?;
                        }
                        out.bytes.resize(wanted, 0);
                    }
                    if wanted == 12 {
                        return Ok(());
                    }
                    continue;
                }
                if wanted == 64 && (&out.bytes[..4] == b"HXVB" || &out.bytes[..4] == b"HXVE") {
                    wanted = protocol::pcm_packet_extent(&out.bytes[..64])?;
                    if wanted == 64 {
                        return Ok(());
                    }
                    continue;
                }
                return check();
            }
            let remaining = out
                .bytes
                .get_mut(out.len..wanted)
                .ok_or(Error::Protocol("frame exceeds initialized packet storage"))?;
            match self.stream.read(remaining) {
                Ok(0) => {
                    check()?;
                    return Err(Error::Protocol(if out.len == 0 {
                        "instance channel ended"
                    } else {
                        "truncated instance frame"
                    }));
                }
                Ok(n) => out.len += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.wait(rustix::event::PollFlags::IN, until)?
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }
    fn wait(&self, flags: rustix::event::PollFlags, until: Instant) -> Result<()> {
        let duration = remaining(until)?.min(CHECK_INTERVAL);
        let timeout = rustix::event::Timespec {
            tv_sec: duration
                .as_secs()
                .try_into()
                .map_err(|_| Error::Protocol("socket deadline overflow"))?,
            tv_nsec: duration.subsec_nanos().into(),
        };
        let mut descriptors = [rustix::event::PollFd::new(&self.stream, flags)];
        match rustix::event::poll(&mut descriptors, Some(&timeout)) {
            Ok(_) => Ok(()),
            Err(rustix::io::Errno::INTR) => Ok(()),
            Err(e) => Err(Error::IoAt("socket readiness", e.into())),
        }
    }
}
pub(crate) fn end_after(duration: Duration) -> Result<Instant> {
    Instant::now()
        .checked_add(duration)
        .ok_or(Error::Protocol("supervision duration overflow"))
}
fn deadline(until: Instant) -> Result<()> {
    if Instant::now() >= until {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}
pub(crate) fn pause(until: Instant) {
    std::thread::sleep(
        until
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(1)),
    );
}

fn remaining(until: Instant) -> Result<Duration> {
    until
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or(Error::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    fn cancellation_while_blocked(write: bool) {
        let (mut stream, peer) = UnixStream::pair().unwrap();
        if write {
            stream.set_nonblocking(true).unwrap();
            let bytes = [0_u8; 8192];
            let mut full = false;
            for _ in 0..512 {
                match stream.write(&bytes) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        full = true;
                        break;
                    }
                    Err(error) => panic!("fill socket: {error}"),
                }
            }
            assert!(full, "bounded fixture must fill the socket");
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut channel = Channel::new(stream).unwrap();
            let mut first = true;
            let mut check = || {
                if worker_cancelled.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                if first {
                    first = false;
                    ready_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                Ok(())
            };
            let until = end_after(Duration::from_secs(2)).unwrap();
            let result = if write {
                channel.send(&[1], until, &mut check)
            } else {
                channel.read(&mut Packet::new().unwrap(), until, &mut check)
            };
            result_tx
                .send(matches!(result, Err(Error::Cancelled)))
                .unwrap();
        });
        ready_rx.recv_timeout(Duration::from_millis(250)).unwrap();
        cancelled.store(true, Ordering::Release);
        release_tx.send(()).unwrap();
        let prompt = result_rx.recv_timeout(Duration::from_millis(200));
        // Wake the old full-deadline implementation before joining on failure.
        drop(peer);
        worker.join().unwrap();
        assert_eq!(
            prompt.ok(),
            Some(true),
            "cancellation must interrupt an idle socket wait"
        );
    }

    #[test]
    fn caller_cancellation_interrupts_idle_read() {
        cancellation_while_blocked(false);
    }

    #[test]
    fn caller_cancellation_interrupts_backpressured_write() {
        cancellation_while_blocked(true);
    }

    #[test]
    fn idle_read_keeps_its_original_deadline() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(stream).unwrap();
        let start = Instant::now();
        let deadline = start + Duration::from_millis(40);
        let result = channel.read(&mut Packet::new().unwrap(), deadline, &mut || Ok(()));
        assert!(matches!(result, Err(Error::Timeout)));
        assert!(
            Instant::now() >= deadline,
            "poll slices must not shorten the deadline"
        );
    }

    #[test]
    fn parameter_packet_framing_consumes_exact_payload_without_eating_the_successor() {
        use crate::transport::{Block, Events, ParameterPoint, Parameters};
        use hex_lattice_contract::ForeignProcessorBinding;
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let mut block = Block {
            binding: ForeignProcessorBinding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            sequence: 0,
            first_frame: 0,
            frames: 8,
            pcm: [[0.5; 2]; crate::transport::MAX_FRAMES],
            events: Events::default(),
            parameters: Parameters::default(),
            timing: None,
            timing_changes: Default::default(),
        };
        block
            .parameters
            .push(ParameterPoint {
                frame: 7,
                id: 0,
                normalized: 0.125,
            })
            .unwrap();
        crate::transport::wire::write(&mut peer, &block).unwrap();
        block.parameters = Parameters::default();
        block.sequence = 1;
        block.first_frame = 8;
        crate::transport::wire::write(&mut peer, &block).unwrap();
        drop(peer);
        let mut channel = Channel::new(stream).unwrap();
        let mut packet = Packet::new().unwrap();
        let until = end_after(Duration::from_secs(2)).unwrap();
        channel.read(&mut packet, until, &mut || Ok(())).unwrap();
        let first = crate::transport::wire::read(&mut packet.data())
            .unwrap()
            .unwrap();
        assert_eq!(
            first.parameters.as_slice(),
            &[ParameterPoint {
                frame: 7,
                id: 0,
                normalized: 0.125
            }]
        );
        assert_eq!(packet.len, 64 + 8 * 8 + 16);
        channel.read(&mut packet, until, &mut || Ok(())).unwrap();
        let second = crate::transport::wire::read(&mut packet.data())
            .unwrap()
            .unwrap();
        assert_eq!(second.sequence, 1);
        assert!(second.parameters.as_slice().is_empty());
        assert_eq!(packet.len, 64 + 8 * 8);
    }
    #[test]
    fn hostile_frames_refuse_before_unbounded_reads_or_allocation() {
        let mut oversized = b"HXVIRD06".to_vec();
        oversized.extend(u32::MAX.to_le_bytes());
        let mut events = vec![0; 64];
        events[..4].copy_from_slice(b"HXVB");
        events[8] = 1;
        let mut frames = events.clone();
        frames[8] = 0;
        frames[6..8].copy_from_slice(&257_u16.to_le_bytes());
        for bytes in [
            oversized,
            events,
            frames,
            b"unknown!\0\0\0\0".to_vec(),
            b"HXVI".to_vec(),
        ] {
            let (stream, mut peer) = UnixStream::pair().unwrap();
            peer.write_all(&bytes).unwrap();
            drop(peer);
            let mut channel = Channel::new(stream).unwrap();
            let mut packet = Packet::new().unwrap();
            let initial = packet.bytes.len();
            let error = channel
                .read(
                    &mut packet,
                    end_after(Duration::from_secs(1)).unwrap(),
                    &mut || Ok(()),
                )
                .unwrap_err();
            assert!(matches!(error, Error::Protocol(_)), "{error:?}");
            assert_eq!(packet.bytes.len(), initial);
        }
    }
    #[test]
    fn state_reply_grows_beyond_pcm_on_demand_and_shrinks_back_to_pcm() {
        let mut packet = Packet::new().unwrap();
        let initial = packet.bytes.len();
        assert_eq!(initial, PCM_BYTES);
        assert!(initial < protocol::PACKET_LIMIT / 2);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            assert_eq!(packet.request_buffer().unwrap().len(), initial);
            let mut header = [0_u8; crate::transport::wire::HEADER_BYTES];
            header[..4].copy_from_slice(b"HXVB");
            let flags = 2
                | protocol::EDITOR_PCM_BIT
                | protocol::EDITOR_GROUP_TOUCH_BIT
                | protocol::SDK_TIMING_BIT
                | protocol::SDK_THREAD_CPU_BIT;
            header[4..6].copy_from_slice(&flags.to_le_bytes());
            header[6..8].copy_from_slice(&(crate::transport::MAX_FRAMES as u16).to_le_bytes());
            header[10..12]
                .copy_from_slice(&(crate::transport::MAX_PARAMETER_POINTS as u16).to_le_bytes());
            header[12..14].fill(protocol::MAX_PARAMETER_EDITS as u8);
            assert!(protocol::pcm_packet_extent(&header).unwrap() <= initial);
        }
        assert_eq!(packet.storage_bytes(), PCM_BYTES);
        let body = vec![0x5a; initial + 1];
        let frame = protocol::control(*b"HXVICS02", &body).unwrap();
        let frame_len = frame.len();
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || peer.write_all(&frame).unwrap());
        Channel::new(stream)
            .unwrap()
            .read(
                &mut packet,
                end_after(Duration::from_secs(1)).unwrap(),
                &mut || Ok(()),
            )
            .unwrap();
        writer.join().unwrap();
        assert_eq!(packet.body().unwrap(), body);
        assert!(packet.storage_bytes() >= frame_len);
        packet.shrink_to_pcm();
        assert_eq!(packet.storage_bytes(), PCM_BYTES);
        assert_eq!(packet.bytes.len(), PCM_BYTES);
        assert_eq!(packet.len, 0);
    }

    #[test]
    fn queued_completed_terminal_is_read_after_peer_exit() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let mut body = Vec::new();
        protocol::stamp(
            &mut body,
            hex_lattice_contract::ForeignProcessorBinding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
        );
        body.extend(4_u64.to_le_bytes());
        body.extend(32_u64.to_le_bytes());
        body.extend(0_u32.to_le_bytes());
        body.extend([1, 1]);
        let expected = protocol::control(*b"HXVITR01", &body).unwrap();
        peer.write_all(&expected).unwrap();
        drop(peer);
        let mut channel = Channel::new(stream).unwrap();
        let mut packet = Packet::new().unwrap();
        channel
            .read(
                &mut packet,
                end_after(Duration::from_secs(1)).unwrap(),
                &mut || Ok(()),
            )
            .unwrap();
        assert_eq!(packet.data(), expected);
        assert_eq!(packet.magic(), b"HXVITR01");
        assert_eq!(packet.body().unwrap(), body);
        channel.reject_queued_after_reap().unwrap();
    }
}

#[cfg(test)]
mod send_tests {
    use super::*;
    #[test]
    fn control_send_close_and_expired_deadline_keep_bounded_socket_semantics() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(stream).unwrap();
        let bytes = protocol::control(*b"HXVIST02", &[]).unwrap();
        channel
            .send(
                &bytes,
                end_after(Duration::from_secs(1)).unwrap(),
                &mut || Ok(()),
            )
            .unwrap();
        let mut received = vec![0; bytes.len()];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(received, bytes);
        let expired = Instant::now();
        pause(expired);
        assert!(matches!(
            channel.send(&bytes, expired, &mut || Ok(())),
            Err(Error::Timeout)
        ));
        channel.close();
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }
}

#[cfg(test)]
mod shm_tests {
    use super::*;
    use crate::transport::{Block, Events, MidiEvent, ParameterPoint, Parameters};
    use hex_lattice_contract::ForeignProcessorBinding;

    fn sample_block(frames: u16, sequence: u64) -> Block {
        let mut block = Block {
            binding: ForeignProcessorBinding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            sequence,
            first_frame: sequence * u64::from(frames),
            frames,
            pcm: [[0.0; 2]; crate::transport::MAX_FRAMES],
            events: Events::default(),
            parameters: Parameters::default(),
            timing: None,
            timing_changes: Default::default(),
        };
        for i in 0..usize::from(frames) {
            block.pcm[i] = [(i as f32) * 0.001, -((i as f32) * 0.0005)];
        }
        block
            .events
            .push(MidiEvent {
                frame: 0,
                bytes: [0x90, 60, 100],
                len: 3,
                note_id: None,
                is_live: false,
            })
            .unwrap();
        block
            .parameters
            .push(ParameterPoint {
                frame: frames - 1,
                id: 7,
                normalized: 0.75,
            })
            .unwrap();
        block
    }

    #[test]
    fn shm_doorbell_round_trip_supports_64_128_and_256_quanta_with_one_byte_socket_signaling() {
        for frames in [64_u16, 128, 256] {
            let (host_stream, mut worker_stream) = UnixStream::pair().unwrap();
            let mut channel = Channel::new(host_stream).unwrap();
            let until = end_after(Duration::from_secs(2)).unwrap();
            let mut host_ring = channel
                .create_and_send_shm_fd(until, &mut || Ok(()))
                .unwrap();

            let worker_fd = hex_shm_ring::recv_shm_fd(&worker_stream).unwrap();
            let mut worker_ring = hex_shm_ring::WorkerRing::from_fd(worker_fd).unwrap();

            let mut host_packet = Packet::new().unwrap();
            let mut worker_buf = vec![0_u8; hex_shm_ring::SLOT_CAPACITY];

            // Run 12 exchanges per quantum (3x ring wraparound) verifying exact payload
            // delivery and strictly 1-byte doorbell traffic on the socket.
            for seq in 0..12_u64 {
                let req = sample_block(frames, seq);
                let req_len = crate::transport::wire::encode_into_slice(
                    host_packet.request_buffer().unwrap(),
                    &req,
                )
                .unwrap();
                channel
                    .send_pcm(
                        &host_packet.bytes[..req_len],
                        Some(&mut host_ring),
                        until,
                        &mut || Ok(()),
                    )
                    .unwrap();

                let mut doorbell = [0_u8; 1];
                worker_stream.read_exact(&mut doorbell).unwrap();
                assert_eq!(doorbell[0], hex_shm_ring::DOORBELL_BYTE);
                let got_len = worker_ring.read_request(&mut worker_buf).unwrap();
                assert_eq!(got_len, req_len);
                let decoded_req = crate::transport::wire::read(&mut &worker_buf[..got_len])
                    .unwrap()
                    .unwrap();
                assert_eq!(decoded_req.frames, frames);
                assert_eq!(decoded_req.sequence, seq);
                assert_eq!(
                    decoded_req.pcm[..usize::from(frames)],
                    req.pcm[..usize::from(frames)]
                );

                // Worker builds a reply (inverting polarity, clearing input notes).
                let mut rep = decoded_req;
                rep.events = Events::default();
                for frame in &mut rep.pcm[..usize::from(frames)] {
                    frame[0] = -frame[0];
                    frame[1] = -frame[1];
                }
                let rep_len =
                    crate::transport::wire::encode_into_slice(&mut worker_buf, &rep).unwrap();
                worker_ring.write_reply(&worker_buf[..rep_len]).unwrap();
                worker_stream
                    .write_all(&[hex_shm_ring::DOORBELL_BYTE])
                    .unwrap();

                channel
                    .read_with_shm(
                        &mut host_packet,
                        Some(&mut host_ring),
                        until,
                        &mut || Ok(()),
                    )
                    .unwrap();
                assert_eq!(host_packet.len, rep_len);
                let decoded_rep = crate::transport::wire::read(&mut host_packet.data())
                    .unwrap()
                    .unwrap();
                assert_eq!(decoded_rep.frames, frames);
                assert_eq!(decoded_rep.sequence, seq);
                assert_eq!(
                    decoded_rep.pcm[..usize::from(frames)],
                    rep.pcm[..usize::from(frames)]
                );
            }
        }
    }

    #[test]
    fn shm_channel_still_reads_socket_control_and_error_frames_and_refuses_hostile_shm_replies() {
        let (host_stream, mut worker_stream) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(host_stream).unwrap();
        let until = end_after(Duration::from_secs(2)).unwrap();
        let mut host_ring = channel
            .create_and_send_shm_fd(until, &mut || Ok(()))
            .unwrap();
        let mut worker_ring =
            hex_shm_ring::WorkerRing::from_fd(hex_shm_ring::recv_shm_fd(&worker_stream).unwrap())
                .unwrap();

        // Worker sends an ordinary HXVIER01 socket error frame while SHM is attached.
        let mut err_body = Vec::new();
        err_body.extend(1_u16.to_le_bytes());
        err_body.push(0);
        err_body.extend(7_u32.to_le_bytes());
        let err_frame = protocol::control(*b"HXVIER01", &err_body).unwrap();
        worker_stream.write_all(&err_frame).unwrap();
        let mut packet = Packet::new().unwrap();
        channel
            .read_with_shm(&mut packet, Some(&mut host_ring), until, &mut || Ok(()))
            .unwrap();
        assert_eq!(packet.magic(), b"HXVIER01");
        assert_eq!(packet.body().unwrap(), err_body);

        // Worker sends a doorbell pointing to a malformed SHM reply (wrong extent).
        let mut bad_reply = [0_u8; 80];
        bad_reply[..4].copy_from_slice(b"HXVB");
        bad_reply[4..6].copy_from_slice(&1_u16.to_le_bytes());
        bad_reply[6..8].copy_from_slice(&64_u16.to_le_bytes()); // claims 64 frames = 576 bytes, not 80
        worker_ring.write_reply(&bad_reply).unwrap();
        worker_stream
            .write_all(&[hex_shm_ring::DOORBELL_BYTE])
            .unwrap();
        let err = channel
            .read_with_shm(&mut packet, Some(&mut host_ring), until, &mut || Ok(()))
            .unwrap_err();
        assert!(matches!(err, Error::Protocol(_)), "{err:?}");

        // Peer EOF is detected immediately on the doorbell socket.
        drop(worker_stream);
        let eof = channel
            .read_with_shm(&mut packet, Some(&mut host_ring), until, &mut || Ok(()))
            .unwrap_err();
        assert!(matches!(eof, Error::Protocol("instance channel ended")));
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn steady_state_shm_pcm_exchange_performs_zero_heap_allocations() {
        let (host_stream, mut worker_stream) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(host_stream).unwrap();
        let until = end_after(Duration::from_secs(2)).unwrap();
        let mut host_ring = channel
            .create_and_send_shm_fd(until, &mut || Ok(()))
            .unwrap();
        let mut worker_ring =
            hex_shm_ring::WorkerRing::from_fd(hex_shm_ring::recv_shm_fd(&worker_stream).unwrap())
                .unwrap();

        let worker = std::thread::spawn(move || {
            let mut buf = vec![0_u8; hex_shm_ring::SLOT_CAPACITY];
            let mut doorbell = [0_u8; 1];
            for _ in 0..8 {
                worker_stream.read_exact(&mut doorbell).unwrap();
                let n = worker_ring.read_request(&mut buf).unwrap();
                worker_ring.write_reply(&buf[..n]).unwrap();
                worker_stream
                    .write_all(&[hex_shm_ring::DOORBELL_BYTE])
                    .unwrap();
            }
        });

        let mut packet = Packet::new().unwrap();
        let mut block = sample_block(128, 0);
        // Echo replies carry zero MIDI events per protocol::pcm_packet_extent.
        block.events = Events::default();
        let measured = allocation_counter::measure(|| {
            for _ in 0..8 {
                let len = crate::transport::wire::encode_into_slice(
                    packet.request_buffer().unwrap(),
                    &block,
                )
                .unwrap();
                channel
                    .send_pcm(
                        &packet.bytes[..len],
                        Some(&mut host_ring),
                        until,
                        &mut || Ok(()),
                    )
                    .unwrap();
                channel
                    .read_with_shm(&mut packet, Some(&mut host_ring), until, &mut || Ok(()))
                    .unwrap();
            }
        });
        worker.join().unwrap();
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn sixteen_parallel_shm_workers_exchange_quanta_cleanly() {
        const PLUGINS: usize = 16;
        const QUANTA: u64 = 16;
        let frames = 128_u16;
        let handles: Vec<_> = (0..PLUGINS)
            .map(|plugin_idx| {
                std::thread::spawn(move || {
                    let (host_stream, mut worker_stream) = UnixStream::pair().unwrap();
                    let mut channel = Channel::new(host_stream).unwrap();
                    let until = end_after(Duration::from_secs(5)).unwrap();
                    let mut host_ring = channel
                        .create_and_send_shm_fd(until, &mut || Ok(()))
                        .unwrap();
                    let mut worker_ring = hex_shm_ring::WorkerRing::from_fd(
                        hex_shm_ring::recv_shm_fd(&worker_stream).unwrap(),
                    )
                    .unwrap();
                    let worker = std::thread::spawn(move || {
                        let mut buf = vec![0_u8; hex_shm_ring::SLOT_CAPACITY];
                        let mut doorbell = [0_u8; 1];
                        for _ in 0..QUANTA {
                            worker_stream.read_exact(&mut doorbell).unwrap();
                            let n = worker_ring.read_request(&mut buf).unwrap();
                            worker_ring.write_reply(&buf[..n]).unwrap();
                            worker_stream
                                .write_all(&[hex_shm_ring::DOORBELL_BYTE])
                                .unwrap();
                        }
                    });
                    let mut packet = Packet::new().unwrap();
                    for seq in 0..QUANTA {
                        let mut block = sample_block(frames, seq);
                        block.binding.instance = (plugin_idx + 1) as u64;
                        block.events = Events::default();
                        let len = crate::transport::wire::encode_into_slice(
                            packet.request_buffer().unwrap(),
                            &block,
                        )
                        .unwrap();
                        channel
                            .send_pcm(
                                &packet.bytes[..len],
                                Some(&mut host_ring),
                                until,
                                &mut || Ok(()),
                            )
                            .unwrap();
                        channel
                            .read_with_shm(&mut packet, Some(&mut host_ring), until, &mut || Ok(()))
                            .unwrap();
                        let rep = crate::transport::wire::read(&mut packet.data())
                            .unwrap()
                            .unwrap();
                        assert_eq!(rep.binding.instance, (plugin_idx + 1) as u64);
                        assert_eq!(rep.sequence, seq);
                        assert_eq!(rep.frames, frames);
                    }
                    worker.join().unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    #[ignore = "explicit release benchmark: cargo test -p hex-plugin-host --release bench_16_plugin_shm_vs_socket -- --ignored --nocapture"]
    fn bench_16_plugin_shm_vs_socket() {
        const PLUGINS: usize = 16;
        const QUANTA: u64 = 500;
        for frames in [64_u16, 128, 256] {
            let deadline_ns = (u64::from(frames) * 1_000_000_000) / 48_000;
            for use_shm in [false, true] {
                let start = Instant::now();
                let handles: Vec<_> = (0..PLUGINS)
                    .map(|idx| {
                        std::thread::spawn(move || {
                            let (host_stream, mut worker_stream) = UnixStream::pair().unwrap();
                            let mut channel = Channel::new(host_stream).unwrap();
                            let until = end_after(Duration::from_secs(10)).unwrap();
                            let (mut host_ring, mut worker_ring) = if use_shm {
                                let hr = channel
                                    .create_and_send_shm_fd(until, &mut || Ok(()))
                                    .unwrap();
                                let wr = hex_shm_ring::WorkerRing::from_fd(
                                    hex_shm_ring::recv_shm_fd(&worker_stream).unwrap(),
                                )
                                .unwrap();
                                (Some(hr), Some(wr))
                            } else {
                                (None, None)
                            };
                            let mut block = sample_block(frames, 0);
                            block.binding.instance = (idx + 1) as u64;
                            block.events = Events::default();
                            let mut sample_buf = vec![0_u8; PCM_BYTES];
                            let packet_len =
                                crate::transport::wire::encode_into_slice(&mut sample_buf, &block)
                                    .unwrap();
                            let worker = std::thread::spawn(move || {
                                let mut buf = vec![0_u8; hex_shm_ring::SLOT_CAPACITY];
                                if let Some(mut ring) = worker_ring.take() {
                                    let mut doorbell = [0_u8; 1];
                                    for _ in 0..QUANTA {
                                        worker_stream.read_exact(&mut doorbell).unwrap();
                                        let n = ring.read_request(&mut buf).unwrap();
                                        ring.write_reply(&buf[..n]).unwrap();
                                        worker_stream
                                            .write_all(&[hex_shm_ring::DOORBELL_BYTE])
                                            .unwrap();
                                    }
                                } else {
                                    for _ in 0..QUANTA {
                                        worker_stream.read_exact(&mut buf[..packet_len]).unwrap();
                                        worker_stream.write_all(&buf[..packet_len]).unwrap();
                                    }
                                }
                            });
                            let mut packet = Packet::new().unwrap();
                            let mut latencies_ns = Vec::with_capacity(QUANTA as usize);
                            let mut misses = 0_u64;
                            for seq in 0..QUANTA {
                                block.sequence = seq;
                                block.first_frame = seq * u64::from(frames);
                                let q_start = Instant::now();
                                let len = crate::transport::wire::encode_into_slice(
                                    packet.request_buffer().unwrap(),
                                    &block,
                                )
                                .unwrap();
                                channel
                                    .send_pcm(
                                        &packet.bytes[..len],
                                        host_ring.as_mut(),
                                        until,
                                        &mut || Ok(()),
                                    )
                                    .unwrap();
                                channel
                                    .read_with_shm(
                                        &mut packet,
                                        host_ring.as_mut(),
                                        until,
                                        &mut || Ok(()),
                                    )
                                    .unwrap();
                                let elapsed = q_start.elapsed().as_nanos() as u64;
                                if elapsed > deadline_ns {
                                    misses += 1;
                                }
                                latencies_ns.push(elapsed);
                            }
                            worker.join().unwrap();
                            (latencies_ns, misses)
                        })
                    })
                    .collect();
                let mut all_ns = Vec::with_capacity(PLUGINS * QUANTA as usize);
                let mut total_misses = 0_u64;
                for handle in handles {
                    let (mut ns, misses) = handle.join().unwrap();
                    all_ns.append(&mut ns);
                    total_misses += misses;
                }
                let wall = start.elapsed();
                all_ns.sort_unstable();
                let p50 = all_ns[all_ns.len() / 2];
                let p95 = all_ns[(all_ns.len() * 95) / 100];
                let p99 = all_ns[(all_ns.len() * 99) / 100];
                let mode = if use_shm { "shm-ring" } else { "unix-sock" };
                eprintln!(
                    "bench 16x {mode} q={frames}: wall={:.2}ms p50={:.1}us p95={:.1}us p99={:.1}us misses={total_misses}/{}",
                    wall.as_secs_f64() * 1e3,
                    p50 as f64 / 1e3,
                    p95 as f64 / 1e3,
                    p99 as f64 / 1e3,
                    all_ns.len()
                );
            }
        }
    }
}
