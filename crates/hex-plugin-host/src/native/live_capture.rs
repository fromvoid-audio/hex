//! One live main-lane capture, serviced only by the original serialized worker.
//! PCM exchanges keep their existing bounded path; no second socket reader or
//! native owner is created. These calls must never run on the Hex callback.
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
        let result = self.live_capture_exchange(pending, false);
        match result {
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
        self.require_live_epoch()?;
        let pending = self
            .live_capture
            .ok_or(Error::Protocol("native capture is not pending"))?;
        match self.live_capture_exchange(pending, true) {
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

    fn live_capture_exchange(
        &mut self,
        pending: PendingCapture,
        poll: bool,
    ) -> Result<Option<NativeDeviceState>> {
        let request =
            protocol::live_capture_request(&self.config, self.sequence, pending.operation, poll)?;
        let until = end_after(self.limits.exchange)?.min(pending.until);
        let token = self.lease.cancellation.clone();
        self.lease
            .channel
            .send(&request, until, &mut || token.check())?;
        self.lease
            .channel
            .read(&mut self.packet, until, &mut || token.check())?;
        let reply = protocol::live_capture_reply(
            self.packet.magic(),
            self.packet.body()?,
            &self.config,
            self.sequence,
            pending.operation,
        );
        // Cancellation wins over a decoded reply or refusal; a worker error or
        // malformed frame stays the terminal cause.
        if matches!(reply, Ok(_) | Err(Error::CaptureRefused(_))) {
            token.check()?;
        }
        reply
    }
}
