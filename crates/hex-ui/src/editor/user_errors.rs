//! Human-readable status text for worker, transport and recording failures.
//! Owner types keep their diagnostic `Debug`; the editor never shows it.
use hex_audio::{gain_recording, midi_recording};
use hex_engine::ArrangementPrerollError;
use hex_session::{
    arrangement_seek::ArrangementSeekSubmitError,
    service::{BusyReason, SubmitError},
};

pub(super) fn seek_submit(error: &ArrangementSeekSubmitError) -> &'static str {
    match error {
        ArrangementSeekSubmitError::Closed => "the seek worker stopped · reopen the project",
        ArrangementSeekSubmitError::RequestIdsExhausted => {
            "too many seek requests this session · reopen the project"
        }
        ArrangementSeekSubmitError::RetirementPending => {
            "the previous seek is still being reclaimed · try again"
        }
    }
}

pub(super) fn seek_preparation(error: &ArrangementPrerollError) -> String {
    let fixed = match error {
        ArrangementPrerollError::Cancelled => "the seek was cancelled",
        ArrangementPrerollError::WorkBudgetExceeded => {
            "the seek needs too much preparation · choose a nearer position"
        }
        ArrangementPrerollError::StorageBudgetExceeded => {
            "the seek needs too much memory · choose a nearer position"
        }
        ArrangementPrerollError::UnsupportedTransportLoop => {
            "seeking inside this loop is not supported yet"
        }
        ArrangementPrerollError::ArithmeticOverflow => "the seek position is out of range",
        ArrangementPrerollError::CandidatePositionMismatch => {
            "the project changed during the seek · try again"
        }
        ArrangementPrerollError::MediaPreparation(reason) => {
            return format!("media could not be prepared · {reason}");
        }
        ArrangementPrerollError::Playback(error) => return error.to_string(),
    };
    fixed.into()
}

pub(super) fn submit(error: &SubmitError) -> &'static str {
    match error {
        SubmitError::Full(_) => "the audio worker queue is full · try again",
        SubmitError::Closed(_) => "the audio worker stopped · reopen the project",
        SubmitError::Busy(_, reason) => busy(*reason),
    }
}

/// The actual admission rule behind a `Busy` refusal; never a guessed cause.
pub(super) fn busy(reason: BusyReason) -> &'static str {
    match reason {
        BusyReason::ExportPending => "an export is already pending",
        BusyReason::InvalidBounceTarget => {
            "nothing to bounce · select between one clip and the clip limit"
        }
        BusyReason::SeekWorkerActive => "seeking is owned by the prepared seek worker",
        BusyReason::SeekQueueFull => "too many seeks are pending · try again",
        BusyReason::DocumentIntentExhausted => "too many project opens this session · restart hex",
        BusyReason::InvalidSaveCorrelation => "the save request was malformed · save again",
    }
}

pub(super) fn midi_abort(reason: midi_recording::AbortReason) -> &'static str {
    use midi_recording::AbortReason::*;
    match reason {
        SourceChanged => "the recorded clip changed",
        TransportChanged => "playback stopped or moved",
        InputChanged => "the midi input changed",
        OutputFailed => "the audio output failed",
        ReceiptPressure => "recording events arrived faster than they could be saved",
        TakeFull => "the take reached its note limit",
        InvalidClock => "the recording clock was invalid",
        Cancelled => "recording was cancelled",
        Capacity => "the project reached its note limit",
        RenderFailed => "the instrument could not play",
        RouteChanged => "the track routing changed",
        UnsupportedOutput => "this audio output does not support midi recording",
    }
}

pub(super) fn gain_abort(reason: gain_recording::AbortReason) -> &'static str {
    use gain_recording::AbortReason::*;
    match reason {
        SourceChanged => "the track changed",
        TransportChanged => "playback stopped or moved",
        OutputFailed => "the audio output failed",
        Cancelled => "recording was cancelled",
        ClockExhausted => "the recording clock ran out",
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    fn assert_readable(message: &str) {
        assert!(!message.is_empty());
        assert!(
            !message.contains(['{', '}', '(', ')']) && !message.contains("::"),
            "{message}"
        );
        assert_eq!(message, message.to_lowercase(), "{message}");
    }

    #[test]
    fn seek_errors_are_readable() {
        use ArrangementSeekSubmitError::*;
        for error in [Closed, RequestIdsExhausted, RetirementPending] {
            assert_readable(seek_submit(&error));
        }
        use ArrangementPrerollError::*;
        for error in [
            Cancelled,
            WorkBudgetExceeded,
            StorageBudgetExceeded,
            UnsupportedTransportLoop,
            ArithmeticOverflow,
            CandidatePositionMismatch,
            MediaPreparation("decoder refused the file"),
            Playback(hex_engine::SessionPlaybackError::UnknownClip),
        ] {
            assert_readable(&seek_preparation(&error));
        }
    }

    #[test]
    fn recording_abort_reasons_are_readable() {
        use midi_recording::AbortReason::*;
        for reason in [
            SourceChanged,
            TransportChanged,
            InputChanged,
            OutputFailed,
            ReceiptPressure,
            TakeFull,
            InvalidClock,
            Cancelled,
            Capacity,
            RenderFailed,
            RouteChanged,
            UnsupportedOutput,
        ] {
            assert_readable(midi_abort(reason));
        }
        use gain_recording::AbortReason as Gain;
        for reason in [
            Gain::SourceChanged,
            Gain::TransportChanged,
            Gain::OutputFailed,
            Gain::Cancelled,
            Gain::ClockExhausted,
        ] {
            assert_readable(gain_abort(reason));
        }
    }

    #[test]
    fn submit_errors_never_dump_the_request() {
        let request = || hex_session::service::Request::ContinuePluginEffect;
        for error in [
            SubmitError::Full(request()),
            SubmitError::Closed(request()),
            SubmitError::Busy(request(), BusyReason::ExportPending),
        ] {
            assert_readable(submit(&error));
        }
    }

    #[test]
    fn each_busy_reason_has_its_own_readable_message() {
        use BusyReason::*;
        let reasons = [
            ExportPending,
            InvalidBounceTarget,
            SeekWorkerActive,
            SeekQueueFull,
            DocumentIntentExhausted,
            InvalidSaveCorrelation,
        ];
        let messages: std::collections::BTreeSet<_> = reasons.map(busy).into_iter().collect();
        assert_eq!(
            messages.len(),
            reasons.len(),
            "busy reasons must stay distinct"
        );
        for message in messages {
            assert_readable(message);
        }
    }
}
