//! Safe bounded shared-memory SPSC audio-quantum ring buffer for `hex-plugin-host`.
//!
//! All `unsafe` POSIX `shm_open`/`mmap`/`sendmsg`/`recvmsg` FFI is isolated inside
//! `mod raw`; this top-level module forbids `unsafe_code` and exposes strictly
//! bounded, atomic-slot copy APIs so no `&[u8]`/`&mut [u8]` slice ever aliases
//! cross-process shared memory.
#![deny(unsafe_code)]

mod raw;
#[allow(unsafe_code)]
mod wakeup;

use raw::{REP_SLOTS_OFFSET, REQ_SLOTS_OFFSET, RawMapping};
use std::{
    fmt, io,
    os::{
        fd::{BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::atomic::Ordering,
};
pub use wakeup::RtWakeup;

/// Apple Silicon L1/L2 cache line granularity (128 bytes).
pub const CACHE_LINE_BYTES: usize = raw::CACHE_LINE_BYTES;
pub const OFF_REQ_WRITE_SEQ: usize = raw::OFF_REQ_WRITE_SEQ;
pub const OFF_REQ_SLOT_LEN: usize = raw::OFF_REQ_SLOT_LEN;
pub const OFF_REQ_READ_SEQ: usize = raw::OFF_REQ_READ_SEQ;
pub const OFF_REP_WRITE_SEQ: usize = raw::OFF_REP_WRITE_SEQ;
pub const OFF_REP_SLOT_LEN: usize = raw::OFF_REP_SLOT_LEN;
pub const OFF_REP_READ_SEQ: usize = raw::OFF_REP_READ_SEQ;
pub const OFF_FAULT_FLAG: usize = raw::OFF_FAULT_FLAG;

/// Single-byte doorbell marker sent on the Unix domain socket after publishing
/// a packet in shared memory. Disjoint from all ASCII `'H'` (`0x48`) control headers.
pub const DOORBELL_BYTE: u8 = 0x01;

/// Number of bounded slots per direction (request ring and reply ring).
pub const SLOT_COUNT: usize = raw::SLOT_COUNT;

/// Maximum byte payload stored in a single ring slot.
pub const SLOT_CAPACITY: usize = raw::SLOT_CAPACITY;

/// Total byte size of the shared-memory mapping (`33 * 4096` bytes).
pub const REGION_BYTES: usize = raw::REGION_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShmError {
    RingFull,
    RingEmpty,
    PayloadTooLarge,
    BufferTooSmall,
    InvalidLength,
    CorruptSequence,
}

impl fmt::Display for ShmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RingFull => f.write_str("shared-memory ring is full"),
            Self::RingEmpty => f.write_str("shared-memory ring is empty"),
            Self::PayloadTooLarge => f.write_str("payload exceeds shared-memory slot capacity"),
            Self::BufferTooSmall => f.write_str("destination buffer smaller than ring payload"),
            Self::InvalidLength => f.write_str("shared-memory slot length out of bounds"),
            Self::CorruptSequence => f.write_str("shared-memory sequence counter out of bounds"),
        }
    }
}

impl std::error::Error for ShmError {}

/// Host-side endpoint of a paired shared-memory audio transport ring.
/// Writes requests into the request ring and reads replies from the reply ring.
///
/// Producer/consumer sequence counters owned by the host (`req_write_seq`,
/// `rep_read_seq`) are kept in process-local fields and only published to
/// `MAP_SHARED` memory, never read back from peer-writable shared memory.
pub struct HostRing {
    mapping: RawMapping,
    req_write_seq: u64,
    rep_read_seq: u64,
}

impl HostRing {
    /// Allocate and map an unlinked anonymous POSIX shared-memory region.
    /// Transfer the returned `OwnedFd` once via [`send_shm_fd`], then drop it:
    /// the mapping remains valid until both sides unmap.
    pub fn create() -> io::Result<(Self, OwnedFd)> {
        let (mapping, fd) = RawMapping::create()?;
        Ok((
            Self {
                mapping,
                req_write_seq: 0,
                rep_read_seq: 0,
            },
            fd,
        ))
    }

    /// Publish one bounded request packet into the next free request slot.
    pub fn write_request(&mut self, payload: &[u8]) -> Result<(), ShmError> {
        if payload.is_empty() || payload.len() > SLOT_CAPACITY {
            return Err(ShmError::PayloadTooLarge);
        }
        let write_seq = self.req_write_seq;
        let read_seq = self.mapping.req_read_seq().load(Ordering::Acquire);
        let pending = write_seq
            .checked_sub(read_seq)
            .ok_or(ShmError::CorruptSequence)?;
        if pending >= SLOT_COUNT as u64 {
            return Err(ShmError::RingFull);
        }
        let next_seq = write_seq.checked_add(1).ok_or(ShmError::CorruptSequence)?;
        let slot = (write_seq % SLOT_COUNT as u64) as usize;
        let offset = REQ_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.write_bytes_relaxed(offset, payload);
        self.mapping
            .req_slot_len(slot)
            .store(payload.len() as u32, Ordering::Relaxed);
        self.req_write_seq = next_seq;
        self.mapping
            .req_write_seq()
            .store(next_seq, Ordering::Release);
        Ok(())
    }

    /// Consume the next completed reply packet from the reply ring into `out`.
    pub fn read_reply(&mut self, out: &mut [u8]) -> Result<usize, ShmError> {
        let read_seq = self.rep_read_seq;
        let write_seq = self.mapping.rep_write_seq().load(Ordering::Acquire);
        let available = write_seq
            .checked_sub(read_seq)
            .ok_or(ShmError::CorruptSequence)?;
        if available > SLOT_COUNT as u64 {
            return Err(ShmError::CorruptSequence);
        }
        if available == 0 {
            return Err(ShmError::RingEmpty);
        }
        let next_seq = read_seq.checked_add(1).ok_or(ShmError::CorruptSequence)?;
        let slot = (read_seq % SLOT_COUNT as u64) as usize;
        let len = self.mapping.rep_slot_len(slot).load(Ordering::Relaxed) as usize;
        if len == 0 || len > SLOT_CAPACITY {
            return Err(ShmError::InvalidLength);
        }
        if len > out.len() {
            return Err(ShmError::BufferTooSmall);
        }
        let offset = REP_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.read_bytes_relaxed(offset, &mut out[..len]);
        self.rep_read_seq = next_seq;
        self.mapping
            .rep_read_seq()
            .store(next_seq, Ordering::Release);
        Ok(len)
    }

    /// Mark the shared fault flag so a peer can observe a host-side abort.
    pub fn set_fault(&self) {
        self.mapping.fault_flag().store(1, Ordering::Release);
    }

    pub fn is_faulted(&self) -> bool {
        self.mapping.fault_flag().load(Ordering::Acquire) != 0
    }
}

/// Worker-side endpoint of a paired shared-memory audio transport ring.
/// Reads requests from the request ring and writes replies into the reply ring.
///
/// Worker-owned sequence counters (`req_read_seq`, `rep_write_seq`) are kept in
/// process-local fields and never loaded back from peer-writable shared memory.
pub struct WorkerRing {
    mapping: RawMapping,
    req_read_seq: u64,
    rep_write_seq: u64,
}

impl WorkerRing {
    /// Attach to a host-created shared-memory descriptor received via [`recv_shm_fd`].
    /// The descriptor is closed immediately after `mmap`.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        let mapping = RawMapping::from_fd(fd)?;
        Ok(Self {
            mapping,
            req_read_seq: 0,
            rep_write_seq: 0,
        })
    }

    /// Consume the next pending request packet into `out`.
    pub fn read_request(&mut self, out: &mut [u8]) -> Result<usize, ShmError> {
        let read_seq = self.req_read_seq;
        let write_seq = self.mapping.req_write_seq().load(Ordering::Acquire);
        let available = write_seq
            .checked_sub(read_seq)
            .ok_or(ShmError::CorruptSequence)?;
        if available > SLOT_COUNT as u64 {
            return Err(ShmError::CorruptSequence);
        }
        if available == 0 {
            return Err(ShmError::RingEmpty);
        }
        let next_seq = read_seq.checked_add(1).ok_or(ShmError::CorruptSequence)?;
        let slot = (read_seq % SLOT_COUNT as u64) as usize;
        let len = self.mapping.req_slot_len(slot).load(Ordering::Relaxed) as usize;
        if len == 0 || len > SLOT_CAPACITY {
            return Err(ShmError::InvalidLength);
        }
        if len > out.len() {
            return Err(ShmError::BufferTooSmall);
        }
        let offset = REQ_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.read_bytes_relaxed(offset, &mut out[..len]);
        self.req_read_seq = next_seq;
        self.mapping
            .req_read_seq()
            .store(next_seq, Ordering::Release);
        Ok(len)
    }

    /// Publish one bounded reply packet into the next free reply slot.
    pub fn write_reply(&mut self, payload: &[u8]) -> Result<(), ShmError> {
        if payload.is_empty() || payload.len() > SLOT_CAPACITY {
            return Err(ShmError::PayloadTooLarge);
        }
        let write_seq = self.rep_write_seq;
        let read_seq = self.mapping.rep_read_seq().load(Ordering::Acquire);
        let pending = write_seq
            .checked_sub(read_seq)
            .ok_or(ShmError::CorruptSequence)?;
        if pending >= SLOT_COUNT as u64 {
            return Err(ShmError::RingFull);
        }
        let next_seq = write_seq.checked_add(1).ok_or(ShmError::CorruptSequence)?;
        let slot = (write_seq % SLOT_COUNT as u64) as usize;
        let offset = REP_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.write_bytes_relaxed(offset, payload);
        self.mapping
            .rep_slot_len(slot)
            .store(payload.len() as u32, Ordering::Relaxed);
        self.rep_write_seq = next_seq;
        self.mapping
            .rep_write_seq()
            .store(next_seq, Ordering::Release);
        Ok(())
    }

    /// Test-only helper for injecting a hostile slot length to verify host bounds refusal.
    #[cfg(test)]
    fn corrupt_reply_len(&mut self, slot: usize, len: u32) {
        self.mapping
            .rep_slot_len(slot)
            .store(len, Ordering::Release);
    }

    /// Test-only helper for simulating a hostile peer tampering shared sequence counters.
    #[cfg(test)]
    fn corrupt_shared_sequences(
        &self,
        req_write_seq: Option<u64>,
        req_read_seq: Option<u64>,
        rep_write_seq: Option<u64>,
        rep_read_seq: Option<u64>,
    ) {
        if let Some(v) = req_write_seq {
            self.mapping.req_write_seq().store(v, Ordering::Release);
        }
        if let Some(v) = req_read_seq {
            self.mapping.req_read_seq().store(v, Ordering::Release);
        }
        if let Some(v) = rep_write_seq {
            self.mapping.rep_write_seq().store(v, Ordering::Release);
        }
        if let Some(v) = rep_read_seq {
            self.mapping.rep_read_seq().store(v, Ordering::Release);
        }
    }
}

/// Transfer `fd` across a Unix domain socket via `SCM_RIGHTS`.
pub fn send_shm_fd(stream: &UnixStream, fd: BorrowedFd<'_>) -> io::Result<()> {
    raw::send_shm_fd(stream, fd)
}

/// Receive a shared-memory descriptor sent by [`send_shm_fd`].
pub fn recv_shm_fd(stream: &UnixStream) -> io::Result<OwnedFd> {
    raw::recv_shm_fd(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn scm_rights_round_trip_and_ring_wraparound_preserve_exact_bytes() {
        let (host_sock, worker_sock) = UnixStream::pair().unwrap();
        let (mut host, shm_fd) = HostRing::create().unwrap();
        send_shm_fd(&host_sock, shm_fd.as_fd()).unwrap();
        // Closing host fd after SCM_RIGHTS keeps mapping valid on both sides.
        drop(shm_fd);

        let peer_fd = recv_shm_fd(&worker_sock).unwrap();
        let mut worker = WorkerRing::from_fd(peer_fd).unwrap();

        let mut req_buf = vec![0_u8; SLOT_CAPACITY];
        let mut rep_buf = vec![0_u8; SLOT_CAPACITY];

        // Wrap around SLOT_COUNT (4) multiple times with varied non-8-byte-aligned sizes.
        for iter in 0..32_usize {
            let len = 64 + (iter * 313) % (SLOT_CAPACITY - 64);
            let req: Vec<u8> = (0..len)
                .map(|i| ((i.wrapping_mul(37) ^ iter) & 0xff) as u8)
                .collect();
            host.write_request(&req).unwrap();
            let got = worker.read_request(&mut req_buf).unwrap();
            assert_eq!(got, len);
            assert_eq!(&req_buf[..got], req.as_slice());

            let rep: Vec<u8> = req.iter().map(|b| b ^ 0x5a).collect();
            worker.write_reply(&rep).unwrap();
            let rep_got = host.read_reply(&mut rep_buf).unwrap();
            assert_eq!(rep_got, len);
            assert_eq!(&rep_buf[..rep_got], rep.as_slice());
        }
    }

    #[test]
    fn ring_enforces_capacity_empty_full_and_hostile_length_bounds() {
        let (host_sock, worker_sock) = UnixStream::pair().unwrap();
        let (mut host, shm_fd) = HostRing::create().unwrap();
        send_shm_fd(&host_sock, shm_fd.as_fd()).unwrap();
        drop(shm_fd);
        let mut worker = WorkerRing::from_fd(recv_shm_fd(&worker_sock).unwrap()).unwrap();

        assert_eq!(host.write_request(&[]), Err(ShmError::PayloadTooLarge));
        assert_eq!(
            host.write_request(&vec![0_u8; SLOT_CAPACITY + 1]),
            Err(ShmError::PayloadTooLarge)
        );
        assert_eq!(host.read_reply(&mut [0_u8; 64]), Err(ShmError::RingEmpty));

        // Fill all SLOT_COUNT slots, then confirm RingFull.
        for i in 0..SLOT_COUNT {
            host.write_request(&[i as u8; 16]).unwrap();
        }
        assert_eq!(host.write_request(&[0xff; 16]), Err(ShmError::RingFull));

        let mut buf = [0_u8; 64];
        for i in 0..SLOT_COUNT {
            let n = worker.read_request(&mut buf).unwrap();
            assert_eq!(n, 16);
            assert_eq!(&buf[..16], &[i as u8; 16]);
        }
        assert_eq!(worker.read_request(&mut buf), Err(ShmError::RingEmpty));

        // Hostile peer writes valid reply, then tampers slot length > SLOT_CAPACITY.
        worker.write_reply(&[0xab; 32]).unwrap();
        worker.corrupt_reply_len(0, (SLOT_CAPACITY + 1) as u32);
        assert_eq!(
            host.read_reply(&mut [0_u8; SLOT_CAPACITY]),
            Err(ShmError::InvalidLength)
        );

        // Destination buffer too small refuses cleanly without consuming slot.
        worker.corrupt_reply_len(0, 32);
        assert_eq!(
            host.read_reply(&mut [0_u8; 16]),
            Err(ShmError::BufferTooSmall)
        );
        assert_eq!(host.read_reply(&mut [0_u8; 32]), Ok(32));
    }

    #[test]
    fn hostile_peer_sequence_tampering_to_u64_max_never_panics_or_corrupts_host() {
        let (host_sock, worker_sock) = UnixStream::pair().unwrap();
        let (mut host, shm_fd) = HostRing::create().unwrap();
        send_shm_fd(&host_sock, shm_fd.as_fd()).unwrap();
        drop(shm_fd);
        let mut worker = WorkerRing::from_fd(recv_shm_fd(&worker_sock).unwrap()).unwrap();

        // Hostile worker overwrites host-owned `req_write_seq` and `rep_read_seq` with `u64::MAX`.
        // Because `HostRing` keeps its own counters private, neither overflows nor corrupts slots.
        worker.corrupt_shared_sequences(Some(u64::MAX), None, None, Some(u64::MAX));
        host.write_request(b"safe-req").unwrap();
        assert_eq!(host.mapping.req_write_seq().load(Ordering::Acquire), 1);

        let mut req_buf = [0_u8; 32];
        assert_eq!(worker.read_request(&mut req_buf), Ok(8));
        assert_eq!(&req_buf[..8], b"safe-req");

        // Restore rep_read_seq to 0 so the worker can publish a valid reply, then tamper again.
        worker.corrupt_shared_sequences(None, None, None, Some(0));
        worker.write_reply(b"safe-rep").unwrap();
        worker.corrupt_shared_sequences(None, None, None, Some(u64::MAX));
        let mut rep_buf = [0_u8; 32];
        assert_eq!(host.read_reply(&mut rep_buf), Ok(8));
        assert_eq!(&rep_buf[..8], b"safe-rep");
        assert_eq!(host.mapping.rep_read_seq().load(Ordering::Acquire), 1);

        // Hostile worker writes `u64::MAX` into peer-owned `req_read_seq` and `rep_write_seq`.
        worker.corrupt_shared_sequences(None, Some(u64::MAX), Some(u64::MAX), None);
        assert_eq!(
            host.write_request(b"blocked"),
            Err(ShmError::CorruptSequence)
        );
        assert_eq!(
            host.read_reply(&mut rep_buf),
            Err(ShmError::CorruptSequence)
        );

        // Exhausted local counter refuses cleanly with `CorruptSequence` instead of wrapping.
        host.req_write_seq = u64::MAX;
        worker.corrupt_shared_sequences(None, Some(u64::MAX), None, None);
        assert_eq!(
            host.write_request(b"overflow"),
            Err(ShmError::CorruptSequence)
        );
        host.rep_read_seq = u64::MAX;
        worker.corrupt_shared_sequences(None, None, Some(u64::MAX), None);
        assert_eq!(host.read_reply(&mut rep_buf), Err(ShmError::RingEmpty));
    }

    #[test]
    fn sequence_counters_occupy_distinct_128_byte_cache_lines() {
        let lines = [
            OFF_REQ_WRITE_SEQ / CACHE_LINE_BYTES,
            OFF_REQ_READ_SEQ / CACHE_LINE_BYTES,
            OFF_REP_WRITE_SEQ / CACHE_LINE_BYTES,
            OFF_REP_READ_SEQ / CACHE_LINE_BYTES,
            OFF_FAULT_FLAG / CACHE_LINE_BYTES,
        ];
        assert_eq!(CACHE_LINE_BYTES, 128);
        for (i, &a) in lines.iter().enumerate() {
            assert_ne!(a, 0, "counter must not share line 0 with immutable header");
            for &b in &lines[i + 1..] {
                assert_ne!(a, b, "counters must occupy distinct 128-byte cache lines");
            }
        }
        assert_eq!(OFF_REQ_SLOT_LEN / CACHE_LINE_BYTES, lines[0]);
        assert_eq!(OFF_REP_SLOT_LEN / CACHE_LINE_BYTES, lines[2]);
    }

    #[test]
    fn rt_wakeup_signals_and_times_out_cleanly() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let wakeup = Arc::new(RtWakeup::new().unwrap());
        // Pre-signaled token wakes immediately without sleeping.
        wakeup.signal();
        let started = Instant::now();
        assert!(wakeup.wait_timeout(Duration::from_millis(250)));
        assert!(started.elapsed() < Duration::from_millis(100));

        // Unsignaled wait returns false after timeout.
        assert!(!wakeup.wait_timeout(Duration::from_millis(5)));

        // Cross-thread wakeup wakes parked waiter promptly.
        let waiter = Arc::clone(&wakeup);
        let handle = std::thread::spawn(move || waiter.wait_timeout(Duration::from_secs(2)));
        std::thread::sleep(Duration::from_millis(5));
        let signal_at = Instant::now();
        wakeup.signal();
        assert!(handle.join().unwrap());
        assert!(signal_at.elapsed() < Duration::from_millis(250));
    }
}
