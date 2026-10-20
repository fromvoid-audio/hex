//! Inline custody for the existing packet Box. No additional payload allocation
//! or owning getter: the lease travels with this SAME Box at every queue move.
use super::*;
use hex_engine::sampler_host_fields::{
    BodyRef, NativeSpectrumPacketFields, NativeSpectrumPacketLease,
};
pub(super) struct PacketOwner {
    pub(super) payload: Option<Box<ProjectPacket>>,
    storage: Option<NativeSpectrumPacketLease>,
}
impl PacketOwner {
    pub(super) fn new(
        value: ProjectPacket,
        parent: Option<&crate::native_callback_owner::NativeRetirement>,
        index: usize,
    ) -> Result<Self, SpectrumWorkerError> {
        let payload = Box::new(value);
        let storage = parent
            .map(|parent| parent.pair_spectrum_packet_on_control(index, fields(&payload)))
            .transpose()
            .map_err(|_| SpectrumWorkerError::StorageRefused)?;
        Ok(Self {
            payload: Some(payload),
            storage,
        })
    }
    pub(super) fn matches_record(
        &self,
        record: &hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration,
    ) -> bool {
        self.payload
            .as_deref()
            .zip(self.storage.as_ref())
            .is_some_and(|(packet, lease)| {
                record.matches_actual_packet_on_control(lease, fields(packet))
            })
    }
    pub(super) fn data_bytes(&self) -> Option<usize> {
        self.payload.as_deref()?.data_bytes()
    }
}
fn fields(packet: &ProjectPacket) -> NativeSpectrumPacketFields<'_> {
    let ProjectPacket {
        selection: _,
        sequence: _,
        revision: _,
        captured_at: _,
        frames: _,
        samples,
    } = packet;
    NativeSpectrumPacketFields::of(BodyRef::of(packet), samples)
}
impl Drop for PacketOwner {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.as_ref() {
            if let Some(packet) = self.payload.as_deref() {
                if storage
                    .begin_actual_packet_drop_on_control(fields(packet))
                    .is_err()
                {
                    storage.record_actual_packet_failure_on_control();
                }
            } else {
                storage.record_actual_packet_failure_on_control();
            }
        }
        // Pending is durable in the Root before this actual destructive take.
        drop(self.payload.take());
        if let Some(storage) = self.storage.as_ref() {
            let _completed = storage.complete_actual_packet_drop_on_control();
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    use hex_engine::AudioStorageError;
    fn value() -> ProjectPacket {
        ProjectPacket {
            selection: ProjectSelection::default(),
            sequence: 0,
            revision: 0,
            captured_at: Instant::now(),
            frames: 0,
            samples: zeroed_frames(PROJECT_PACKET_SAMPLES)
                .unwrap()
                .try_into()
                .unwrap(),
        }
    }
    #[test]
    fn actual_packet_box_and_samples_drop_normally_once_metadata_and_foreign_lineage_remain_distinct()
     {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let other = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let foreign =
            crate::native_callback_owner::NativeRetirement::begin_construction(other.clone());
        let owner = PacketOwner::new(value(), Some(&parent), 0).unwrap();
        let second = PacketOwner::new(value(), Some(&foreign), 0).unwrap();
        let bytes = owner.data_bytes().unwrap();
        assert_eq!(bytes, second.data_bytes().unwrap());
        assert_ne!(
            owner.payload.as_deref().unwrap() as *const ProjectPacket,
            second.payload.as_deref().unwrap() as *const ProjectPacket
        );
        let metadata = hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration::metadata_requested_bytes_on_control().unwrap();
        assert_eq!(
            root.test_spectrum_packet_facts().unwrap(),
            (1, 0, bytes + metadata)
        );
        let measured = allocation_counter::measure(|| {
            assert!(root.test_matches_spectrum_packet(owner.storage.as_ref().unwrap()));
            assert!(!root.test_matches_spectrum_packet(second.storage.as_ref().unwrap()));
            drop(owner);
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, -2, 0, -(bytes as i64))
        );
        assert_eq!(root.test_spectrum_packet_facts().unwrap(), (1, 1, metadata));
        drop(second);
        assert_eq!(
            other.test_spectrum_packet_facts().unwrap(),
            (1, 1, metadata)
        );
    }
    #[test]
    fn actual_pool_moves_same_owners_through_capture_forward_return_and_join_without_allocating() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let (mut capture, reader, mut worker) = channel_for_parent(48000, Some(&parent)).unwrap();
        let before = root.test_spectrum_packet_facts().unwrap();
        assert_eq!((before.0, before.1), (PROJECT_PACKET_OWNERS, 0));
        let actual =
            capture.packet.as_ref().unwrap().payload.as_deref().unwrap() as *const ProjectPacket;
        let selection = ProjectSelectionMailbox::default()
            .select(&[1], 7, 11, 48000)
            .unwrap();
        let now = Instant::now();
        let measured = allocation_counter::measure(|| {
            capture.begin(Some(selection), MAX_CAPTURE_FRAMES, true);
            capture.observe(ProjectSignal::Master, 0, &[[0.25; 2]; MAX_CAPTURE_FRAMES]);
            capture.observe(
                ProjectSignal::Track(1),
                0,
                &[[0.125; 2]; MAX_CAPTURE_FRAMES],
            );
            assert!(matches!(
                capture.finish(now, 11, true),
                Some(CaptureOutcome::Queued { .. })
            ));
            assert_eq!(worker.poll(now), 1);
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(root.test_spectrum_packet_facts().unwrap(), before);
        // Existing FIFO returns the same physical last packet after the thirty
        // two prefilled free packets; no owning payload is recreated.
        let mut recovered = None;
        for _ in 0..PROJECT_PACKET_OWNERS {
            let packet = capture.returned.pop().unwrap();
            if std::ptr::eq(packet.payload.as_deref().unwrap(), actual) {
                recovered = Some(packet);
            } else {
                capture
                    .producer
                    .push(packet)
                    .unwrap_or_else(|_| panic!("test FIFO capacity"));
            }
        }
        assert!(recovered.is_some());
        drop(recovered);
        drop(capture);
        drop(worker);
        let after = root.test_spectrum_packet_facts().unwrap();
        assert_eq!(
            (after.0, after.1),
            (PROJECT_PACKET_OWNERS, PROJECT_PACKET_OWNERS)
        );
        assert_eq!(after.2, PROJECT_PACKET_OWNERS * hex_engine::sampler_host_fields::NativeSpectrumPacketRegistration::metadata_requested_bytes_on_control().unwrap());
        drop(reader); // Result endpoint is independent of packet retirement.
        assert_eq!(root.test_spectrum_packet_facts().unwrap(), after);
    }
    #[test]
    fn omitted_packet_lease_and_interrupted_drop_stay_unknown_without_masking_first_panic() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let mut owner = PacketOwner::new(value(), Some(&parent), 0).unwrap();
        let original = owner.payload.as_deref().unwrap() as *const ProjectPacket;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            owner
                .storage
                .as_ref()
                .unwrap()
                .begin_actual_packet_drop_on_control(fields(owner.payload.as_deref().unwrap()))
                .unwrap();
            std::panic::panic_any(31850_u64);
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<u64>(), Some(&31850));
        assert_eq!(
            owner.payload.as_deref().unwrap() as *const ProjectPacket,
            original
        );
        assert_eq!(
            root.test_spectrum_packet_facts(),
            Err(AudioStorageError::UnknownStorage)
        );
        drop(owner.storage.take());
        assert_eq!(
            root.test_spectrum_packet_facts(),
            Err(AudioStorageError::UnknownStorage)
        );
        drop(owner);
        assert_eq!(
            root.test_spectrum_packet_facts(),
            Err(AudioStorageError::UnknownStorage)
        );
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "packet/carrier_binding_tests.rs"]
mod carrier_binding_tests;
