//! Keep only the detached Copy-receipt observation across output retirement.
//! The existing audio reaper owns quiescence and endpoints; this token owns no
//! stream/resource lease. A new capture cannot arm while this draft is retained.
use super::*;
use hex_audio::prepared::{
    RetiredScalarReceipts, ScalarReceipt, ScalarReceiptTarget, ScalarReceiptTerminal,
};

pub(super) struct RetiredCapture {
    target: Target,
    receipts: RetiredScalarReceipts,
}

impl EditorView {
    /// Every replacement of the current endpoint shares this custody transfer.
    /// Closing cannot race away a final sink receipt: only the existing reaper
    /// completes the detached mailbox after stream and callback quiescence.
    pub(in crate::editor) fn retire_audio_preserving_gain_take(&mut self) -> bool {
        self.poll_retired_gain_receipts();
        self.poll_retired_action_outcomes();
        if let Some(mut audio) = self.audio.take() {
            let retain_client =
                self.retired_action_outcomes.is_some() && audio.control.has_action_outcomes();
            let identity = audio.control.output_identity();
            if let Some(receipts) = audio.control.close_retaining_gain_receipts() {
                if self.gain_recorder.retired.is_none()
                    && identity == Some(receipts.output_identity())
                    && self.gain_recorder.target.map(ScalarReceiptTarget::Gain)
                        == Some(receipts.target())
                {
                    if let ScalarReceiptTarget::Gain(target) = receipts.target() {
                        self.gain_recorder.retired = Some(RetiredCapture { target, receipts });
                    }
                } else {
                    // A different output/target cannot authenticate this draft.
                    // Keep its delivered events incomplete, never re-stamp them.
                    self.error =
                        Some("gain receipt source mismatch · incomplete take retained".into());
                }
            }
            if retain_client {
                // Keep the closed client and its reader rather than discard an
                // unobserved terminal when an older output has not retired.
                self.audio = Some(audio);
                self.error = Some("waiting for previous audio outcomes · retry audio".into());
                self.gain_recorder.audio_lost();
                return false;
            }
            if let Some(reader) = audio.control.take_action_outcomes() {
                self.retired_action_outcomes = Some(reader);
            }
        }
        self.gain_recorder.audio_lost();
        true
    }

    pub(in crate::editor) fn poll_retired_action_outcomes(&mut self) -> bool {
        // Keep one bounded batch until the editor tick has displayed it. A
        // second retiring output retains its reader and backpressure instead
        // of overwriting the first batch.
        if self.retired_action_message.is_some() {
            return false;
        }
        let Some(mut reader) = self.retired_action_outcomes.take() else {
            return false;
        };
        // Only an acquire observed before the drain proves that a subsequent
        // empty read cannot race the final quiescent publication.
        let retired = reader.is_retired();
        let mut messages = String::new();
        for _ in 0..hex_audio::prepared::ACTION_OUTCOME_CAPACITY {
            let Some(receipt) = reader.try_take() else {
                break;
            };
            let message = match receipt.outcome {
                hex_audio::prepared::ActionOutcome::Rejected(reason) => Some(reason.message()),
                hex_audio::prepared::ActionOutcome::Interrupted => {
                    Some("previous audio action interrupted")
                }
                _ => None,
            };
            if let Some(message) = message {
                if !messages.is_empty() {
                    messages.push_str(" · ");
                }
                messages.push_str(message);
            }
        }
        if reader.failure().is_some() && messages.is_empty() {
            messages.push_str("previous audio outcomes unavailable · restart audio");
        }
        if !retired {
            self.retired_action_outcomes = Some(reader);
        }
        if messages.is_empty() {
            return false;
        }
        self.retired_action_message = Some(messages.clone());
        if self.error.as_deref() == Some(messages.as_str()) {
            return false;
        }
        self.error = Some(messages.into());
        true
    }

    pub(in crate::editor) fn poll_retired_gain_receipts(&mut self) -> bool {
        let Some(retired) = self.gain_recorder.retired.as_mut() else {
            return false;
        };
        let Some(result) = retired.receipts.try_take() else {
            return false;
        };
        let target = retired.target;
        self.gain_recorder.retired = None;
        match result {
            Ok(batch)
                if batch.target() == ScalarReceiptTarget::Gain(target)
                    && self.gain_recorder.target == Some(target) =>
            {
                for receipt in batch.receipts() {
                    if let ScalarReceipt::Gain(receipt) = receipt {
                        self.gain_receipt(receipt);
                    }
                }
                if batch.terminal() == ScalarReceiptTerminal::Interrupted {
                    self.error = Some(
                        "audio interrupted · incomplete gain take retained · discard explicitly"
                            .into(),
                    );
                }
            }
            _ => {
                self.error = Some(
                    "gain receipts unavailable · incomplete take retained · discard explicitly"
                        .into(),
                );
            }
        }
        // Finished has already sealed the real end clock (and may have queued
        // its edit). Every other outcome retains an honestly incomplete draft.
        self.gain_recorder.audio_lost();
        true
    }
}
