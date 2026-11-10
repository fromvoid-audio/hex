//! Match session action feedback to its current ticket and project generation.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionTicketOutcome {
    Consumed,
    Rejected,
    Superseded,
    Interrupted,
}

pub(super) fn reconcile_session_receipt(
    pending: Option<(u64, u64)>,
    current_generation: u64,
    receipt: hex_audio::prepared::ActionOutcomeReceipt,
) -> (Option<(u64, u64)>, Option<SessionTicketOutcome>) {
    let Some((ticket, generation)) = pending else {
        return (None, None);
    };
    if generation != current_generation {
        return (None, None);
    }
    if ticket != receipt.ticket {
        return (Some((ticket, generation)), None);
    }
    let outcome = match receipt.outcome {
        hex_audio::prepared::ActionOutcome::Applied => SessionTicketOutcome::Consumed,
        hex_audio::prepared::ActionOutcome::Rejected(_) => SessionTicketOutcome::Rejected,
        hex_audio::prepared::ActionOutcome::Superseded => SessionTicketOutcome::Superseded,
        hex_audio::prepared::ActionOutcome::Interrupted => SessionTicketOutcome::Interrupted,
    };
    (None, Some(outcome))
}

pub(super) fn reconcile_session_ticket(
    pending: Option<(u64, u64)>,
    current_generation: u64,
    consumed_high_water: u64,
    rejected_ticket: Option<u64>,
) -> (Option<(u64, u64)>, Option<SessionTicketOutcome>) {
    let Some((ticket, generation)) = pending else {
        return (None, None);
    };
    if generation != current_generation {
        return (None, None);
    }
    if rejected_ticket == Some(ticket) {
        return (None, Some(SessionTicketOutcome::Rejected));
    }
    if consumed_high_water >= ticket {
        return (None, Some(SessionTicketOutcome::Consumed));
    }
    (Some((ticket, generation)), None)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{SessionTicketOutcome, reconcile_session_receipt, reconcile_session_ticket};

    #[test]
    fn session_ticket_feedback_clears_on_consumption_or_matching_rejection() {
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 4, None),
            (Some((5, 7)), None)
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 5, None),
            (None, Some(SessionTicketOutcome::Consumed))
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 7, 5, Some(5)),
            (None, Some(SessionTicketOutcome::Rejected))
        );
        assert_eq!(
            reconcile_session_ticket(Some((5, 7)), 8, 5, None),
            (None, None)
        );
    }

    #[test]
    fn exact_receipts_retire_only_the_matching_generation_and_ticket() {
        use hex_audio::prepared::{ActionOutcome, ActionOutcomeReceipt};
        let pending = Some((5, 7));
        let receipt = |ticket, outcome| ActionOutcomeReceipt { ticket, outcome };
        for (outcome, expected) in [
            (ActionOutcome::Applied, SessionTicketOutcome::Consumed),
            (
                ActionOutcome::Rejected(hex_engine::PlaybackActionRejection::OutputFailed),
                SessionTicketOutcome::Rejected,
            ),
            (ActionOutcome::Superseded, SessionTicketOutcome::Superseded),
            (
                ActionOutcome::Interrupted,
                SessionTicketOutcome::Interrupted,
            ),
        ] {
            assert_eq!(
                reconcile_session_receipt(pending, 7, receipt(5, outcome)),
                (None, Some(expected))
            );
            assert_eq!(
                reconcile_session_receipt(pending, 7, receipt(6, outcome)),
                (pending, None)
            );
            assert_eq!(
                reconcile_session_receipt(pending, 8, receipt(5, outcome)),
                (None, None)
            );
            assert_eq!(
                reconcile_session_receipt(None, 7, receipt(5, outcome)),
                (None, None)
            );
        }
    }
}
