//! A stopped or stale output cannot authorize an interruption of its successor.
//! The control owner derives a short-lived offer from an exact rejected
//! publication. Its witness ignores cursor advancement but includes every
//! frontier whose change can affect the promised handoff.
use super::*;
use std::{sync::OnceLock, time::Duration};

const INSTALL_WINDOW: Duration = Duration::from_millis(250);
const CONSENT_WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(super) struct DeviceInsertBasis {
    predecessor: AppliedAudioSource,
    fence: FreshRuntimeFence,
    transport_epoch: u64,
    pub(super) install_deadline: Instant,
    // Only PublicationControl creates an offer. No callback uses this cell.
    offer_deadline: OnceLock<Instant>,
}

impl DeviceInsertBasis {
    pub(super) fn new(
        predecessor: AppliedAudioSource,
        fence: FreshRuntimeFence,
        status: &Status,
    ) -> Self {
        Self {
            predecessor,
            fence,
            transport_epoch: status.transport_epoch.load(Acquire),
            install_deadline: Instant::now() + INSTALL_WINDOW,
            offer_deadline: OnceLock::new(),
        }
    }
}

/// The evidence for one deliberate interruption. It owns no graph, resources,
/// or transport command. Creating or validating an offer never changes audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceInsertConsent {
    predecessor: AppliedAudioSource,
    candidate: hex_engine::AudioSourceContext,
    refused_revision: u64,
    fence: FreshRuntimeFence,
    transport_epoch: u64,
    expires_at: Instant,
}

impl DeviceInsertConsent {
    pub fn predecessor(&self) -> AppliedAudioSource {
        self.predecessor
    }

    pub fn candidate_context(&self) -> hex_engine::AudioSourceContext {
        self.candidate
    }

    pub fn refused_revision(&self) -> u64 {
        self.refused_revision
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    fn matches_at(&self, control: &PublicationControl, now: Instant) -> bool {
        let status = &control.status;
        now < self.expires_at
            && control.output_identity() == Some(self.predecessor.output_identity)
            && status.recording_source() == Some(self.predecessor)
            && status.playing.load(Acquire)
            && !status.closing.load(Acquire)
            && !status.publication_in_flight()
            && capture_runtime_fence(status) == Some(self.fence)
            && status.transport_epoch.load(Acquire) == self.transport_epoch
            && status.consumed_command.load(Acquire) == self.fence.admitted_command_ticket
            && status.admitted_ticket.load(Acquire) == self.fence.admitted_command_ticket
    }
}

impl PublicationControl {
    /// Offer consent only for a device insert refused for continuity. The
    /// receipt must still be the latest submitted publication on this owner.
    /// Asking again for the same offer preserves its first deadline.
    pub fn offer_device_insert_consent(
        &self,
        receipt: &CancellablePublicationReceipt,
    ) -> Option<DeviceInsertConsent> {
        if self
            .latest_fresh_runtime_cancellation
            .as_ref()
            .is_none_or(|latest| !latest.same_decision(receipt))
            || receipt.state()
                != PublicationReceiptState::Refused(
                    PlaybackHandoffRejection::DeviceInsertNeedsConfirmation,
                )
        {
            return None;
        }
        let basis = receipt.device_insert_basis()?;
        let now = Instant::now();
        let consent = DeviceInsertConsent {
            predecessor: basis.predecessor,
            candidate: receipt.context()?,
            refused_revision: receipt.revision(),
            fence: basis.fence,
            transport_epoch: basis.transport_epoch,
            expires_at: basis
                .offer_deadline
                .get()
                .copied()
                .unwrap_or(now + CONSENT_WINDOW),
        };
        if !consent.matches_at(self, now) {
            return None;
        }
        let expires_at = *basis.offer_deadline.get_or_init(|| consent.expires_at);
        (now < expires_at).then_some(DeviceInsertConsent {
            expires_at,
            ..consent
        })
    }

    /// Revalidate the exact offer before accepting Proceed. A callback must
    /// still check its admitted frontier before making the first audible change.
    pub fn device_insert_consent_is_current(&self, consent: &DeviceInsertConsent) -> bool {
        self.latest_fresh_runtime_cancellation
            .as_ref()
            .is_some_and(|receipt| {
                receipt.output_identity() == consent.predecessor.output_identity
                    && receipt.revision() == consent.refused_revision
                    && receipt.context() == Some(consent.candidate)
            })
            && consent.matches_at(self, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepared::latency_edit_timing_tests::{begin_insert, revisions};

    #[test]
    fn offer_tracks_one_refusal_and_expires_even_when_requested_again() {
        let ([old, next], track) = revisions();
        let (control, mut callback, predecessor, receipt) = begin_insert(old, next, track, 512);
        assert!(control.offer_device_insert_consent(&receipt).is_none());
        callback.begin_block();
        assert_eq!(
            receipt.state(),
            PublicationReceiptState::Refused(
                PlaybackHandoffRejection::DeviceInsertNeedsConfirmation
            ),
        );
        let offer = control.offer_device_insert_consent(&receipt).unwrap();
        assert_eq!(offer.predecessor(), predecessor);
        assert_eq!(offer.refused_revision(), receipt.revision());
        assert_eq!(Some(offer.candidate_context()), receipt.context());
        assert!(control.device_insert_consent_is_current(&offer));
        let deadline = offer.expires_at();
        assert!(offer.matches_at(&control, deadline - Duration::from_millis(1)));
        assert!(!offer.matches_at(&control, deadline));

        let mut out = [[0.0; 2]; 64];
        let position = control.status.position.load(Acquire);
        callback.render(&mut out).unwrap();
        assert_eq!(
            control.status.position.load(Acquire),
            position + out.len() as u64
        );
        assert!(control.device_insert_consent_is_current(&offer));
        assert_eq!(control.offer_device_insert_consent(&receipt), Some(offer));

        let (other, _) = channel(48_000).unwrap();
        assert!(!other.device_insert_consent_is_current(&offer));
        assert!(other.offer_device_insert_consent(&receipt).is_none());
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert!(control.status.playing.load(Acquire));
    }

    #[test]
    fn new_command_or_priority_stop_revokes_before_callback_processing() {
        for stop in [false, true] {
            let ([old, next], track) = revisions();
            let (mut control, mut callback, predecessor, receipt) =
                begin_insert(old, next, track, 512);
            callback.begin_block();
            let offer = control.offer_device_insert_consent(&receipt).unwrap();
            assert!(control.device_insert_consent_is_current(&offer));
            if stop {
                control.stop();
            } else {
                control.command(PlaybackCommand::Pause).unwrap();
            }
            assert!(!control.device_insert_consent_is_current(&offer));
            assert!(control.offer_device_insert_consent(&receipt).is_none());
            assert_eq!(control.status.recording_source(), Some(predecessor));
            assert!(control.status.playing.load(Acquire));
        }
    }

    #[test]
    fn offer_cannot_outlive_an_admitted_action_or_output_fault() {
        for fault in [false, true] {
            let ([old, next], track) = revisions();
            let (control, mut callback, predecessor, receipt) =
                begin_insert(old, next, track, 2990);
            callback.begin_block();
            let offer = control.offer_device_insert_consent(&receipt).unwrap();
            assert!(control.device_insert_consent_is_current(&offer));
            if fault {
                callback.report_render_fault(None, 2990);
            } else {
                // The already-admitted Stop Track changes the route after the
                // offer, without any new command ticket reaching control.
                callback.render(&mut [[0.0; 2]; 64]).unwrap();
                assert_eq!(
                    control.status.admitted_ticket.load(Acquire),
                    offer.fence.admitted_command_ticket
                );
            }
            assert!(!control.device_insert_consent_is_current(&offer));
            assert!(control.offer_device_insert_consent(&receipt).is_none());
            assert_eq!(control.status.recording_source(), Some(predecessor));
        }
    }
    #[test]
    fn newer_publication_revokes_old_consent_before_installing() {
        let ([old, next], track) = revisions();
        let (mut control, mut callback, predecessor, receipt) =
            begin_insert(Arc::clone(&old), next, track, 512);
        callback.begin_block();
        let offer = control.offer_device_insert_consent(&receipt).unwrap();
        control.reclaim();
        let current = hex_engine::AudioSourceContext {
            editor_revision: offer.candidate_context().editor_revision + 1,
            ..predecessor.context
        };
        let revision = control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(old).unwrap(),
                PublicationPolicy::InheritRuntime,
                current.project_generation,
                true,
                false,
                Some(current),
            )
            .unwrap();
        assert!(revision > offer.refused_revision());
        // A late Proceed must fail while the predecessor is still playing and
        // again after the newer graph actually installs on this output.
        assert_eq!(control.status.recording_source(), Some(predecessor));
        assert!(!control.device_insert_consent_is_current(&offer));
        assert!(control.offer_device_insert_consent(&receipt).is_none());
        callback.begin_block();
        assert_eq!(
            control.status.recording_source().unwrap().audio_revision,
            revision
        );
        assert!(!control.device_insert_consent_is_current(&offer));
    }
}
