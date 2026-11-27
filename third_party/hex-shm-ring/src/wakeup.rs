//! Wait-free real-time worker wakeup primitive isolating narrow OS FFI.
//!
//! Construct and drop on the control thread. [`RtWakeup::signal`] performs a
//! single non-blocking OS wakeup with zero heap allocation and zero user-space
//! locks, avoiding `std::thread::Thread::unpark()` parker mutex and priority
//! inversion hazards on CoreAudio real-time threads.

use std::io;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_int;
    use std::io;
    use std::time::{Duration, Instant};

    type KernReturn = c_int;
    type Task = u32;
    type Semaphore = u32;
    type SyncPolicy = c_int;

    const KERN_SUCCESS: KernReturn = 0;
    const KERN_ABORTED: KernReturn = 14;
    const KERN_OPERATION_TIMED_OUT: KernReturn = 49;
    const SYNC_POLICY_FIFO: SyncPolicy = 0;
    const MACH_PORT_NULL: Semaphore = 0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MachTimespec {
        tv_sec: u32,
        tv_nsec: c_int,
    }

    unsafe extern "C" {
        static mach_task_self_: Task;
        fn semaphore_create(
            task: Task,
            semaphore: *mut Semaphore,
            policy: SyncPolicy,
            value: c_int,
        ) -> KernReturn;
        fn semaphore_signal(semaphore: Semaphore) -> KernReturn;
        fn semaphore_timedwait(semaphore: Semaphore, wait_time: MachTimespec) -> KernReturn;
        fn semaphore_destroy(task: Task, semaphore: Semaphore) -> KernReturn;
    }

    pub struct RtWakeup {
        sem: Semaphore,
    }

    // SAFETY: Mach semaphore port names are process-wide kernel objects safe to
    // signal and wait on concurrently from any thread in the owning task.
    unsafe impl Send for RtWakeup {}
    unsafe impl Sync for RtWakeup {}

    impl RtWakeup {
        pub fn new() -> io::Result<Self> {
            let mut sem: Semaphore = MACH_PORT_NULL;
            // SAFETY: `mach_task_self_` identifies the current task and `sem` is valid out-storage.
            let status =
                unsafe { semaphore_create(mach_task_self_, &mut sem, SYNC_POLICY_FIFO, 0) };
            if status != KERN_SUCCESS || sem == MACH_PORT_NULL {
                return Err(io::Error::other(format!(
                    "mach semaphore_create failed (kern_return={status})"
                )));
            }
            Ok(Self { sem })
        }

        #[inline]
        pub fn signal(&self) {
            // SAFETY: `self.sem` is a live Mach kernel semaphore owned until `Drop`.
            let _ = unsafe { semaphore_signal(self.sem) };
        }

        pub fn wait_timeout(&self, timeout: Duration) -> bool {
            let deadline = Instant::now().checked_add(timeout);
            loop {
                let remaining = match deadline {
                    Some(until) => until.saturating_duration_since(Instant::now()),
                    None => timeout,
                };
                let secs = remaining.as_secs().min(u32::MAX as u64) as u32;
                let nanos = remaining.subsec_nanos() as c_int;
                let ts = MachTimespec {
                    tv_sec: secs,
                    tv_nsec: nanos,
                };
                // SAFETY: `self.sem` is a live Mach kernel semaphore owned until `Drop`.
                let status = unsafe { semaphore_timedwait(self.sem, ts) };
                if status == KERN_SUCCESS {
                    return true;
                }
                if status == KERN_OPERATION_TIMED_OUT {
                    return false;
                }
                if status == KERN_ABORTED {
                    if let Some(until) = deadline
                        && Instant::now() >= until
                    {
                        return false;
                    }
                    continue;
                }
                return false;
            }
        }
    }

    impl Drop for RtWakeup {
        fn drop(&mut self) {
            // SAFETY: `self.sem` was created by `semaphore_create` for `mach_task_self_`.
            let _ = unsafe { semaphore_destroy(mach_task_self_, self.sem) };
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::time::{Duration, Instant};

    pub struct RtWakeup {
        read_fd: OwnedFd,
        write_fd: OwnedFd,
    }

    fn set_nonblock_cloexec(fd: RawFd) -> io::Result<()> {
        // SAFETY: `fd` is a valid freshly-created pipe descriptor.
        let fd_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if fd_flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFD, fd_flags | libc::FD_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if status_flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFL, status_flags | libc::O_NONBLOCK) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    impl RtWakeup {
        pub fn new() -> io::Result<Self> {
            let mut fds = [-1_i32; 2];
            // SAFETY: `fds` is a valid 2-element output array for `pipe`.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `pipe` succeeded and returned two freshly opened unique fds.
            let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
            let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
            set_nonblock_cloexec(read_fd.as_raw_fd())?;
            set_nonblock_cloexec(write_fd.as_raw_fd())?;
            Ok(Self { read_fd, write_fd })
        }

        #[inline]
        pub fn signal(&self) {
            let byte: u8 = 1;
            // SAFETY: Non-blocking 1-byte write into live pipe descriptor; EAGAIN
            // simply means a wakeup token is already queued.
            let _ = unsafe {
                libc::write(
                    self.write_fd.as_raw_fd(),
                    (&byte as *const u8).cast::<libc::c_void>(),
                    1,
                )
            };
        }

        pub fn wait_timeout(&self, timeout: Duration) -> bool {
            let mut drain = [0_u8; 64];
            let mut read_once = || {
                // SAFETY: `read_fd` is a valid non-blocking pipe descriptor.
                unsafe {
                    libc::read(
                        self.read_fd.as_raw_fd(),
                        drain.as_mut_ptr().cast::<libc::c_void>(),
                        drain.len(),
                    )
                }
            };
            if read_once() > 0 {
                return true;
            }
            let deadline = Instant::now().checked_add(timeout);
            loop {
                let remaining = match deadline {
                    Some(until) => until.saturating_duration_since(Instant::now()),
                    None => timeout,
                };
                let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
                let mut pfd = libc::pollfd {
                    fd: self.read_fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: `pfd` points to one valid `pollfd`.
                let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
                if rc > 0 {
                    return read_once() > 0;
                }
                if rc == 0 {
                    return false;
                }
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    if let Some(until) = deadline
                        && Instant::now() >= until
                    {
                        return false;
                    }
                    continue;
                }
                return false;
            }
        }
    }
}

/// Real-time-safe cross-thread wakeup signal (Mach kernel semaphore on macOS,
/// non-blocking pipe on other Unix targets).
pub struct RtWakeup {
    inner: imp::RtWakeup,
}

impl RtWakeup {
    /// Allocate a new wakeup handle on the control thread.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            inner: imp::RtWakeup::new()?,
        })
    }

    /// Wake a parked worker without heap allocation or user-space mutexes.
    #[inline]
    pub fn signal(&self) {
        self.inner.signal();
    }

    /// Block the worker thread until signaled or `timeout` elapses.
    /// Returns `true` when woken by a signal, `false` on timeout.
    #[inline]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        self.inner.wait_timeout(timeout)
    }
}
