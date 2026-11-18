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
    pub(super) approved: Option<DeviceInsertConsent>,
    // Only PublicationControl creates an offer. No callback uses this cell.
    offer_deadline: OnceLock<Instant>,
}

impl DeviceInsertBasis {
    pub(super) fn new(
        predecessor: AppliedAudioSource,
        fence: FreshRuntimeFence,
        status: &Status,
        approved: Option<DeviceInsertConsent>,
    ) -> Self {
        let deadline = Instant::now() + INSTALL_WINDOW;
        Self {
            predecessor,
            fence,
            transport_epoch: approved.map_or_else(
                || status.transport_epoch.load(Acquire),
                |consent| consent.transport_epoch,
            ),
            install_deadline: approved.map_or(deadline, |consent| deadline.min(consent.expires_at)),
            approved,
            offer_deadline: OnceLock::new(),
        }
    }

    pub(super) fn cut_is_current(&self, status: &Status, active: Option<&Publication>) -> bool {
        Instant::now() < self.install_deadline
            && active.is_some_and(|graph| graph.source_context == Some(self.predecessor.context))
            && !Callback::runtime_fence_changed(status, active, self.fence)
            && status.consumed_command.load(Acquire) == self.fence.admitted_command_ticket
            && status.transport_epoch.load(Acquire) == self.transport_epoch
            && !status.closing.load(Acquire)
    }
}

pub(super) struct DeviceInsertAdmission<'a> {
    pub(super) bind: &'a mut dyn FnMut(&CancellablePublicationReceipt),
    pub(super) approved: Option<DeviceInsertConsent>,
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
    /// Offer only when the observed source can use the resident cut.
    /// The callback still rechecks the exact routes before fading.
    pub fn offer_resident_device_insert_consent(
        &self,
        receipt: &CancellablePublicationReceipt,
    ) -> Option<DeviceInsertConsent> {
        if self.output_transport_reaper.is_some() {
            return None;
        }
        let basis = receipt.device_insert_basis()?;
        let mut routes = [SessionRouteEntry::default(); MAX_TRACKS];
        let snapshot = self.status.session_routes(&mut routes)?;
        if snapshot.audio_revision != basis.fence.expected_active_revision
            || snapshot.route_state_generation != basis.fence.route_state_generation
            || routes[..snapshot.count]
                .iter()
                .any(|route| route.pending.is_some())
        {
            return None;
        }
        self.offer_device_insert_consent(receipt)
    }

    /// Prepare the exact refused edit for a confirmed interruption on its
    /// original output. Both admission and the callback revalidate consent.
    #[allow(clippy::result_large_err)] // Refusal returns the actual graph to control.
    pub fn publish_confirmed_device_insert_with_receipt_on_control<E>(
        &mut self,
        playback: Playback,
        consent: DeviceInsertConsent,
        mut bind_receipt: impl FnMut(&CancellablePublicationReceipt),
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            PublicationPolicy::DeviceInsert {
                predecessor: consent.predecessor,
                fence: consent.fence,
            },
            consent.candidate.project_generation,
            Some(consent.candidate),
            Some(DeviceInsertAdmission {
                bind: &mut bind_receipt,
                approved: Some(consent),
            }),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }

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
        if basis.approved.is_some() {
            return None;
        }
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

impl Publication {
    pub(super) fn approved_device_insert(&self) -> Option<&DeviceInsertBasis> {
        self.fresh_runtime_cancellation
            .as_ref()
            .and_then(|reader| reader.0.device_insert_basis())
            .filter(|basis| basis.approved.is_some())
    }
}

impl Callback {
    pub(super) fn abort_blocked_device_insert(&mut self) {
        if let Some(next) = self.deferred.as_ref()
            && let Some(basis) = next.approved_device_insert()
            && self.recording_source() == Some(basis.predecessor)
            && let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
            && active.playback.device_insert_cut_pending()
        {
            active.playback.release_device_insert_cut();
            if let Some(receipt) = next.fresh_runtime_cancellation.as_ref() {
                receipt.returned(Some(PlaybackHandoffRejection::RuntimeFenceChanged));
            }
        }
    }

    /// Start and commit only at physical callback boundaries. Source-free
    /// ordinary stream workspaces have no page roles to promote at this cut.
    pub(super) fn advance_confirmed_device_insert(
        &mut self,
        next: &Publication,
        basis: &DeviceInsertBasis,
    ) -> Result<bool, PlaybackHandoffRejection> {
        let stale = PlaybackHandoffRejection::RuntimeFenceChanged;
        let active = self.active.as_ref().and_then(ActivePublication::legacy);
        if !basis.cut_is_current(&self.status, active)
            || self.recording_source() != Some(basis.predecessor)
            || self.transport_fade.is_some()
            || self.stream_pending_play.is_some()
            || self.stream_action_pending()
            || self.output_transport.is_some()
            || self.native_effects_revoked()
        {
            return Err(stale);
        }
        let active = active.ok_or(stale)?;
        let source_free = |graph: &Publication| {
            !graph.playback.contains_streamed_sources()
                && graph
                    .streams
                    .as_ref()
                    .is_none_or(|streams| streams.permits_paused_monitor())
        };
        if !source_free(active) || !source_free(next) {
            return Err(PlaybackHandoffRejection::StreamedPagesNotReady);
        }
        // A scheduled launch has already been admitted at an exact cut.
        // Its timing takes precedence over the confirmed edit.
        if (0..active.playback.session_route_count()).any(|index| {
            active
                .playback
                .session_route_status(index)
                .is_some_and(|route| route.pending.is_some())
        }) {
            return Err(stale);
        }
        if active.playback.device_insert_cut_pending() {
            return Ok(active.playback.device_insert_held());
        }
        if !next
            .fresh_runtime_cancellation
            .as_ref()
            .is_some_and(PublicationReceiptOwner::begin_confirmed_fade)
        {
            return Err(PlaybackHandoffRejection::FreshRuntimeCancelled);
        }
        if !basis.cut_is_current(&self.status, Some(active)) {
            return Err(stale);
        }
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .ok_or(stale)?;
        if !active.playback.begin_device_insert_cut() {
            return Err(stale);
        }
        Ok(false)
    }

    pub(super) fn release_refused_device_insert_cut(&mut self, publication: &Publication) {
        if let Some(basis) = publication.approved_device_insert()
            && !self.status.closing.load(Acquire)
            && self.recording_source() == Some(basis.predecessor)
            && let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
        {
            active.playback.release_device_insert_cut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepared::latency_edit_timing_tests::{
        begin_insert, begin_insert_with_pending_track, revisions,
    };

    #[test]
    fn confirmed_insert_preserves_a_launched_session_clips_exact_source_position() {
        use hex_engine::{LaunchQuantization, SessionPlaybackCommand};
        use hex_project::{Command, Created, Editor, Project};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("import");
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene");
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "ramp".into(),
                asset,
                source_start: 32,
                source_end: 8192,
            })
            .unwrap()
        else {
            panic!("session audio clip");
        };
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 1024,
                enabled: true,
            })
            .unwrap();
        let sample = |frame: u64| [0.125 + frame as f32 / 65_536.0; 2];
        let media = std::collections::BTreeMap::from([(
            asset,
            hex_engine::Audio::new(48_000, (0..8192).map(sample).collect()).unwrap(),
        )]);
        let table = Arc::new(hex_engine::SineTable::new());
        let prepare = |project: &Project| {
            Arc::new(
                hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                    .unwrap(),
            )
        };
        let limiter = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: limiter.clone(),
            })
            .unwrap();
        let old = prepare(editor.project());
        let old_latency = old.arrangement().main_latency_frames();
        assert!(old_latency > 0);
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: limiter,
            })
            .unwrap();
        let next = prepare(editor.project());
        let latency = next.arrangement().main_latency_frames();
        assert!(latency > old_latency);

        let (mut control, mut callback) = channel(48_000).unwrap();
        let initial = hex_engine::AudioSourceContext {
            project_generation: 5,
            editor_revision: 7,
        };
        let candidate = hex_engine::AudioSourceContext {
            editor_revision: 8,
            ..initial
        };
        control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(old).unwrap(),
                PublicationPolicy::InstallFreshRuntime,
                initial.project_generation,
                true,
                false,
                Some(initial),
            )
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 192]).unwrap();
        control
            .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 2048]).unwrap();
        let mut routes = [SessionRouteEntry::default(); 1];
        control.status.session_routes(&mut routes).unwrap();
        let launch = routes[0].audio_launch_frame.unwrap();
        assert!(launch > 0);
        let predecessor = control.status.recording_source().unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let policy = PublicationPolicy::DeviceInsert { predecessor, fence };
        let publish = |control: &mut Control, approval: Option<DeviceInsertConsent>| {
            let mut bind_receipt = |_: &CancellablePublicationReceipt| {};
            control
                .publish_unaccounted_with_native_continuation(
                    Playback::try_from_prepared_with_session(Arc::clone(&next)).unwrap(),
                    policy,
                    initial.project_generation,
                    true,
                    false,
                    Some(candidate),
                    approval.map(|approved| DeviceInsertAdmission {
                        bind: &mut bind_receipt,
                        approved: Some(approved),
                    }),
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    None,
                )
                .unwrap()
        };

        let revision = publish(&mut control, None);
        let refused = control.fresh_runtime_receipt(revision).unwrap();
        callback.begin_block();
        let consent = control
            .offer_resident_device_insert_consent(&refused)
            .unwrap();
        control.reclaim();
        let revision = publish(&mut control, Some(consent));
        let receipt = control.fresh_runtime_receipt(revision).unwrap();
        let cut = 192 + 2048 + 128;
        let mut pcm = [[0.0; 2]; 512];
        let mut render = || {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 256]).unwrap();
            assert_eq!(control.status.performance_frame.load(Acquire), cut);
            assert_eq!(control.status.position.load(Acquire), cut % 1024);
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0),
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        assert_eq!(receipt.state(), PublicationReceiptState::Installed);
        assert_eq!(control.status.performance_frame.load(Acquire), cut + 512);
        assert_eq!(control.status.position.load(Acquire), (cut + 512) % 1024);
        control.status.session_routes(&mut routes).unwrap();
        assert_eq!(routes[0].audio_launch_frame, Some(launch));
        assert_eq!(
            pcm[511],
            sample(32 + cut - launch + 511 - u64::from(latency))
        );
    }

    #[test]
    fn confirmed_insert_yields_to_priority_stop_admitted_routes_and_output_loss() {
        enum Interrupt {
            StopBeforeFade,
            StopAtCut,
            PendingRoute,
            OutputFault,
        }
        for interrupt in [
            Interrupt::StopBeforeFade,
            Interrupt::StopAtCut,
            Interrupt::PendingRoute,
            Interrupt::OutputFault,
        ] {
            let ([old, next], track) = revisions();
            let pending_track = matches!(interrupt, Interrupt::PendingRoute).then_some(track);
            let (mut control, mut callback, predecessor, original) =
                begin_insert_with_pending_track(
                    old,
                    Arc::clone(&next),
                    pending_track,
                    if pending_track.is_some() { 2990 } else { 512 },
                );
            callback.begin_block();
            let consent = control.offer_device_insert_consent(&original).unwrap();
            control.reclaim();
            let revision = control
                .publish_unaccounted_with_native_continuation(
                    Playback::try_from_prepared_with_session(next).unwrap(),
                    PublicationPolicy::DeviceInsert {
                        predecessor,
                        fence: consent.fence,
                    },
                    consent.candidate.project_generation,
                    true,
                    false,
                    Some(consent.candidate),
                    Some(DeviceInsertAdmission {
                        approved: Some(consent),
                        bind: &mut |_| {},
                    }),
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    None,
                )
                .unwrap();
            let receipt = control.fresh_runtime_receipt(revision).unwrap();
            let mut pcm = [[0.0; 2]; 256];
            if matches!(interrupt, Interrupt::StopAtCut | Interrupt::OutputFault) {
                callback.begin_block();
                callback.render(&mut pcm).unwrap();
                assert_eq!(receipt.state(), PublicationReceiptState::Applying);
                assert_eq!(pcm[0], [0.25 * 127.0 / 128.0; 2]);
                assert!(pcm[127..].iter().all(|frame| *frame == [0.0; 2]));
            }
            if matches!(interrupt, Interrupt::OutputFault) {
                // An output error can be its last callback. The worker still
                // closes an interrupted fade that never entered installation.
                control.status.device_fault.store(true, Release);
                control.poll_resident_publication();
                assert_eq!(receipt.state(), PublicationReceiptState::OutputClosed);
                assert_eq!(control.status.recording_source(), Some(predecessor));
                continue;
            }
            if pending_track.is_none() {
                control.stop();
            }
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
            assert_eq!(
                receipt.state(),
                PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
            );
            assert_eq!(control.status.recording_source(), Some(predecessor));
            if pending_track.is_none() {
                assert_eq!(control.status.position.load(Acquire), 0);
                assert!(!control.status.playing.load(Acquire));
                assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
            } else {
                assert_eq!(pcm[..10], [[0.25; 2]; 10]);
                assert!(pcm[10..].iter().all(|frame| *frame == [0.0; 2]));
                assert!(control.status.playing.load(Acquire));
            }
        }
    }

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
