//! Narrow audited POSIX shared-memory mapping and `SCM_RIGHTS` FD transfer.
//!
//! Safety invariants:
//! - Shared memory is NEVER exposed as `&[u8]` or `&mut [u8]` because a peer
//!   process can mutate mapped pages concurrently.
//! - All reads and writes to the `MAP_SHARED` region go exclusively through
//!   atomic loads/stores (`AtomicU8`, `AtomicU32`, `AtomicU64`).
//! - POSIX shared-memory objects are unlinked immediately after `shm_open` so
//!   no stale `/dev/shm` or macOS Mach shm names survive even `SIGKILL`.
#![allow(unsafe_code)]

use std::{
    ffi::CString,
    io,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    ptr::NonNull,
    sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering},
};

pub(crate) const MAGIC: [u8; 8] = *b"HXSHM001";
pub(crate) const VERSION: u32 = 1;
pub(crate) const SLOT_COUNT: usize = 4;
pub(crate) const SLOT_CAPACITY: usize = 16_384;
pub(crate) const HEADER_BYTES: usize = 4_096;
pub(crate) const REQ_SLOTS_OFFSET: usize = HEADER_BYTES;
pub(crate) const REP_SLOTS_OFFSET: usize = HEADER_BYTES + SLOT_COUNT * SLOT_CAPACITY;
pub(crate) const REGION_BYTES: usize = HEADER_BYTES + 2 * SLOT_COUNT * SLOT_CAPACITY;

// Header field byte offsets (must match crates/hex-plugin-scan/native/macos_instance_shm.hpp).
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_SLOT_COUNT: usize = 12;
const OFF_SLOT_CAPACITY: usize = 16;
const OFF_RESERVED: usize = 20;

// Cache line 1 (64..128): Host -> Worker request ring.
const OFF_REQ_WRITE_SEQ: usize = 64;
const OFF_REQ_READ_SEQ: usize = 72;
const OFF_REQ_SLOT_LEN: usize = 80;

// Cache line 2 (128..192): Worker -> Host reply ring.
const OFF_REP_WRITE_SEQ: usize = 128;
const OFF_REP_READ_SEQ: usize = 136;
const OFF_REP_SLOT_LEN: usize = 144;

// Cache line 3 (192..256): Shared fault indicator.
const OFF_FAULT_FLAG: usize = 192;

const _: () = {
    assert!(HEADER_BYTES.is_multiple_of(8));
    assert!(SLOT_CAPACITY.is_multiple_of(8));
    assert!(REQ_SLOTS_OFFSET.is_multiple_of(8));
    assert!(REP_SLOTS_OFFSET.is_multiple_of(8));
    assert!(REGION_BYTES == 135_168);
};

static SHM_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) struct RawMapping {
    ptr: NonNull<u8>,
}

// SAFETY: All accesses into the mapping are strictly atomic and bounded within
// `REGION_BYTES`. Single-producer / single-consumer slot exclusivity is enforced
// by `HostRing` and `WorkerRing` taking `&mut self`.
unsafe impl Send for RawMapping {}
unsafe impl Sync for RawMapping {}

impl Drop for RawMapping {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was returned by `mmap` with length `REGION_BYTES`
        // and has not been unmapped yet.
        unsafe {
            let _ = libc::munmap(self.ptr.as_ptr().cast::<libc::c_void>(), REGION_BYTES);
        }
    }
}

impl RawMapping {
    pub(crate) fn create() -> io::Result<(Self, OwnedFd)> {
        let pid = std::process::id();
        let mut fd_raw: RawFd = -1;
        for _ in 0..16 {
            let seq = SHM_COUNTER.fetch_add(1, Ordering::Relaxed);
            // macOS PSHMNAMLEN is 31 bytes (including leading '/').
            // "/hxshm_" (7) + pid (<=10) + "_" (1) + seq hex (<=10) <= 28 bytes.
            let name = format!("/hxshm_{pid}_{seq:x}");
            let Ok(c_name) = CString::new(name) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid shm name",
                ));
            };
            // SAFETY: `c_name` is a valid NUL-terminated string.
            let opened = unsafe {
                libc::shm_open(
                    c_name.as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                    0o600 as libc::c_uint,
                )
            };
            if opened >= 0 {
                // Immediately unlink so no named shm object leaks even on SIGKILL.
                // SAFETY: `c_name` is the exact name just created above.
                unsafe {
                    let _ = libc::shm_unlink(c_name.as_ptr());
                }
                fd_raw = opened;
                break;
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EEXIST) {
                return Err(err);
            }
        }
        if fd_raw < 0 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate unique posix shm name",
            ));
        }
        // SAFETY: `fd_raw` is a fresh open descriptor owned exclusively here.
        let fd = unsafe { OwnedFd::from_raw_fd(fd_raw) };
        set_cloexec(fd.as_raw_fd())?;

        // SAFETY: `fd` is a valid writable POSIX shm descriptor.
        let truncated = unsafe { libc::ftruncate(fd.as_raw_fd(), REGION_BYTES as libc::off_t) };
        if truncated != 0 {
            return Err(io::Error::last_os_error());
        }

        let mapping = mmap_fd(fd.as_raw_fd())?;
        mapping.init_header();
        Ok((mapping, fd))
    }

    pub(crate) fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is a valid open descriptor and `stat` is writable.
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_size < REGION_BYTES as libc::off_t {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared-memory region smaller than required layout",
            ));
        }
        let mapping = mmap_fd(fd.as_raw_fd())?;
        // Close `fd` immediately; POSIX `mmap` keeps the VM object alive until `munmap`.
        drop(fd);
        if !mapping.validate_header() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared-memory header validation failed",
            ));
        }
        Ok(mapping)
    }

    fn init_header(&self) {
        self.write_bytes_relaxed(OFF_MAGIC, &MAGIC);
        self.atomic_u32(OFF_VERSION)
            .store(VERSION, Ordering::Relaxed);
        self.atomic_u32(OFF_SLOT_COUNT)
            .store(SLOT_COUNT as u32, Ordering::Relaxed);
        self.atomic_u32(OFF_SLOT_CAPACITY)
            .store(SLOT_CAPACITY as u32, Ordering::Relaxed);
        self.atomic_u32(OFF_RESERVED).store(0, Ordering::Relaxed);

        self.req_write_seq().store(0, Ordering::Relaxed);
        self.req_read_seq().store(0, Ordering::Relaxed);
        self.rep_write_seq().store(0, Ordering::Relaxed);
        self.rep_read_seq().store(0, Ordering::Relaxed);
        for slot in 0..SLOT_COUNT {
            self.req_slot_len(slot).store(0, Ordering::Relaxed);
            self.rep_slot_len(slot).store(0, Ordering::Relaxed);
        }
        self.fault_flag().store(0, Ordering::Release);
    }

    fn validate_header(&self) -> bool {
        let mut magic = [0_u8; 8];
        self.read_bytes_relaxed(OFF_MAGIC, &mut magic);
        magic == MAGIC
            && self.atomic_u32(OFF_VERSION).load(Ordering::Acquire) == VERSION
            && self.atomic_u32(OFF_SLOT_COUNT).load(Ordering::Acquire) == SLOT_COUNT as u32
            && self.atomic_u32(OFF_SLOT_CAPACITY).load(Ordering::Acquire) == SLOT_CAPACITY as u32
            && self.atomic_u32(OFF_RESERVED).load(Ordering::Acquire) == 0
    }

    #[inline]
    fn atomic_u32(&self, offset: usize) -> &AtomicU32 {
        debug_assert!(offset.is_multiple_of(4) && offset + 4 <= REGION_BYTES);
        // SAFETY: `self.ptr` is page-aligned (4096+) and `offset` is 4-byte aligned
        // within `REGION_BYTES`.
        unsafe { &*(self.ptr.as_ptr().add(offset).cast::<AtomicU32>()) }
    }

    #[inline]
    fn atomic_u64(&self, offset: usize) -> &AtomicU64 {
        debug_assert!(offset.is_multiple_of(8) && offset + 8 <= REGION_BYTES);
        // SAFETY: `self.ptr` is page-aligned (4096+) and `offset` is 8-byte aligned
        // within `REGION_BYTES`.
        unsafe { &*(self.ptr.as_ptr().add(offset).cast::<AtomicU64>()) }
    }

    #[inline]
    fn atomic_u8(&self, offset: usize) -> &AtomicU8 {
        debug_assert!(offset < REGION_BYTES);
        // SAFETY: `offset < REGION_BYTES` within the live mapping.
        unsafe { &*(self.ptr.as_ptr().add(offset).cast::<AtomicU8>()) }
    }

    #[inline]
    pub(crate) fn req_write_seq(&self) -> &AtomicU64 {
        self.atomic_u64(OFF_REQ_WRITE_SEQ)
    }

    #[inline]
    pub(crate) fn req_read_seq(&self) -> &AtomicU64 {
        self.atomic_u64(OFF_REQ_READ_SEQ)
    }

    #[inline]
    pub(crate) fn req_slot_len(&self, slot: usize) -> &AtomicU32 {
        debug_assert!(slot < SLOT_COUNT);
        self.atomic_u32(OFF_REQ_SLOT_LEN + slot * 4)
    }

    #[inline]
    pub(crate) fn rep_write_seq(&self) -> &AtomicU64 {
        self.atomic_u64(OFF_REP_WRITE_SEQ)
    }

    #[inline]
    pub(crate) fn rep_read_seq(&self) -> &AtomicU64 {
        self.atomic_u64(OFF_REP_READ_SEQ)
    }

    #[inline]
    pub(crate) fn rep_slot_len(&self, slot: usize) -> &AtomicU32 {
        debug_assert!(slot < SLOT_COUNT);
        self.atomic_u32(OFF_REP_SLOT_LEN + slot * 4)
    }

    #[inline]
    pub(crate) fn fault_flag(&self) -> &AtomicU32 {
        self.atomic_u32(OFF_FAULT_FLAG)
    }

    /// Copy `src` into shared memory at 8-byte-aligned `base_offset` using only
    /// relaxed atomic stores (64-bit chunks + 8-bit tail). Caller must publish
    /// with a `Release` store on the ring sequence counter afterward.
    pub(crate) fn write_bytes_relaxed(&self, base_offset: usize, src: &[u8]) {
        debug_assert!(base_offset.is_multiple_of(8));
        debug_assert!(base_offset + src.len() <= REGION_BYTES);
        let mut at = 0;
        let chunks = src.chunks_exact(8);
        let remainder = chunks.remainder();
        for chunk in chunks {
            let word = u64::from_ne_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]);
            self.atomic_u64(base_offset + at)
                .store(word, Ordering::Relaxed);
            at += 8;
        }
        for &byte in remainder {
            self.atomic_u8(base_offset + at)
                .store(byte, Ordering::Relaxed);
            at += 1;
        }
    }

    /// Copy from shared memory at 8-byte-aligned `base_offset` into `dst` using
    /// only relaxed atomic loads (64-bit chunks + 8-bit tail). Caller must have
    /// synchronized with an `Acquire` load on the ring sequence counter first.
    pub(crate) fn read_bytes_relaxed(&self, base_offset: usize, dst: &mut [u8]) {
        debug_assert!(base_offset.is_multiple_of(8));
        debug_assert!(base_offset + dst.len() <= REGION_BYTES);
        let mut at = 0;
        let mut chunks = dst.chunks_exact_mut(8);
        for chunk in &mut chunks {
            let word = self
                .atomic_u64(base_offset + at)
                .load(Ordering::Relaxed)
                .to_ne_bytes();
            chunk.copy_from_slice(&word);
            at += 8;
        }
        for byte in chunks.into_remainder() {
            *byte = self.atomic_u8(base_offset + at).load(Ordering::Relaxed);
            at += 1;
        }
    }
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is a valid open file descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn mmap_fd(fd: RawFd) -> io::Result<RawMapping> {
    // SAFETY: `fd` is valid and sized to at least `REGION_BYTES`.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            REGION_BYTES,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if mapped == libc::MAP_FAILED || mapped.is_null() {
        return Err(io::Error::last_os_error());
    }
    let Some(ptr) = NonNull::new(mapped.cast::<u8>()) else {
        return Err(io::Error::other("mmap returned null pointer"));
    };
    Ok(RawMapping { ptr })
}

const SCM_HANDSHAKE_BYTE: u8 = b'S';

pub(crate) fn send_shm_fd(stream: &UnixStream, fd: BorrowedFd<'_>) -> io::Result<()> {
    let mut dummy = [SCM_HANDSHAKE_BYTE];
    let mut iov = libc::iovec {
        iov_base: dummy.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: dummy.len(),
    };

    // Align ancillary buffer to `cmsghdr`.
    let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cmsg_buf = vec![0_u64; cmsg_space.div_ceil(std::mem::size_of::<u64>())];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
    msg.msg_controllen = cmsg_space as _;

    // SAFETY: `msg` points to properly aligned and sized `cmsg_buf`.
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::other("CMSG_FIRSTHDR returned null"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        let raw_fd = fd.as_raw_fd();
        std::ptr::copy_nonoverlapping(
            (&raw_fd as *const RawFd).cast::<u8>(),
            libc::CMSG_DATA(cmsg),
            std::mem::size_of::<RawFd>(),
        );
        msg.msg_controllen = (*cmsg).cmsg_len;
    }

    // SAFETY: `stream` is a live Unix domain socket and `msg` is properly initialized.
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, 0) };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    if sent != 1 {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "short sendmsg on shm fd transfer",
        ));
    }
    Ok(())
}

pub(crate) fn recv_shm_fd(stream: &UnixStream) -> io::Result<OwnedFd> {
    let mut dummy = [0_u8; 1];
    let mut iov = libc::iovec {
        iov_base: dummy.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: dummy.len(),
    };

    let cmsg_space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cmsg_buf = vec![0_u64; cmsg_space.div_ceil(std::mem::size_of::<u64>())];

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
    msg.msg_controllen = cmsg_space as _;

    // SAFETY: `stream` is a live Unix domain socket and `msg` buffers are valid.
    let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if received == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "socket closed before shm fd arrived",
        ));
    }

    let mut extracted: Option<OwnedFd> = None;
    let mut valid_cmsg = false;

    // SAFETY: Walk ancillary headers returned by kernel within `msg`.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data_len = ((*cmsg).cmsg_len as usize)
                    .saturating_sub((libc::CMSG_DATA(cmsg) as usize) - (cmsg as usize));
                let fd_count = data_len / std::mem::size_of::<RawFd>();
                for index in 0..fd_count {
                    let mut raw_fd: RawFd = -1;
                    std::ptr::copy_nonoverlapping(
                        libc::CMSG_DATA(cmsg).add(index * std::mem::size_of::<RawFd>()),
                        (&mut raw_fd as *mut RawFd).cast::<u8>(),
                        std::mem::size_of::<RawFd>(),
                    );
                    if raw_fd >= 0 {
                        let owned = OwnedFd::from_raw_fd(raw_fd);
                        if extracted.is_none() && fd_count == 1 {
                            extracted = Some(owned);
                            valid_cmsg = true;
                        } else {
                            // Refuse extra descriptors and close them via Drop.
                            extracted = None;
                            valid_cmsg = false;
                        }
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }

    if (msg.msg_flags & libc::MSG_CTRUNC) != 0 || dummy[0] != SCM_HANDSHAKE_BYTE || !valid_cmsg {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "malformed shm fd control message",
        ));
    }

    let fd = extracted.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing shm fd in control message",
        )
    })?;
    set_cloexec(fd.as_raw_fd())?;
    Ok(fd)
}
