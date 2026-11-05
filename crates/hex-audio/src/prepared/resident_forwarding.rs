//! One Resident compatibility request/result. This is never a Verified input.
//! The UI receives a request token, not a publication or callback ACK.
use super::*;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentPublicationToken(u64);
pub struct ResidentPublicationResponse {
    pub token: ResidentPublicationToken,
    pub result: Result<u64, PublishError>,
}
struct Request {
    token: ResidentPublicationToken,
    playback: Playback,
    policy: PublicationPolicy,
    context_generation: u64,
}
#[derive(Default)]
struct Slot {
    next: u64,
    busy: bool,
    suspended: bool,
    worker_owned: bool,
    pcm_frames: u64,
    ever_used: bool,
    request: Option<Request>,
    response: Option<ResidentPublicationResponse>,
    cancellation: Option<Arc<FreshRuntimeCancellation>>,
}
impl Slot {
    fn virgin(&self) -> bool {
        !self.worker_owned
            && !self.ever_used
            && !self.suspended
            && self.pcm_frames == 0
            && !self.busy
            && self.request.is_none()
            && self.response.is_none()
            && self.cancellation.is_none()
    }
}
pub(super) struct ResidentForwarding {
    slot: Mutex<Slot>,
}
impl ResidentForwarding {
    pub(super) fn claim_worker_publication_on_control(
        &self,
        status: &Status,
    ) -> Result<(), &'static str> {
        let mut slot = self
            .slot
            .try_lock()
            .map_err(|_| "resident forwarding admission is busy")?;
        if status.client_exposed.load(Acquire) || status.closing.load(Acquire) || !slot.virgin() {
            return Err("worker graph ownership requires an unexposed virgin forwarding bridge");
        }
        slot.worker_owned = true;
        Ok(())
    }
    pub(super) fn expose_client_on_control(&self, status: &Status) -> bool {
        let slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.client_exposed.store(true, Release);
        slot.worker_owned
    }
    pub(super) fn new() -> Arc<Self> {
        let shared = Arc::new(Self {
            slot: Mutex::new(Slot::default()),
        });
        // Prewarm this actual constructor mutex before Client exposure.
        drop(shared.slot.lock());
        shared
    }
    pub(super) fn release_pcm_after_drop(&self, frames: u64) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.pcm_frames = slot.pcm_frames.checked_sub(frames).unwrap_or(u64::MAX);
    }
    fn virgin(&self) -> bool {
        self.slot.try_lock().is_ok_and(|slot| slot.virgin())
    }
    pub(super) fn storage(
        client: &Arc<Self>,
        worker: &Arc<Self>,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        if !Arc::ptr_eq(client, worker)
            || Arc::strong_count(client) != 2
            || Arc::weak_count(client) != 0
            || !client.virgin()
        {
            return None;
        }
        Some(crate::native_callback_owner::ChannelStorage::paired_payload(client.as_ref()))
    }
}
impl endpoints::ClientEndpoints {
    pub(crate) fn resident_forwarding_storage(
        &self,
        worker: &PublicationControl,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        ResidentForwarding::storage(
            &self.resident_forwarding,
            worker.resident_forwarding.as_ref()?,
        )
    }
}
impl Client {
    /// Resident-only compatibility handoff. A synchronous refusal returns the
    /// exact Playback; a queued request has one later real publication result.
    #[allow(clippy::result_large_err)] // Return the actual graph inline without allocating on refusal.
    pub fn forward_resident_publication(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        context_generation: u64,
    ) -> Result<ResidentPublicationToken, PublishError> {
        let Some(endpoint) = self.owner_ref() else {
            return Err(PublishError {
                reason: "audio client is closed",
                playback,
            });
        };
        if endpoint.status.constructor_mode.load(Acquire)
            || endpoint.status.closing.load(Acquire)
            || !endpoint.status.client_exposed.load(Acquire)
        {
            return Err(PublishError {
                reason: "resident publication unavailable",
                playback,
            });
        }
        let Ok(mut slot) = endpoint.resident_forwarding.slot.try_lock() else {
            return Err(PublishError {
                reason: "resident publication busy",
                playback,
            });
        };
        if slot.worker_owned {
            return Err(PublishError {
                reason: "graph publication belongs to the audio worker for this output",
                playback,
            });
        }
        if slot.suspended {
            return Err(PublishError {
                reason: "resident publication suspended for fresh output",
                playback,
            });
        }
        if slot.busy {
            return Err(PublishError {
                reason: "resident publication busy",
                playback,
            });
        }
        let Some(pcm) = playback
            .resident_pcm_capacity_frames()
            .and_then(|n| slot.pcm_frames.checked_add(n))
        else {
            return Err(PublishError {
                reason: "resident pcm obligation overflow",
                playback,
            });
        };
        let Some(next) = slot.next.checked_add(1) else {
            return Err(PublishError {
                reason: "resident publication token exhausted",
                playback,
            });
        };
        let token = ResidentPublicationToken(next);
        slot.next = next;
        slot.pcm_frames = pcm;
        slot.busy = true;
        slot.ever_used = true;
        slot.request = Some(Request {
            token,
            playback,
            policy,
            context_generation,
        });
        Ok(token)
    }
    /// Resident conditional-seek cancellation borrows the worker's actual
    /// cancellation object through the same finite compatibility cell. It never
    /// clones an owner into the UI. Contention is Busy, not a no-match result;
    /// callers retain/retry until cancellation or an actual terminal observation.
    pub fn cancel_fresh_runtime_publication(
        &self,
        revision: u64,
    ) -> FreshRuntimeCancellationResult {
        let Some(endpoint) = self.owner_ref() else {
            return FreshRuntimeCancellationResult::UnknownRevision;
        };
        let Ok(slot) = endpoint.resident_forwarding.slot.try_lock() else {
            return FreshRuntimeCancellationResult::Busy;
        };
        slot.cancellation
            .as_ref()
            .filter(|c| c.revision == revision)
            .map_or(FreshRuntimeCancellationResult::UnknownRevision, |c| {
                c.request_cancel()
            })
    }
    /// Nonblocking; a contended slot remains pending. The accepted revision is
    /// still distinct from Status.applied_revision produced by the callback.
    pub fn try_resident_publication_response(&mut self) -> Option<ResidentPublicationResponse> {
        let mut slot = self.owner_ref()?.resident_forwarding.slot.try_lock().ok()?;
        let result = slot.response.take()?;
        slot.busy = false;
        Some(result)
    }
}
impl PublicationControl {
    /// Atomic forwarding suspension and actual accepted-PCM snapshot. No
    /// allocation, constructor or owner destruction occurs under this mutex.
    pub fn suspend_resident_forwarding_pcm(&self) -> Option<u64> {
        let bridge = self.resident_forwarding.as_ref()?;
        let mut slot = bridge.slot.try_lock().ok()?;
        slot.suspended = true;
        Some(self.legacy_pcm_frames.saturating_add(slot.pcm_frames))
    }
    pub fn resume_resident_forwarding(&self) {
        if let Some(bridge) = &self.resident_forwarding {
            let mut slot = bridge
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !slot.worker_owned {
                slot.suspended = false;
            }
        }
    }
    pub fn resident_forwarding_pcm_frames(&self) -> Option<u64> {
        let slot = self.resident_forwarding.as_ref()?.slot.try_lock().ok()?;
        Some(self.legacy_pcm_frames.saturating_add(slot.pcm_frames))
    }
    pub(super) fn update_resident_cancellation(&self) {
        let Some(bridge) = &self.resident_forwarding else {
            return;
        };
        let previous = {
            let mut slot = match bridge.slot.lock() {
                Ok(slot) => slot,
                Err(poisoned) => poisoned.into_inner(),
            };
            std::mem::replace(
                &mut slot.cancellation,
                self.latest_fresh_runtime_cancellation
                    .as_ref()
                    .map(Arc::clone),
            )
        };
        // A possibly final cancellation handle never drops under the cell lock
        // or on the Client thread. Verified construction requires virgin None.
        drop(previous);
    }
    pub(super) fn resident_forwarding_virgin(&self) -> bool {
        self.resident_forwarding
            .as_ref()
            .is_none_or(|slot| slot.virgin())
    }
    /// Call only on the Session worker. No mutex spans publication, allocation,
    /// graph retirement, or a potentially owning destructor.
    pub fn poll_resident_publication(&mut self) {
        let request = self
            .resident_forwarding
            .as_ref()
            .and_then(|bridge| bridge.slot.try_lock().ok()?.request.take());
        let Some(request) = request else { return };
        let result = self.publish_with_pcm_obligation(
            request.playback,
            request.policy,
            request.context_generation,
            true,
            true,
        );
        // Refused responses keep their accepted PCM obligation until this Root
        // drains, even if the Client takes the returned owner. No UI ACK releases it.
        // Only this worker fills the result, and busy remains set until the
        // unique Client takes it. No competing request can replace this slot.
        if let Some(bridge) = &self.resident_forwarding {
            let mut slot = match bridge.slot.lock() {
                Ok(slot) => slot,
                Err(poisoned) => poisoned.into_inner(),
            };
            slot.response = Some(ResidentPublicationResponse {
                token: request.token,
                result,
            });
        }
    }
}

pub(super) fn constructor_storage_quote() -> usize {
    std::mem::size_of::<ResidentForwarding>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    fn playback() -> Playback {
        let project = hex_project::Project::new(48000).unwrap();
        let prepared = hex_engine::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
        Playback::from_prepared(Arc::new(prepared))
    }
    fn pcm_playback() -> (Playback, std::sync::Weak<hex_engine::Project>) {
        let audio = hex_engine::Audio::new(48000, vec![[0.25; 2]; 4]).unwrap();
        let clip = hex_engine::Clip::new(audio, 0, 4, false).unwrap();
        let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
        let project = Arc::new(hex_engine::Project::new(48000, vec![track]).unwrap());
        let weak = Arc::downgrade(&project);
        (Playback::from_shared(project), weak)
    }
    fn resource_limits() -> hex_engine::AudioStorageLedgerReservation {
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 256,
            root_rows: 128,
            max_retained_bytes: 20_000_000,
            max_construction_peak_bytes: 20_000_000,
        }
    }
    #[test]
    fn worker_owned_graphs_refuse_client_forwarding_but_keep_transport_and_priority_stop() {
        let _serial = candidate_tests::serial();
        let (mut control, callback) = channel(48000).unwrap();
        control
            .configure_device_resource_publication_on_control(resource_limits())
            .unwrap();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        // Resuming a temporary suspension cannot undo permanent output ownership.
        let control = worker.control().unwrap();
        assert_eq!(control.suspend_resident_forwarding_pcm(), Some(0));
        control.resume_resident_forwarding();
        let (request, weak) = pcm_playback();
        let client_thread = std::thread::spawn(move || {
            let refused = client
                .forward_resident_publication(request, PublicationPolicy::InheritRuntime, 7)
                .unwrap_err();
            assert!(refused.reason.contains("audio worker"));
            assert!(weak.upgrade().is_some());
            drop(refused);
            assert!(weak.upgrade().is_none());
            client
        });
        let mut client = client_thread.join().unwrap();
        let (graph, _) = pcm_playback();
        let revision = worker.control().unwrap().publish(graph).unwrap();
        client.command(PlaybackCommand::Play).unwrap();
        let mut output = [[0.0; 2]; 1];
        let mut render = || {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render();
        assert_eq!(output, [[0.25; 2]; 1]);
        let status = client.status().unwrap();
        assert_eq!(status.applied_revision.load(Acquire), revision);
        assert!(status.playing.load(Acquire));
        // The normal queue is still finite; priority Stop bypasses it.
        let mut queued = 0;
        while client.command(PlaybackCommand::Play).is_ok() {
            queued += 1;
            assert!(queued <= COMMANDS);
        }
        assert!(queued > 0);
        client.stop();
        let mut stop_block = || {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut [[0.0; 2]; 1]).unwrap();
            });
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(&mut stop_block);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        stop_block();
        assert!(!client.status().unwrap().playing.load(Acquire));
        assert!(client.try_resident_publication_response().is_none());
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
    #[test]
    fn worker_graph_configuration_refuses_exposed_or_used_or_contended_bridge() {
        let _serial = candidate_tests::serial();
        let (control, callback) = channel(48000).unwrap();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        let control = worker.control().unwrap();
        let bridge = Arc::clone(control.resident_forwarding.as_ref().unwrap());
        let locked = bridge.slot.lock().unwrap();
        assert!(
            control
                .configure_device_resource_publication_on_control(resource_limits())
                .is_err()
        );
        assert!(control.device_resources.is_none());
        drop(locked);
        // Exposure takes the same slot lock as configuration/admission. Once
        // visible, configuration must refuse without disabling the legacy bridge.
        client.activate();
        assert!(
            control
                .configure_device_resource_publication_on_control(resource_limits())
                .is_err()
        );
        let (graph, _) = pcm_playback();
        client
            .forward_resident_publication(graph, PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        // Also check the final locked guard independently of the exposure guard:
        // an accepted request can never be adopted as a resource-ledger obligation.
        assert!(
            bridge
                .claim_worker_publication_on_control(&control.status)
                .is_err()
        );
        assert!(!bridge.slot.lock().unwrap().worker_owned);
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
    #[test]
    fn pcm_obligation_overflow_is_a_refusing_sentinel_and_preserves_request_owner() {
        let _serial = candidate_tests::serial();
        let (control, callback) = channel(48000).unwrap();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let control = worker.control().unwrap();
        control.legacy_pcm_frames = 1;
        control
            .resident_forwarding
            .as_ref()
            .unwrap()
            .slot
            .lock()
            .unwrap()
            .pcm_frames = u64::MAX;
        assert_eq!(control.suspend_resident_forwarding_pcm(), Some(u64::MAX));
        assert_eq!(control.resident_forwarding_pcm_frames(), Some(u64::MAX));
        control.resume_resident_forwarding();
        let (request, weak) = pcm_playback();
        let refused = client
            .forward_resident_publication(request, PublicationPolicy::InheritRuntime, 7)
            .unwrap_err();
        assert!(refused.reason.contains("overflow"));
        assert!(weak.upgrade().is_some());
        drop(refused);
        assert!(weak.upgrade().is_none());
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
    #[test]
    fn suspended_forwarding_snapshots_real_pcm_until_actual_return_drop() {
        let _serial = candidate_tests::serial();
        let (control, callback) = channel(48000).unwrap();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let (first, weak) = pcm_playback();
        client
            .forward_resident_publication(first, PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        assert_eq!(
            worker.control().unwrap().suspend_resident_forwarding_pcm(),
            Some(4)
        );
        // A caller racing AFTER the same-slot suspension keeps its exact owner.
        let (racing, race_weak) = pcm_playback();
        let thread = std::thread::spawn(move || {
            let refusal = client
                .forward_resident_publication(racing, PublicationPolicy::InheritRuntime, 7)
                .unwrap_err();
            assert!(refusal.reason.contains("suspended"));
            assert!(race_weak.upgrade().is_some());
            drop(refusal);
            assert!(race_weak.upgrade().is_none());
            client
        });
        let mut client = thread.join().unwrap();
        worker.control().unwrap().poll_resident_publication();
        client
            .try_resident_publication_response()
            .unwrap()
            .result
            .unwrap();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        assert!(weak.upgrade().is_some());
        assert_eq!(
            worker.control().unwrap().resident_forwarding_pcm_frames(),
            Some(4)
        );
        worker.control().unwrap().resume_resident_forwarding();
        client
            .forward_resident_publication(playback(), PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        client
            .try_resident_publication_response()
            .unwrap()
            .result
            .unwrap();
        data.owner_mut().unwrap().run(|c, _| c.begin_block());
        assert_eq!(
            worker.control().unwrap().resident_forwarding_pcm_frames(),
            Some(4)
        );
        assert!(weak.upgrade().is_some()); // callback return is not destruction
        worker.control().unwrap().reclaim();
        assert!(weak.upgrade().is_none());
        assert_eq!(
            worker.control().unwrap().resident_forwarding_pcm_frames(),
            Some(0)
        );
        // A refused owning response remains conservatively charged until Root
        // teardown, including after the UI takes/drops it; no ACK subtraction.
        let (queued, _) = pcm_playback();
        client
            .forward_resident_publication(queued, PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        client
            .try_resident_publication_response()
            .unwrap()
            .result
            .unwrap();
        let (refused, _) = pcm_playback();
        client
            .forward_resident_publication(refused, PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        assert!(
            client
                .try_resident_publication_response()
                .unwrap()
                .result
                .is_err()
        );
        assert_eq!(
            worker.control().unwrap().suspend_resident_forwarding_pcm(),
            Some(8)
        );
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
    #[test]
    fn one_request_preserves_refusal_owner_and_separates_request_submission_and_apply() {
        let _serial = candidate_tests::serial();
        let (control, callback) = channel(48000).unwrap();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        let ticket = client
            .forward_resident_publication(playback(), PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        let pending = playback();
        let identity = pending.prepared_revision().cloned().unwrap();
        let refused = client
            .forward_resident_publication(pending, PublicationPolicy::InheritRuntime, 7)
            .unwrap_err();
        assert!(Arc::ptr_eq(
            refused.playback.prepared_revision().unwrap(),
            &identity
        ));
        assert_eq!(client.status().unwrap().submitted_revision.load(Acquire), 0);
        assert!(client.try_resident_publication_response().is_none());
        worker.control().unwrap().poll_resident_publication();
        let response = client.try_resident_publication_response().unwrap();
        assert_eq!(response.token, ticket);
        assert_eq!(response.result.unwrap(), 1);
        assert_eq!(client.status().unwrap().submitted_revision.load(Acquire), 1);
        assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 0);
        // A second request may enter the one cell after result collection, but
        // actual queue/pending pressure still returns its exact original owner.
        let refused_ticket = client
            .forward_resident_publication(refused.playback, PublicationPolicy::InheritRuntime, 7)
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        let response = client.try_resident_publication_response().unwrap();
        assert_eq!(response.token, refused_ticket);
        assert!(Arc::ptr_eq(
            response
                .result
                .unwrap_err()
                .playback
                .prepared_revision()
                .unwrap(),
            &identity
        ));
        let render = || {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                callback.render(&mut [[0.0; 2]; 8]).unwrap();
            });
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 1);
        assert!(client.try_resident_publication_response().is_none());
        let fence = client.capture_fresh_runtime_fence().unwrap();
        assert_eq!(
            worker.control().unwrap().capture_fresh_runtime_fence(),
            Some(fence)
        );
        let request = client
            .forward_resident_publication(
                playback(),
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                7,
            )
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        let response = client.try_resident_publication_response().unwrap();
        assert_eq!(response.token, request);
        let submitted = response.result.unwrap();
        assert_eq!(
            client.cancel_fresh_runtime_publication(submitted + 1),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        assert_eq!(
            client.cancel_fresh_runtime_publication(submitted),
            FreshRuntimeCancellationResult::Cancelled
        );
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 1);
        assert_eq!(
            client.status().unwrap().rejected_revision(),
            Some((submitted, PlaybackHandoffRejection::FreshRuntimeCancelled))
        );
        worker.control().unwrap().reclaim();
        let fence = client.capture_fresh_runtime_fence().unwrap();
        client
            .forward_resident_publication(
                playback(),
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                7,
            )
            .unwrap();
        worker.control().unwrap().poll_resident_publication();
        let successor = client
            .try_resident_publication_response()
            .unwrap()
            .result
            .unwrap();
        assert_eq!(
            client.cancel_fresh_runtime_publication(submitted),
            FreshRuntimeCancellationResult::UnknownRevision
        );
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(
            client.status().unwrap().applied_revision.load(Acquire),
            successor
        );
        assert_eq!(
            client.cancel_fresh_runtime_publication(successor),
            FreshRuntimeCancellationResult::Complete
        );
        worker.control().unwrap().reclaim();
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
    #[test]
    fn contended_client_access_is_nonblocking_and_keeps_original_playback() {
        let (control, callback) = channel(48000).unwrap();
        let (data, errors, terminals, mut client, mut worker) =
            reserve_native_worker(control, callback, None);
        let root = worker.control().unwrap().native_candidate_root();
        let mut retirement = NativeRetirement::new(terminals, root);
        client.activate();
        // Hold the worker's real cell guard; client methods must try, not wait.
        let held = worker
            .control()
            .unwrap()
            .resident_forwarding
            .as_ref()
            .unwrap()
            .slot
            .lock()
            .unwrap();
        let original = playback();
        let identity = original.prepared_revision().cloned().unwrap();
        let refused = client
            .forward_resident_publication(original, PublicationPolicy::InheritRuntime, 0)
            .unwrap_err();
        assert_eq!(refused.reason, "resident publication busy");
        assert!(Arc::ptr_eq(
            refused.playback.prepared_revision().unwrap(),
            &identity
        ));
        assert!(client.try_resident_publication_response().is_none());
        assert_eq!(
            client.cancel_fresh_runtime_publication(1),
            FreshRuntimeCancellationResult::Busy
        );
        drop(held);
        client.close();
        worker.close();
        drop(data);
        drop(errors);
        candidate_tests::finish(&mut retirement);
    }
}
