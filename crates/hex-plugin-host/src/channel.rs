//! Bounded control/worker socket I/O with kernel wakeups. Never used on the audio callback.
use crate::{Error, Result, protocol};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

// An arbitrary caller-intent check has no wakeup descriptor. Cap each idle
// control-side poll timeout at 10 ms so intent is revisited without extending
// the original deadline. This adds up to 100 timer wakeups per second; OS
// scheduling may delay them and readiness or signals may wake the poll sooner.
const CHECK_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct Packet {
    pub bytes: [u8; protocol::PACKET_LIMIT],
    pub len: usize,
}
impl Packet {
    pub fn new() -> Self {
        Self {
            bytes: [0; protocol::PACKET_LIMIT],
            len: 0,
        }
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
    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
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
        out.len = 0;
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
                        b"HXVIFA03",
                        b"HXVIRD04",
                        b"HXVIQF02",
                        b"HXVIAR01",
                        b"HXVIRR02",
                        b"HXVICS01",
                        &protocol::CAPTURE_REFUSAL_MAGIC,
                        b"HXVIFL01",
                        b"HXVIGE01",
                        b"HXVIMD01",
                        b"HXVLCA01",
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
                    if n > protocol::CONTROL_LIMIT {
                        return Err(Error::Protocol("instance control extent exceeds bound"));
                    }
                    wanted = 12 + n;
                    if wanted == 12 {
                        return Ok(());
                    }
                    continue;
                }
                if wanted == 64 && (&out.bytes[..4] == b"HXVB" || &out.bytes[..4] == b"HXVE") {
                    let frames = usize::from(u16::from_le_bytes([out.bytes[6], out.bytes[7]]));
                    let version = u16::from_le_bytes([out.bytes[4], out.bytes[5]]);
                    let parameters =
                        usize::from(u16::from_le_bytes([out.bytes[10], out.bytes[11]]));
                    if frames > crate::transport::MAX_FRAMES
                        || out.bytes[8..10] != [0, 0]
                        || !matches!(version, 1 | 2)
                        || (version == 1 && parameters != 0)
                        || parameters > crate::transport::MAX_PARAMETER_POINTS
                        || out.bytes[12..16] != [0; 4]
                        || (parameters != 0 && (frames == 0 || &out.bytes[..4] != b"HXVB"))
                    {
                        return Err(Error::Protocol("unsupported instance PCM extent or events"));
                    }
                    wanted = 64 + frames * 8 + parameters * 16;
                    if wanted == 64 {
                        return Ok(());
                    }
                    continue;
                }
                return check();
            }
            match self.stream.read(&mut out.bytes[out.len..wanted]) {
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
                channel.read(&mut Packet::new(), until, &mut check)
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
        let result = channel.read(&mut Packet::new(), deadline, &mut || Ok(()));
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
        let mut packet = Packet::new();
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
        let mut oversized = b"HXVIRD04".to_vec();
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
            let error = channel
                .read(
                    &mut Packet::new(),
                    end_after(Duration::from_secs(1)).unwrap(),
                    &mut || Ok(()),
                )
                .unwrap_err();
            assert!(matches!(error, Error::Protocol(_)), "{error:?}");
        }
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
        let mut packet = Packet::new();
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
        let bytes = protocol::control(*b"HXVIST01", &[]).unwrap();
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
