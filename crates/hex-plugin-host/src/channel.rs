//! Bounded control/worker socket I/O with kernel wakeups. Never used on the audio callback.
use crate::{Error, Result, protocol};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

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
                        b"HXVIFA02",
                        b"HXVIRD03",
                        b"HXVIQF01",
                        b"HXVIRR01",
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
                    if frames > 256 || out.bytes[8..10] != [0, 0] {
                        return Err(Error::Protocol("unsupported instance PCM extent or events"));
                    }
                    wanted = 64 + frames * 8;
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
        let duration = remaining(until)?;
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
    #[test]
    fn hostile_frames_refuse_before_unbounded_reads_or_allocation() {
        let mut oversized = b"HXVIRD03".to_vec();
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
            hex_audio::plugin_transport::Binding {
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
        channel.reject_queued_after_reap().unwrap();
    }
}
