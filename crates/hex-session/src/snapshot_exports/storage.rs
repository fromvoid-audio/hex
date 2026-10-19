//! Actual retained record inventory. No owning root escapes the family guard;
//! enclosing holder/table, other roots, peak and publication remain separate.
use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageError};

impl ViewState {
    /// Actual children of this view, including every paired physical field.
    /// The enclosing inline/shared view body is charged by its actual parent.
    /// Unknown nested storage refuses without declaring the view admitted.
    pub fn visit_children_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let ViewState {
            canonical_audio_source,
            project,
            waveforms: _,
            audio_clip_waveforms: _,
            midi_preview,
            gain_preview: _,
            timeline_frames,
            can_undo,
            can_redo,
            revision,
            unsaved,
            document_path,
        } = self;
        let _: (&u64, &bool, &bool, &u64, &bool) =
            (timeline_frames, can_undo, can_redo, revision, unsaved);
        if let Some(source) = canonical_audio_source {
            crate::service::visit_canonical_source_selected_storage_on_control(source, ledger)?;
        }
        crate::visit_shared_project_selected_storage_on_control(project, ledger)?;
        self.visit_waveform_storage_on_control(ledger)?;
        hex_engine::visit_shared_midi_preview_storage_on_control(midi_preview, ledger)?;
        self.visit_gain_preview_storage_on_control(ledger)?;
        if let Some(path) = document_path {
            ledger.borrow_metadata_path_backing_on_control(path)?;
        }
        Ok(())
    }
}
impl SnapshotExportRecords {
    /// Both actual table backings and ALL occupied snapshots, including retired,
    /// unpainted, pending-read and cleanup-latched records. Released read counts
    /// do not erase still-rooted owners. Snapshot fields are visited only at
    /// their actual physical location, never reconstructed from read IDs.
    pub fn visit_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let SnapshotExportRecords {
            family,
            revision,
            closed,
            snapshots,
            reads,
            native_compiled,
        } = self;
        let _: (&u64, &u64, &bool) = (family, revision, closed);
        self.visit_backings_on_control(ledger)?;
        if let Some(records) = native_compiled {
            records.visit(ledger)?;
        }
        for SnapshotRecord {
            generation,
            snapshot,
            retired,
            readers,
            cleanup_started,
        } in snapshots
        {
            let _: (&u64, &bool, &AtomicUsize, &bool) =
                (generation, retired, readers, cleanup_started);
            if let Some(view) = snapshot {
                ledger.borrow_shared_metadata_body_on_control(view)?;
                view.visit_children_selected_storage_on_control(ledger)?;
            }
        }
        for ReadRecord {
            generation,
            snapshot_index,
            snapshot_generation,
            occupied,
            released,
        } in reads
        {
            let _: (
                &AtomicU64,
                &AtomicUsize,
                &AtomicU64,
                &AtomicBool,
                &AtomicBool,
            ) = (
                generation,
                snapshot_index,
                snapshot_generation,
                occupied,
                released,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> SnapshotExportReservation {
        SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        }
    }
    fn budget() -> hex_engine::AudioStorageLedgerReservation {
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        }
    }
    fn pending(revision: u64) -> Option<ViewState> {
        let session = crate::Session::new(48_000, 8, 1024).unwrap();
        Some(crate::service::view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            revision,
            1,
            false,
            None,
        ))
    }
    fn summary(
        records: &SnapshotExportRecords,
        budget: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<hex_engine::AudioRootStorageSummary, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
        records.visit_selected_storage_on_control(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        ledger.summary()
    }
    #[test]
    fn retained_snapshot_inventory_counts_old_new_unpainted_records_and_exact_budgets() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported actual snapshot child constructors; no positive proof");
            return;
        }
        eprintln!(
            "SUPPORTED actual retained canonical snapshot children and independent whole-record omission"
        );
        let mut records = SnapshotExportRecords::try_new_on_control(limits()).unwrap();
        let empty = summary(&records, budget()).unwrap();
        let mut first = pending(7);
        let old = records.insert_on_control(&mut first).unwrap();
        let read = records.reserve_read(old).unwrap();
        let one = summary(&records, budget()).unwrap();
        assert!(one.data_bytes > empty.data_bytes);
        let mut second = pending(8);
        let new = records.insert_on_control(&mut second).unwrap();
        let two = summary(&records, budget()).unwrap();
        // Independently created equal empty views have separate outer/child
        // allocations. Omitting an ENTIRE record cannot hide behind count closure.
        assert_eq!(
            two.data_bytes - empty.data_bytes,
            2 * (one.data_bytes - empty.data_bytes)
        );
        assert_eq!(
            two.data_allocations - empty.data_allocations,
            2 * (one.data_allocations - empty.data_allocations)
        );
        let mut exact = budget();
        exact.max_retained_bytes = two.retained_bytes;
        exact.max_construction_peak_bytes = two.construction_peak_bytes;
        assert_eq!(summary(&records, exact).unwrap(), two);
        exact.max_retained_bytes -= 1;
        assert!(summary(&records, exact).is_err());
        let mut refused = pending(9);
        let before = std::ptr::from_ref(&refused.as_ref().unwrap().project);
        assert_eq!(
            records.insert_on_control(&mut refused),
            Err(ExportRecordError::Reservation)
        );
        assert_eq!(
            before,
            std::ptr::from_ref(&refused.as_ref().unwrap().project)
        );
        records.retire_on_control(old).unwrap();
        records.retire_on_control(new).unwrap();
        assert_eq!(summary(&records, budget()).unwrap(), two);
        records.release_read(read).unwrap();
        records.collect_releases_on_control().unwrap();
        assert_eq!(
            summary(&records, budget()).unwrap(),
            two,
            "durable ACK is not owning cleanup or a byte release"
        );
        // Another live pending source is traversed at its actual inline parent;
        // no outer shared body is invented for its Option<ViewState>.
        let mut joined = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        records
            .visit_selected_storage_on_control(&mut joined)
            .unwrap();
        refused
            .as_ref()
            .unwrap()
            .visit_children_selected_storage_on_control(&mut joined)
            .unwrap();
        joined
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(joined.summary().unwrap().data_bytes > two.data_bytes);
        assert!(joined.summary().unwrap().require_complete().is_err());
    }
    #[test]
    fn guarded_snapshot_inventory_keeps_retired_children_until_actual_control_cleanup() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported actual snapshot child constructors; no positive proof");
            return;
        }
        eprintln!("SUPPORTED guarded actual family inventory and control-only child retirement");
        let (mut family, producer) = SessionExportFamily::try_new_on_control(limits()).unwrap();
        let measure = |family: &SessionExportFamily| {
            family
                .inspect_queries_on_control(|queries| {
                    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
                    queries
                        .visit_selected_storage_on_control(&mut ledger)
                        .unwrap();
                    ledger
                        .validate_selected_shared_handles_on_control()
                        .unwrap();
                    ledger.summary().unwrap()
                })
                .unwrap()
        };
        let empty = measure(&family);
        let mut source = pending(13);
        let stamp = family.insert_on_control(&producer, &mut source).unwrap();
        let read = family.reserve_read_on_control(&producer, stamp).unwrap();
        let occupied = measure(&family);
        family.abandon_on_control(&producer).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Pending)
        );
        assert_eq!(measure(&family), occupied);
        assert_eq!(family.read_with(&read, |view| view.revision()), Ok(13));
        family.release_read(read).unwrap();
        assert_eq!(measure(&family), occupied);
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(SnapshotFamilyCleanup::Disposed)
        );
        assert_eq!(measure(&family), empty);
    }
    #[test]
    fn snapshot_known_outer_refuses_unknown_midi_child_and_charges_reserved_document_path() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported actual snapshot child constructors; no positive proof");
            return;
        }
        eprintln!(
            "SUPPORTED nested snapshot Unknown refusal and actual reserved Unix path capacity"
        );
        let mut source = pending(1).unwrap();
        let mut path = std::path::PathBuf::with_capacity(311);
        path.push("original.hex");
        path.clear();
        assert!(path.capacity() >= 311);
        source.document_path = Some(path);
        let owner = SharedMetadata::new_on_control(source);
        let measure = |with_children| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
            ledger
                .borrow_shared_metadata_body_on_control(&owner)
                .unwrap();
            if with_children {
                owner
                    .visit_children_selected_storage_on_control(&mut ledger)
                    .unwrap();
            }
            ledger.summary().unwrap()
        };
        let body = measure(false);
        let children = measure(true);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&owner)
            .unwrap();
        let path = owner.document_path.as_ref().unwrap();
        ledger
            .borrow_metadata_path_backing_on_control(path)
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            body.data_bytes + path.capacity()
        );
        ledger
            .borrow_metadata_path_backing_on_control(path)
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            body.data_bytes + path.capacity()
        );
        assert!(children.data_bytes > body.data_bytes + path.capacity());
        let mut unknown = pending(2).unwrap();
        unknown.midi_preview =
            hex_engine::SharedMidiArrangementPreview::new_on_control(Default::default());
        let unknown = SharedMetadata::new_on_control(unknown);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&unknown)
            .unwrap();
        assert!(
            unknown
                .visit_children_selected_storage_on_control(&mut ledger)
                .is_err()
        );
    }
}
