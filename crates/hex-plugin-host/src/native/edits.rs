//! Receipts from the original native Host's bounded callback FIFO. The same
//! serialized reader interleaves these control exchanges with PCM/capture.
use super::*;
impl Prepared {
    /// Consume at most eight ordered receipts. Perform means actual successful
    /// SDK processing/flush; Begin/End need no invented audio samples. Pending
    /// callbacks remain native-owned under backpressure. Receipt sequences
    /// can skip values superseded in a saturated editor queue. Gesture
    /// boundaries remain ordered. Source cancellation or terminal refusal
    /// ends queued operations; no receipt grants canonical edits.
    pub fn poll_parameter_edits_on_worker(&mut self) -> Result<protocol::ParameterEdits> {
        Ok(self.poll_edits_on_worker(false, false)?.edits)
    }

    /// Opt this source into latency restart recovery and read a bounded batch.
    /// The consumer owns stopped reactivation and compensation rebuild. Use
    /// this poll for subsequent observations on the same source. Reading an
    /// ordinal does not cover it or grant permission to resume.
    pub fn poll_recovery_edits_on_worker(&mut self) -> Result<protocol::RecoveryEdits> {
        self.poll_edits_on_worker(true, false)
    }

    /// Apply one batch of queued editor changes on the original SDK worker
    /// using a parameter-only call and preserving the completed PCM prefix.
    pub fn service_idle_editor_edits_on_worker(&mut self) -> Result<protocol::RecoveryEdits> {
        self.poll_edits_on_worker(true, true)
    }

    fn poll_edits_on_worker(
        &mut self,
        recovery: bool,
        flush_idle: bool,
    ) -> Result<protocol::RecoveryEdits> {
        let result = (|| {
            self.require_live_epoch()?;
            let operation = self
                .edit_query_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("parameter edit operation exhausted"))?;
            let request = if flush_idle {
                protocol::idle_editor_edit_request(&self.config, self.sequence, operation)?
            } else if recovery {
                protocol::recovery_edit_request(&self.config, self.sequence, operation)?
            } else {
                protocol::edit_request(&self.config, self.sequence, operation)?
            };
            let until = end_after(self.limits.exchange)?;
            let token = self.lease.cancellation.clone();
            self.lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.lease
                .channel
                .read(&mut self.packet, until, &mut || token.check())?;
            if self.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.packet.body()?,
                    self.config.binding,
                )?));
            }
            let magic = if recovery { b"HXVIGE02" } else { b"HXVIGE01" };
            if self.packet.magic() != magic {
                return Err(Error::Protocol("missing parameter edit acknowledgement"));
            }
            let body = self.packet.body()?;
            let previous = (self.last_edit_sequence, self.last_edit_frame);
            let result = if recovery {
                protocol::recovery_edits(body, &self.config, self.sequence, operation, previous)?
            } else {
                protocol::RecoveryEdits {
                    edits: protocol::parameter_edits(
                        body,
                        &self.config,
                        self.sequence,
                        operation,
                        previous,
                    )?,
                    pending_latency_restart: None,
                }
            };
            self.require_live_epoch()?;
            self.edit_query_sequence = operation;
            if let Some(last) = result.edits.as_slice().last() {
                self.last_edit_sequence = last.sequence;
                self.last_edit_frame = last.first_frame;
            }
            Ok(result)
        })();
        if result.is_err() {
            self.faulted = true;
            self.lease.cancel_native();
        }
        result
    }
}
impl Quiescent {
    /// Observe uncovered restart intent before planning this stopped epoch's
    /// resume. A new ordinal can arrive during SDK activation.
    pub fn poll_recovery_edits_on_control(&mut self) -> Result<protocol::RecoveryEdits> {
        self.instance.poll_recovery_edits_on_worker()
    }

    /// Same original stopped reader; polling alone never processes Perform.
    /// Apply pending values through the legitimate zero-frame flush contract.
    pub fn poll_parameter_edits_on_control(&mut self) -> Result<protocol::ParameterEdits> {
        self.instance.poll_parameter_edits_on_worker()
    }
}
