//! Retained construction or epoch refusal. Root may observe cleanup without disposing
//! the actual lease on an error-return path or inferring completion from Drop.
use super::*;

pub(super) enum Custody {
    NoInstance,
    Probe(ChildCustody),
    Native(Lease),
}

/// Physical native refusal cleanup only. Neither result admits a native source,
/// acknowledges a graph, or establishes callback/Root return.
#[derive(Clone, Copy, Debug)]
pub enum PreparationDisposal {
    /// Refused before any native instance child was started. The waitability
    /// probe, if created, was actually reaped before returning this outcome.
    NoInstance,
    Reaped(Retirement),
}

/// Unique failed-preparation/epoch custody. Keep this on the existing control or
/// lifecycle worker until `poll_disposal_on_control` observes completion. An
/// error/timeout leaves custody retained; it never permits reuse. Drop remains
/// a joining control-only fallback, not an observable disposal receipt.
pub struct PreparationFailure {
    cause: Error,
    pub(super) custody: Custody,
    deadline: Option<Instant>,
}
impl std::fmt::Debug for PreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparationFailure")
            .field("cause", &self.cause)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for PreparationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.cause.fmt(f)
    }
}
impl std::error::Error for PreparationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
impl PreparationFailure {
    // Deliberately private: arbitrary errors cannot manufacture a no-instance
    // construction result. Only the pre-spawn path may use this constructor.
    pub(super) fn before_native(cause: Error) -> Self {
        Self {
            cause,
            custody: Custody::NoInstance,
            deadline: None,
        }
    }
    pub(super) fn native(cause: Error, lease: Lease, retirement: Duration) -> Self {
        lease.cancel_native();
        Self {
            cause,
            custody: Custody::Native(lease),
            // Overflow must expire immediately rather than disable the bound.
            deadline: Some(
                Instant::now()
                    .checked_add(retirement)
                    .unwrap_or_else(Instant::now),
            ),
        }
    }
    pub(super) fn probe(cause: Error, child: ChildCustody) -> Self {
        Self {
            cause,
            custody: Custody::Probe(child),
            deadline: None,
        }
    }
    pub fn cause(&self) -> &Error {
        &self.cause
    }
    /// Move the original diagnostic into enclosing control custody without
    /// formatting or copying its backing. The failure retains all native and
    /// endpoint owners; taking a cause proves no disposal or callback return.
    /// A repeated take returns only the static transferred-cause marker.
    pub fn take_cause_on_control(&mut self) -> Error {
        std::mem::replace(
            &mut self.cause,
            Error::Protocol("native preparation cause transferred"),
        )
    }
    /// One nonblocking child observation. Cancellation has already targeted
    /// this exact lease. Timeout preserves it for subsequent reap observation;
    /// lost OS custody is sticky and never retries a numeric process wait.
    pub fn poll_disposal_on_control(&mut self) -> Result<Option<PreparationDisposal>> {
        let child = match &mut self.custody {
            Custody::NoInstance => return Ok(Some(PreparationDisposal::NoInstance)),
            Custody::Probe(child) => child,
            Custody::Native(lease) => &mut lease.child,
        };
        if let Some(status) = child.observe()? {
            return Ok(Some(PreparationDisposal::Reaped(Retirement {
                status,
                terminal: None,
            })));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(Error::Timeout);
        }
        Ok(None)
    }
    /// Synchronous convenience callers retain their existing joined-error
    /// behavior. Root uses the retained result and polling method instead.
    pub(super) fn finish_on_control(mut self) -> Error {
        let child = match &mut self.custody {
            Custody::NoInstance | Custody::Probe(_) => return self.cause,
            Custody::Native(lease) => &mut lease.child,
        };
        let (exit, custody_error) = match child.reap() {
            Ok(status) => (Some(status), None),
            Err(error) => (None, Some(Box::new(error))),
        };
        Error::Preparation {
            cause: Box::new(self.cause),
            exit,
            custody_error,
        }
    }
}
