//! Actual private Callback constructor's Status Arc and fixed route Vec.
//! Neither a generic Arc adoption API nor complete native-root admission.
use super::{AtomicRouteSlot, Callback, MAX_TRACKS, Status};
use std::{
    alloc::Layout,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize},
};

#[derive(Clone, Copy)]
pub(crate) struct NativeStatusAllocation {
    body: usize,
    routes: usize,
    shared: Layout,
    backing: Layout,
}
fn supported() -> bool {
    let target = match option_env!("HEX_NATIVE_CARRIER_LAYOUT_TARGET") {
        Some("x86_64-unknown-linux-gnu") => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        Some("aarch64-apple-darwin") => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        _ => false,
    };
    target && option_env!("HEX_NATIVE_STATUS_LAYOUT_KNOWN") == Some("1")
}

// Cost of this one fresh, private constructor, not a declaration for an
// arbitrary Arc or proof that its individual transported handles were visited.
pub(super) fn fresh_requested_bytes() -> Option<usize> {
    if !supported() {
        return None;
    }
    shared_layout()?
        .size()
        .checked_add(Layout::array::<AtomicRouteSlot>(MAX_TRACKS).ok()?.size())
}

fn shared_layout() -> Option<Layout> {
    Some(
        Layout::array::<AtomicUsize>(2)
            .ok()?
            .extend(Layout::new::<Status>())
            .ok()?
            .0
            .pad_to_align(),
    )
}

// The actual Callback's private status field is initialized only by
// channel_with_spectrum's fresh Arc::new(Status::try_new()). Shared status
// equality below correlates the other constructor fields, not provenance.
pub(super) fn capture(callback: &Callback) -> Option<NativeStatusAllocation> {
    capture_callback_status(callback.status.as_ref(), supported())
}
#[cfg(all(test, feature = "allocation-probe"))]
pub(super) fn capture_callback_status_for_factory(status: &Status) -> Option<usize> {
    capture_callback_status(status, supported())?.requested_bytes()
}
fn capture_callback_status(status: &Status, qualified: bool) -> Option<NativeStatusAllocation> {
    if !qualified || status.route_slots.len() != MAX_TRACKS {
        return None;
    }
    status.audit_scalar_storage();
    Some(NativeStatusAllocation {
        body: status as *const Status as usize,
        routes: status.route_slots.as_ptr() as usize,
        shared: shared_layout()?,
        backing: Layout::array::<AtomicRouteSlot>(status.route_slots.capacity()).ok()?,
    })
}
impl NativeStatusAllocation {
    pub(crate) fn requested_bytes(self) -> Option<usize> {
        self.shared.size().checked_add(self.backing.size())
    }
    pub(crate) fn matches(self, status: &Status) -> bool {
        self.body == status as *const Status as usize
            && self.routes == status.route_slots.as_ptr() as usize
            && Some(self.backing)
                == Layout::array::<AtomicRouteSlot>(status.route_slots.capacity()).ok()
    }
}

impl Status {
    pub(crate) fn audit_scalar_storage(&self) {
        let Self {
            project_spectrum,
            pending_stop_marker,
            admitted_ticket,
            closing,
            publication_pending,
            preview_used,
            constructor_mode,
            verified_streaming_mode,
            applied_source,
            split_endpoints,
            client_exposed,
            submitted_revision,
            device_fault,
            render_fault,
            render_fault_record,
            callback_max_ns,
            deadline_misses,
            peak_left,
            peak_right,
            applied_revision,
            consumed_command,
            stream_applied_command_prefix,
            stream_buffering,
            stream_scheduled_command,
            stream_activated_command,
            stream_invalidated_schedule,
            stream_readiness_refused,
            stream_committed_frames,
            rejected_action_ticket,
            rejected_action_reason,
            rejected_action_sequence,
            rejected_revision,
            rejected_revision_reason,
            rejected_revision_sequence,
            position,
            performance_frame,
            transport_epoch,
            gain_recording_fault,
            gain_recording_ready,
            playing,
            metronome_enabled,
            session_capability,
            shutdown_complete,
            route_sequence,
            route_revision,
            route_count,
            route_capability,
            route_generation,
            route_performance_at_update,
            route_slots,
        } = self;
        project_spectrum.audit_scalar_storage();
        let _: &AtomicU64 = pending_stop_marker;
        let _: &AtomicU64 = admitted_ticket;
        let _: &AtomicBool = closing;
        publication_pending.audit_scalar_storage();
        let _: &AtomicBool = preview_used;
        let _: &AtomicBool = constructor_mode;
        let _: &AtomicBool = verified_streaming_mode;
        applied_source.audit_scalar_storage();
        let _: &AtomicBool = split_endpoints;
        let _: &AtomicBool = client_exposed;
        let _: &AtomicU64 = submitted_revision;
        let _: &AtomicBool = device_fault;
        let _: &AtomicBool = render_fault;
        render_fault_record.audit_scalar_storage();
        let _: &AtomicU64 = callback_max_ns;
        let _: &AtomicU64 = deadline_misses;
        let _: &AtomicU32 = peak_left;
        let _: &AtomicU32 = peak_right;
        let _: &AtomicU64 = applied_revision;
        let _: &AtomicU64 = consumed_command;
        let _: &AtomicU64 = stream_applied_command_prefix;
        let _: &AtomicBool = stream_buffering;
        let _: &AtomicU64 = stream_scheduled_command;
        let _: &AtomicU64 = stream_activated_command;
        let _: &AtomicU64 = stream_invalidated_schedule;
        let _: &AtomicBool = stream_readiness_refused;
        let _: &AtomicU64 = stream_committed_frames;
        let _: &AtomicU64 = rejected_action_ticket;
        let _: &AtomicU32 = rejected_action_reason;
        let _: &AtomicU64 = rejected_action_sequence;
        let _: &AtomicU64 = rejected_revision;
        let _: &AtomicU32 = rejected_revision_reason;
        let _: &AtomicU64 = rejected_revision_sequence;
        let _: &AtomicU64 = position;
        let _: &AtomicU64 = performance_frame;
        let _: &AtomicU64 = transport_epoch;
        let _: &AtomicBool = gain_recording_fault;
        let _: &AtomicBool = gain_recording_ready;
        let _: &AtomicBool = playing;
        let _: &AtomicBool = metronome_enabled;
        let _: &AtomicU32 = session_capability;
        let _: &AtomicBool = shutdown_complete;
        let _: &AtomicU64 = route_sequence;
        let _: &AtomicU64 = route_revision;
        let _: &AtomicU32 = route_count;
        let _: &AtomicU32 = route_capability;
        let _: &AtomicU64 = route_generation;
        let _: &AtomicU64 = route_performance_at_update;
        for slot in route_slots {
            slot.audit_scalar_storage();
        }
    }
}

impl AtomicRouteSlot {
    fn audit_scalar_storage(&self) {
        let Self {
            track_id,
            owner_tag,
            owner_clip_id,
            active_launch_tag,
            active_launch_beat_raw,
            pending_tag,
            pending_target_tag,
            pending_clip_id,
            pending_frame,
            pending_beat_raw,
        } = self;
        let _: &AtomicU64 = track_id;
        let _: &AtomicU32 = owner_tag;
        let _: &AtomicU64 = owner_clip_id;
        let _: &AtomicU32 = active_launch_tag;
        let _: &AtomicU64 = active_launch_beat_raw;
        let _: &AtomicU32 = pending_tag;
        let _: &AtomicU32 = pending_target_tag;
        let _: &AtomicU64 = pending_clip_id;
        let _: &AtomicU64 = pending_frame;
        let _: &AtomicU64 = pending_beat_raw;
    }
}

impl Callback {
    pub(crate) fn constructor_status_storage(&self) -> Option<NativeStatusAllocation> {
        capture(self)
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::Ordering};
    #[test]
    fn constructor_status_storage_matches_actual_requested_allocations_and_fixed_leaf_mutation() {
        assert!(supported(), "native Status constructor gate must execute");
        eprintln!("SUPPORTED native-callback-status-arc-route-box-v1");
        let mut owner = None;
        let measured =
            allocation_counter::measure(|| owner = Some(Arc::new(Status::try_new().unwrap())));
        let owner = owner.unwrap();
        let receipt = capture_callback_status(&owner, supported()).unwrap();
        assert_eq!(measured.count_total, 2);
        assert_eq!(measured.count_current, 2);
        let bytes = receipt.requested_bytes().unwrap();
        assert_eq!(measured.bytes_total, u64::try_from(bytes).unwrap());
        assert_eq!(measured.bytes_current, i64::try_from(bytes).unwrap());
        let observed = allocation_counter::measure(|| {
            for frame in 0..128 {
                owner.position.store(frame, Ordering::Relaxed);
                owner.route_slots[0]
                    .pending_frame
                    .store(frame, Ordering::Relaxed);
                assert!(receipt.matches(&owner));
                assert_eq!(receipt.requested_bytes(), Some(bytes));
            }
        });
        assert_eq!(
            (
                observed.count_total,
                observed.bytes_total,
                observed.count_current,
                observed.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(u64::try_from(bytes - 1).unwrap() < measured.bytes_total);
        let equal = Arc::new(Status::try_new().unwrap());
        assert!(!receipt.matches(&equal));
        assert!(capture_callback_status(&owner, false).is_none());
    }
    #[test]
    fn constructor_status_storage_comes_from_actual_callback_and_matches_all_native_status_fields()
    {
        assert!(supported(), "native Status constructor gate must execute");
        eprintln!("SUPPORTED actual callback constructor Status correlation");
        let (control, callback) = super::super::channel(48_000).unwrap();
        let receipt = capture(&callback).unwrap();
        assert!(receipt.matches(&control.status));
        let root = control.native_candidate_root();
        let (mut data, errors, terminals) =
            crate::native_callback_owner::reserve_native(callback, &control.status).unwrap();
        let mut retirement =
            crate::native_callback_owner::NativeRetirement::new(terminals, root.clone());
        assert!(root.native_boundary_bytes().unwrap() >= receipt.requested_bytes().unwrap());
        assert!(receipt.matches(data.owner_ref().unwrap().borrowed_status()));
        data.owner_mut()
            .unwrap()
            .run(|_, status| assert!(receipt.matches(status)));
        // Both native carriers were correlated with this exact callback Status;
        // retaining another field never creates another physical Status body.
        drop(errors);
        assert!(receipt.matches(&control.status));
        drop(data);
        assert!(retirement.advance().unwrap());
        assert!(receipt.matches(&control.status));
    }
}
