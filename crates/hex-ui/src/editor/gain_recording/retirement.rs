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
    pub(in crate::editor) fn retire_audio_preserving_gain_take(&mut self) {
        self.poll_retired_gain_receipts();
        if let Some(mut audio) = self.audio.take() {
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
        }
        self.gain_recorder.audio_lost();
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
