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
    ever_used: bool,
    request: Option<Request>,
    response: Option<ResidentPublicationResponse>,
    cancellation: Option<Arc<FreshRuntimeCancellation>>,
}
pub(super) struct ResidentForwarding {
    slot: Mutex<Slot>,
}
impl ResidentForwarding {
    pub(super) fn new() -> Arc<Self> {
        let shared = Arc::new(Self {
            slot: Mutex::new(Slot::default()),
        });
        // Prewarm this actual constructor mutex before Client exposure.
        drop(shared.slot.lock());
        shared
    }
    fn virgin(&self) -> bool {
        self.slot.try_lock().is_ok_and(|slot| {
            !slot.ever_used
                && !slot.busy
                && slot.request.is_none()
                && slot.response.is_none()
                && slot.cancellation.is_none()
        })
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
        if slot.busy {
            return Err(PublishError {
                reason: "resident publication busy",
                playback,
            });
        }
        let Some(next) = slot.next.checked_add(1) else {
            return Err(PublishError {
                reason: "resident publication token exhausted",
                playback,
            });
        };
        let token = ResidentPublicationToken(next);
        slot.next = next;
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
        let result = self.publish_with_policy_and_context_generation(
            request.playback,
            request.policy,
            request.context_generation,
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    fn playback() -> Playback {
        let project = hex_project::Project::new(48000).unwrap();
        let prepared = hex_engine::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            hex_engine::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap();
        Playback::from_prepared(Arc::new(prepared))
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
