//! Child process custody, waitable lease retirement, and bounded shutdown.
use super::*;

pub(crate) fn duration_to_ms_u32(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

pub(super) struct Lease {
    pub(super) child: ChildCustody,
    pub(super) channel: Channel,
    pub(super) cancellation: Cancellation,
    pub(super) shm: Option<Box<hex_shm_ring::HostRing>>,
    pub(super) retirement_ms: u32,
    #[cfg(test)]
    pub(super) reap_probe: Option<Arc<std::sync::atomic::AtomicI32>>,
}
// The process handle is consumed exactly once. Lost custody is terminal: even
// cleanup/Drop must not retry a numeric wait against a potentially reused PID.
pub(super) enum ChildCustody {
    Owned(Child, Option<deadline::Permit>),
    Reaped(ExitStatus),
    Lost(Arc<std::io::Error>),
}
impl ChildCustody {
    pub(super) fn lose(&mut self, error: std::io::Error) -> Error {
        if let Self::Owned(_, permit) = self
            && let Some(permit) = permit.take()
        {
            // A wait error cannot prove the other scheduling lane has exited.
            // Keep its scalar reservation until the application exits.
            std::mem::forget(permit);
        }
        let source = Arc::new(error);
        *self = Self::Lost(Arc::clone(&source));
        Error::Custody(crate::CustodyError::Lost(source))
    }

    pub(super) fn kill(&mut self) {
        if let Self::Owned(child, _) = self {
            let _ = child.kill();
        }
    }

    pub(super) fn observe(&mut self) -> Result<Option<ExitStatus>> {
        let result = match self {
            Self::Owned(child, _) => child.try_wait(),
            Self::Reaped(status) => return Ok(Some(*status)),
            Self::Lost(error) => {
                return Err(Error::Custody(crate::CustodyError::Lost(Arc::clone(error))));
            }
        };
        match result {
            Ok(Some(status)) => {
                *self = Self::Reaped(status);
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(error) => Err(self.lose(error)),
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn reap(&mut self) -> Result<ExitStatus> {
        let result = match self {
            Self::Owned(child, _) => child.wait(),
            Self::Reaped(status) => return Ok(*status),
            Self::Lost(error) => {
                return Err(Error::Custody(crate::CustodyError::Lost(Arc::clone(error))));
            }
        };
        match result {
            Ok(status) => {
                *self = Self::Reaped(status);
                Ok(status)
            }
            Err(error) => Err(self.lose(error)),
        }
    }
    pub(super) fn reap_bounded(&mut self, limit: Duration) -> Result<ExitStatus> {
        if let Some(status) = self.observe()? {
            return Ok(status);
        }
        let until = end_after(limit)?;
        loop {
            if let Some(status) = self.observe()? {
                return Ok(status);
            }
            if Instant::now() >= until {
                break;
            }
            pause(until);
        }
        // Escalate to SIGKILL once the bounded retirement window expires,
        // then allow a short bounded kernel reap grace instead of blocking forever.
        self.kill();
        let kill_grace = limit.clamp(Duration::from_millis(25), Duration::from_millis(250));
        let kill_until = end_after(kill_grace)?;
        loop {
            if let Some(status) = self.observe()? {
                return Ok(status);
            }
            if Instant::now() >= kill_until {
                return Err(Error::Timeout);
            }
            pause(kill_until);
        }
    }
}
impl Lease {
    pub(super) fn cancel_native(&self) {
        self.cancellation.cancel_on_control();
        if let Some(shm) = &self.shm {
            shm.set_fault();
        }
        self.channel.close();
    }
}
pub(super) fn wait(
    child: &mut ChildCustody,
    until: Instant,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<ExitStatus> {
    loop {
        check()?;
        match child.observe() {
            Ok(Some(s)) => return Ok(s),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= until {
            return Err(Error::Timeout);
        }
        pause(until);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancel_native();
        let reaped = self
            .child
            .reap_bounded(Duration::from_millis(u64::from(self.retirement_ms))); // joining control/worker only; lost custody never retries
        #[cfg(test)]
        if let Some(probe) = &self.reap_probe {
            let code = match &reaped {
                Ok(status) => status.code().unwrap_or(-1),
                Err(_) => -2,
            };
            probe.store(code, Ordering::Release);
        }
        let _ = reaped;
    }
}
