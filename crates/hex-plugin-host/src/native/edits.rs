//! Receipts from the original native Host's bounded callback FIFO. The same
//! serialized reader interleaves these control exchanges with PCM/capture.
use super::*;
impl Prepared {
    /// Consume at most eight ordered receipts. Perform means actual successful
    /// SDK processing/flush; Begin/End need no invented audio samples. Pending
    /// callbacks remain native-owned under backpressure. Source cancellation or
    /// refusal terminates queued operations; no receipt grants canonical edits.
    pub fn poll_parameter_edits_on_worker(&mut self) -> Result<protocol::ParameterEdits> {
        let result = (|| {
            self.require_live_epoch()?;
            let operation = self
                .edit_query_sequence
                .checked_add(1)
                .ok_or(Error::Protocol("parameter edit operation exhausted"))?;
            let request = protocol::edit_request(&self.config, self.sequence, operation)?;
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
            if self.packet.magic() != b"HXVIGE01" {
                return Err(Error::Protocol("missing parameter edit acknowledgement"));
            }
            let result = protocol::parameter_edits(
                self.packet.body()?,
                &self.config,
                self.sequence,
                operation,
                (self.last_edit_sequence, self.last_edit_frame),
            )?;
            self.require_live_epoch()?;
            self.edit_query_sequence = operation;
            if let Some(last) = result.as_slice().last() {
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
    /// Same original stopped reader; polling alone never processes Perform.
    /// Apply pending values through the legitimate zero-frame flush contract.
    pub fn poll_parameter_edits_on_control(&mut self) -> Result<protocol::ParameterEdits> {
        self.instance.poll_parameter_edits_on_worker()
    }
}
