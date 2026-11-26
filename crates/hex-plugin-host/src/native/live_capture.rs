//! One live main-lane capture, serviced only by the original serialized worker.
//! PCM exchanges keep their existing bounded path; no second socket reader or
//! native owner is created. These calls must never run on the Hex callback.
use super::exchange::{self, ExchangeChannel};
use super::*;

#[derive(Clone, Copy)]
pub(super) struct PendingCapture {
    operation: u64,
    until: Instant,
}

impl Prepared {
    /// Begin one bounded current component/controller capture while processing
    /// continues. The acknowledgement means only that this exact request was
    /// admitted; the caller must poll through this same original worker. A
    /// second begin refuses without disturbing the first operation. An
    /// `Error::CaptureRefused` refusal, here or from a poll, retires only this
    /// operation: the instance keeps processing and a later capture may begin.
    /// The helper refuses a capture as still changing before this deadline
    /// expires, so continuous plugin edits never fault the lease.
    /// Every other failure stays terminal. Startup and
    /// canonical state remain unchanged. Ordinary PCM progress checks the same
    /// capture deadline, so an abandoned operation cannot live indefinitely.
    pub fn begin_state_capture_on_worker(&mut self) -> Result<()> {
        if let Some(control) = self.control.as_mut() {
            let result = control.begin_state_capture_on_control();
            if control.is_faulted() {
                self.faulted = true;
                self.lease.cancel_native();
            }
            return result;
        }
        self.require_live_epoch()?;
        if self.live_capture.is_some() {
            return Err(Error::Protocol("native capture is still pending"));
        }
        let operation = self
            .live_capture_sequence
            .checked_add(1)
            .ok_or(Error::Protocol("live capture operation exhausted"))?;
        let pending = PendingCapture {
            operation,
            until: end_after(self.limits.startup)?,
        };
        let sequence = self.sequence;
        let mut io = ExchangeChannel {
            channel: &mut self.lease.channel,
            packet: &mut self.packet,
            config: &self.config,
            cancellation: &self.lease.cancellation,
            exchange_limit: self.limits.exchange,
        };
        match exchange::live_capture_exchange(
            &mut io,
            sequence,
            pending.operation,
            pending.until,
            false,
        ) {
            Ok(None) => {
                self.live_capture = Some(pending);
                self.live_capture_sequence = operation;
                Ok(())
            }
            Ok(Some(_)) => {
                self.refuse_live_capture(Error::Protocol("live capture completed before admission"))
            }
            Err(Error::CaptureRefused(refusal)) => {
                self.live_capture_sequence = operation;
                Err(Error::CaptureRefused(refusal))
            }
            Err(cause) => self.refuse_live_capture(cause),
        }
    }

    /// One immediate status exchange, interleaved with ordinary PCM by the
    /// original joining worker. Pending never waits for the SDK call. Some
    /// transfers bounded data only and retires the exact operation; it is not a
    /// save, canonical edit or publication acknowledgement. Failure cancels the
    /// same lease and leaves its disposal with the existing owner.
    pub fn poll_state_capture_on_worker(&mut self) -> Result<Option<NativeDeviceState>> {
        if let Some(control) = self.control.as_mut() {
            let result = control.poll_state_capture_on_control();
            if control.is_faulted() {
                self.faulted = true;
                self.lease.cancel_native();
            }
            return result;
        }
        self.require_live_epoch()?;
        let pending = self
            .live_capture
            .ok_or(Error::Protocol("native capture is not pending"))?;
        let sequence = self.sequence;
        let mut io = ExchangeChannel {
            channel: &mut self.lease.channel,
            packet: &mut self.packet,
            config: &self.config,
            cancellation: &self.lease.cancellation,
            exchange_limit: self.limits.exchange,
        };
        match exchange::live_capture_exchange(
            &mut io,
            sequence,
            pending.operation,
            pending.until,
            true,
        ) {
            Ok(state) => {
                if state.is_some() {
                    self.live_capture = None;
                }
                Ok(state)
            }
            Err(Error::CaptureRefused(refusal)) => {
                self.live_capture = None;
                Err(Error::CaptureRefused(refusal))
            }
            Err(cause) => self.refuse_live_capture(cause),
        }
    }

    pub(super) fn check_live_capture_deadline(&self) -> Result<()> {
        if let Some(control) = self.control.as_ref() {
            control.check_deadlines()?;
        }
        if self
            .live_capture
            .is_some_and(|pending| Instant::now() >= pending.until)
        {
            return Err(Error::Timeout);
        }
        Ok(())
    }

    fn refuse_live_capture<T>(&mut self, cause: Error) -> Result<T> {
        self.faulted = true;
        self.lease.cancel_native();
        Err(cause)
    }
}
