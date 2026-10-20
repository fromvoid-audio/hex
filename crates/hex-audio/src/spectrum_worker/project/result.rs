//! Exactly five constructor result buffers. Payload leases follow the SAME Box;
//! reader-home aliases recognize returns. No runtime Clone/allocation or owner
//! escape through the readonly frame query.
use super::*;
use hex_engine::sampler_host_fields::{
    BodyRef, NativeSpectrumResultFields, NativeSpectrumResultHome, NativeSpectrumResultLease,
};

/// Readonly nonClone snapshot. Recycle through Client refresh/clear while that
/// Client remains live; ordinary view teardown normally retires this exact Box.
/// ```compile_fail
/// fn duplicate(value: hex_audio::spectrum_worker::PublishedProjectSpectrum) { let _ = value.clone(); }
/// ```
/// ```compile_fail
/// fn escape(value: &hex_audio::spectrum_worker::PublishedProjectSpectrum) -> &'static [hex_audio::spectrum::SpectrumFrame] { value.frames() }
/// ```
pub struct PublishedProjectSpectrum {
    pub selection: ProjectSelection,
    pub applied_revision: u64,
    pub captured_at: Instant,
    pub processed_at: Instant,
    pub(super) frames: Option<Box<[SpectrumFrame; MAX_PROJECT_LAYERS]>>,
    pub(super) storage: Option<NativeSpectrumResultLease>,
    pub(super) index: usize,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) panic_on_drop: bool,
}
impl std::fmt::Debug for PublishedProjectSpectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublishedProjectSpectrum")
            .field("selection", &self.selection)
            .field("applied_revision", &self.applied_revision)
            .finish_non_exhaustive()
    }
}
impl PartialEq for PublishedProjectSpectrum {
    fn eq(&self, other: &Self) -> bool {
        self.selection == other.selection
            && self.applied_revision == other.applied_revision
            && self.captured_at == other.captured_at
            && self.processed_at == other.processed_at
            && self.frames == other.frames
    }
}
impl PublishedProjectSpectrum {
    /// Deliberately UNQUALIFIED drawing fixture. Not present in normal builds;
    /// cannot match any constructor home/Root or enter native admission.
    #[cfg(feature = "allocation-probe")]
    #[doc(hidden)]
    pub fn unqualified_for_ui_test(
        selection: ProjectSelection,
        applied_revision: u64,
        captured_at: Instant,
        processed_at: Instant,
        frames: Box<[SpectrumFrame; MAX_PROJECT_LAYERS]>,
    ) -> Self {
        Self {
            selection,
            applied_revision,
            captured_at,
            processed_at,
            frames: Some(frames),
            storage: None,
            index: usize::MAX,
            #[cfg(all(test, feature = "allocation-probe"))]
            panic_on_drop: false,
        }
    }
    pub fn frames(&self) -> &[SpectrumFrame] {
        self.frames
            .as_deref()
            .map_or(&[], |frames| &frames[..self.selection.layer_count()])
    }
    pub fn is_fresh_at(&self, now: Instant) -> bool {
        now.checked_duration_since(self.captured_at)
            .is_some_and(|age| age <= self.selection.stale_after())
    }
    pub(super) fn new(
        rate: u32,
        parent: Option<&crate::native_callback_owner::NativeRetirement>,
        index: usize,
    ) -> Result<(Self, NativeSpectrumResultHome), SpectrumWorkerError> {
        let mut value = Self {
            selection: ProjectSelection::default(),
            applied_revision: 0,
            captured_at: Instant::now(),
            processed_at: Instant::now(),
            frames: Some(project_frames(rate)?),
            storage: None,
            index,
            #[cfg(all(test, feature = "allocation-probe"))]
            panic_on_drop: false,
        };
        let (lease, home) = match parent {
            Some(parent) => parent
                .pair_spectrum_result_on_control(
                    index,
                    value.fields().ok_or(SpectrumWorkerError::StorageRefused)?,
                )
                .map_err(|_| SpectrumWorkerError::StorageRefused)?,
            None => {
                let (_record, lease, home) = hex_engine::sampler_host_fields::NativeSpectrumResultRegistration::pair_actual_result_on_control(value.fields().ok_or(SpectrumWorkerError::StorageRefused)?).map_err(|_| SpectrumWorkerError::StorageRefused)?;
                (lease, home)
            }
        };
        value.storage = Some(lease);
        Ok((value, home))
    }
    fn fields(&self) -> Option<NativeSpectrumResultFields<'_>> {
        let Self {
            selection: _,
            applied_revision: _,
            captured_at: _,
            processed_at: _,
            frames,
            storage: _,
            index: _,
            #[cfg(all(test, feature = "allocation-probe"))]
                panic_on_drop: _,
        } = self;
        let frames = frames.as_ref()?;
        for frame in frames.iter() {
            frame.validate_native_inline_fields();
        }
        Some(NativeSpectrumResultFields::of(
            BodyRef::of(self),
            BodyRef::of(frames),
            BodyRef::of(frames.as_ref()),
        ))
    }
    pub(super) fn belongs(&self, home: &NativeSpectrumResultHome) -> bool {
        self.storage
            .as_ref()
            .is_some_and(|lease| home.matches_actual_lease_on_control(lease))
    }
    pub(super) fn copy_from(&mut self, source: &Self) -> bool {
        let Some((to, from)) = self.frames.as_mut().zip(source.frames.as_ref()) else {
            return false;
        };
        to.clone_from_slice(from.as_ref());
        self.selection = source.selection;
        self.applied_revision = source.applied_revision;
        self.captured_at = source.captured_at;
        self.processed_at = source.processed_at;
        true
    }
}
impl Drop for PublishedProjectSpectrum {
    fn drop(&mut self) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let Some(fields) = self.fields() else {
            storage.record_actual_result_failure_on_control();
            return;
        };
        if storage.begin_actual_result_drop_on_control(fields).is_err() {
            return;
        }
        let payload = self.frames.take();
        #[cfg(all(test, feature = "allocation-probe"))]
        if self.panic_on_drop {
            std::panic::panic_any(31995u64);
        }
        drop(payload);
        if let Some(storage) = self.storage.as_ref() {
            let _completed = storage.complete_actual_result_drop_on_control();
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    fn publish(worker: &mut ProjectWorker, now: Instant, selection: ProjectSelection) {
        worker.core.selection = Some(selection);
        worker.core.filled = FFT_SIZE;
        worker.core.dirty = true;
        worker.core.captured_at = Some(now);
        worker.core.revision = selection.activation_floor;
        worker.core.rolling.fill([0.25; 2]);
        worker.poll(now);
    }
    #[test]
    fn actual_five_result_owners_refresh_clear_reopen_and_stale_recycle_without_alloc_or_free() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let (_capture, mut reader, mut worker) = channel_for_parent(48_000, Some(&parent)).unwrap();
        let start = Instant::now();
        let selection = ProjectSelection::new(&[1], 7, 1, 48_000).unwrap();
        let mut ui = None;
        let before = root.test_spectrum_result_facts().unwrap();
        assert_eq!((before.0, before.1), (5, 0));
        let measured = allocation_counter::measure(|| {
            for step in 0..32 {
                let now = start + Duration::from_millis(step * 500);
                publish(&mut worker, now, selection);
                assert_eq!(
                    reader.refresh(&mut ui, now, Some(selection)),
                    ProjectSpectrumRefresh::Updated
                );
                let address = ui.as_ref().unwrap().frames.as_ref().unwrap().as_ptr();
                assert_eq!(
                    reader.refresh(&mut ui, now, Some(selection)),
                    ProjectSpectrumRefresh::Unchanged
                );
                assert_eq!(
                    ui.as_ref().unwrap().frames.as_ref().unwrap().as_ptr(),
                    address
                );
                assert_eq!(
                    reader.refresh(&mut ui, now, None),
                    ProjectSpectrumRefresh::Cleared
                );
                assert!(ui.is_none());
                assert_eq!(reader.ui_free.iter().flatten().count(), 2);
            }
            let now = start + Duration::from_secs(30);
            publish(&mut worker, now, selection);
            assert_eq!(
                reader.refresh(&mut ui, now, Some(selection)),
                ProjectSpectrumRefresh::Updated
            );
            reader.refresh(&mut ui, now + Duration::from_secs(1), Some(selection));
            assert!(
                ui.is_none(),
                "stale owner recycled, never displayed through Busy"
            );
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
        assert_eq!(root.test_spectrum_result_facts().unwrap(), before);
        let now = start + Duration::from_secs(40);
        publish(&mut worker, now, selection);
        assert_eq!(
            reader.refresh(&mut ui, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        drop(worker);
        drop(reader);
        assert_eq!(
            root.test_spectrum_result_facts().unwrap().1,
            4,
            "one actual UI Box outlives worker/reader"
        );
        drop(ui);
        let after = root.test_spectrum_result_facts().unwrap();
        assert_eq!((after.0, after.1), (5, 5));
    }
    #[test]
    fn actual_foreign_ui_owner_retires_against_old_root_without_entering_new_pool() {
        let root_a = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent_a =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_a.clone());
        let (_ca, mut ra, mut wa) = channel_for_parent(48_000, Some(&parent_a)).unwrap();
        let root_b = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent_b =
            crate::native_callback_owner::NativeRetirement::begin_construction(root_b.clone());
        let (_cb, mut rb, mut wb) = channel_for_parent(48_000, Some(&parent_b)).unwrap();
        let now = Instant::now();
        let selection = ProjectSelection::new(&[], 1, 1, 48_000).unwrap();
        let mut ui = None;
        publish(&mut wa, now, selection);
        ra.refresh(&mut ui, now, Some(selection));
        let old = ui.as_ref().unwrap().frames.as_ref().unwrap().as_ptr();
        drop((wa, ra));
        publish(&mut wb, now, selection);
        assert_eq!(
            rb.refresh(&mut ui, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        assert_ne!(ui.as_ref().unwrap().frames.as_ref().unwrap().as_ptr(), old);
        assert_eq!(root_a.test_spectrum_result_facts().unwrap().1, 5);
        rb.refresh(&mut ui, now, None);
        assert_eq!(rb.ui_free.iter().flatten().count(), 2);
    }
    #[test]
    fn equal_timestamp_metadata_with_changed_actual_frames_is_updated_without_new_owner() {
        let (_capture, mut reader, mut worker) = channel(48_000).unwrap();
        let now = Instant::now();
        let selection = ProjectSelection::new(&[], 1, 1, 48_000).unwrap();
        let mut ui = None;
        publish(&mut worker, now, selection);
        assert_eq!(
            reader.refresh(&mut ui, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        worker.core.analyzed_at = None;
        publish(&mut worker, now, selection);
        reader.poll_latest(now, Some(selection));
        let latest = reader.latest.as_mut().unwrap();
        latest.processed_at = ui.as_ref().unwrap().processed_at;
        latest.frames.as_mut().unwrap()[0].bins_dbfs.fill(-12.0);
        assert_eq!(
            reader.refresh(&mut ui, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        assert!(
            ui.as_ref().unwrap().frames()[0]
                .bins_dbfs
                .iter()
                .all(|x| *x == -12.0)
        );
    }

    #[test]
    fn two_external_ui_owners_backpressure_and_generation_clear_recovers_same_live_pool() {
        let (_capture, mut reader, mut worker) = channel(48_000).unwrap();
        let now = Instant::now();
        let selection = ProjectSelection::new(&[], 1, 1, 48_000).unwrap();
        let (mut a, mut b, mut c) = (None, None, None);
        publish(&mut worker, now, selection);
        assert_eq!(
            reader.refresh(&mut a, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        assert_eq!(
            reader.refresh(&mut b, now, Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        assert_eq!(
            reader.refresh(&mut c, now, Some(selection)),
            ProjectSpectrumRefresh::Busy
        );
        assert!(c.is_none());
        reader.refresh(&mut a, now, None);
        assert!(a.is_none());
        publish(&mut worker, now + Duration::from_millis(500), selection);
        assert_eq!(
            reader.refresh(&mut c, now + Duration::from_millis(500), Some(selection)),
            ProjectSpectrumRefresh::Updated
        );
        let next = ProjectSelection::new(&[], 2, 1, 48_000).unwrap();
        reader.refresh(&mut b, now + Duration::from_millis(500), Some(next));
        assert!(
            b.is_none(),
            "Busy must not preserve a displayable foreign generation"
        );
        reader.refresh(&mut c, now, None);
        assert_eq!(reader.ui_free.iter().flatten().count(), 2);
    }
    #[test]
    fn actual_result_destructor_unwind_keeps_first_payload_and_sticky_consumed_custody() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let (_capture, reader, mut worker) = channel_for_parent(48_000, Some(&parent)).unwrap();
        worker.core.free_results[0].as_mut().unwrap().panic_on_drop = true;
        let failure =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(worker))).unwrap_err();
        assert_eq!(failure.downcast_ref::<u64>(), Some(&31995));
        for _ in 0..2 {
            assert_eq!(
                root.test_spectrum_result_facts(),
                Err(hex_engine::AudioStorageError::UnknownStorage)
            );
        }
        drop(reader);
        assert_eq!(
            root.test_spectrum_result_facts(),
            Err(hex_engine::AudioStorageError::UnknownStorage)
        );
    }
}
