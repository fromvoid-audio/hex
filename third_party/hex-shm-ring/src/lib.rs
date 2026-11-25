//! Safe bounded shared-memory SPSC audio-quantum ring buffer for `hex-plugin-host`.
//!
//! All `unsafe` POSIX `shm_open`/`mmap`/`sendmsg`/`recvmsg` FFI is isolated inside
//! `mod raw`; this top-level module forbids `unsafe_code` and exposes strictly
//! bounded, atomic-slot copy APIs so no `&[u8]`/`&mut [u8]` slice ever aliases
//! cross-process shared memory.
#![deny(unsafe_code)]

mod raw;

use raw::{REP_SLOTS_OFFSET, REQ_SLOTS_OFFSET, RawMapping};
use std::{
    fmt, io,
    os::{
        fd::{BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::atomic::Ordering,
};

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
pub struct HostRing {
    mapping: RawMapping,
}

impl HostRing {
    /// Allocate and map an unlinked anonymous POSIX shared-memory region.
    /// Transfer the returned `OwnedFd` once via [`send_shm_fd`], then drop it:
    /// the mapping remains valid until both sides unmap.
    pub fn create() -> io::Result<(Self, OwnedFd)> {
        let (mapping, fd) = RawMapping::create()?;
        Ok((Self { mapping }, fd))
    }

    /// Publish one bounded request packet into the next free request slot.
    pub fn write_request(&mut self, payload: &[u8]) -> Result<(), ShmError> {
        if payload.is_empty() || payload.len() > SLOT_CAPACITY {
            return Err(ShmError::PayloadTooLarge);
        }
        let write_seq = self.mapping.req_write_seq().load(Ordering::Relaxed);
        let read_seq = self.mapping.req_read_seq().load(Ordering::Acquire);
        if write_seq < read_seq || write_seq - read_seq >= SLOT_COUNT as u64 {
            return Err(if write_seq < read_seq {
                ShmError::CorruptSequence
            } else {
                ShmError::RingFull
            });
        }
        let slot = (write_seq % SLOT_COUNT as u64) as usize;
        let offset = REQ_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.write_bytes_relaxed(offset, payload);
        self.mapping
            .req_slot_len(slot)
            .store(payload.len() as u32, Ordering::Relaxed);
        self.mapping
            .req_write_seq()
            .store(write_seq + 1, Ordering::Release);
        Ok(())
    }

    /// Consume the next completed reply packet from the reply ring into `out`.
    pub fn read_reply(&mut self, out: &mut [u8]) -> Result<usize, ShmError> {
        let read_seq = self.mapping.rep_read_seq().load(Ordering::Relaxed);
        let write_seq = self.mapping.rep_write_seq().load(Ordering::Acquire);
        if write_seq < read_seq || write_seq - read_seq > SLOT_COUNT as u64 {
            return Err(ShmError::CorruptSequence);
        }
        if read_seq == write_seq {
            return Err(ShmError::RingEmpty);
        }
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
        self.mapping
            .rep_read_seq()
            .store(read_seq + 1, Ordering::Release);
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
pub struct WorkerRing {
    mapping: RawMapping,
}

impl WorkerRing {
    /// Attach to a host-created shared-memory descriptor received via [`recv_shm_fd`].
    /// The descriptor is closed immediately after `mmap`.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        let mapping = RawMapping::from_fd(fd)?;
        Ok(Self { mapping })
    }

    /// Consume the next pending request packet into `out`.
    pub fn read_request(&mut self, out: &mut [u8]) -> Result<usize, ShmError> {
        let read_seq = self.mapping.req_read_seq().load(Ordering::Relaxed);
        let write_seq = self.mapping.req_write_seq().load(Ordering::Acquire);
        if write_seq < read_seq || write_seq - read_seq > SLOT_COUNT as u64 {
            return Err(ShmError::CorruptSequence);
        }
        if read_seq == write_seq {
            return Err(ShmError::RingEmpty);
        }
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
        self.mapping
            .req_read_seq()
            .store(read_seq + 1, Ordering::Release);
        Ok(len)
    }

    /// Publish one bounded reply packet into the next free reply slot.
    pub fn write_reply(&mut self, payload: &[u8]) -> Result<(), ShmError> {
        if payload.is_empty() || payload.len() > SLOT_CAPACITY {
            return Err(ShmError::PayloadTooLarge);
        }
        let write_seq = self.mapping.rep_write_seq().load(Ordering::Relaxed);
        let read_seq = self.mapping.rep_read_seq().load(Ordering::Acquire);
        if write_seq < read_seq || write_seq - read_seq >= SLOT_COUNT as u64 {
            return Err(if write_seq < read_seq {
                ShmError::CorruptSequence
            } else {
                ShmError::RingFull
            });
        }
        let slot = (write_seq % SLOT_COUNT as u64) as usize;
        let offset = REP_SLOTS_OFFSET + slot * SLOT_CAPACITY;
        self.mapping.write_bytes_relaxed(offset, payload);
        self.mapping
            .rep_slot_len(slot)
            .store(payload.len() as u32, Ordering::Relaxed);
        self.mapping
            .rep_write_seq()
            .store(write_seq + 1, Ordering::Release);
        Ok(())
    }

    /// Test-only helper for injecting a hostile slot length to verify host bounds refusal.
    #[cfg(test)]
    fn corrupt_reply_len(&mut self, slot: usize, len: u32) {
        self.mapping
            .rep_slot_len(slot)
            .store(len, Ordering::Release);
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
}
