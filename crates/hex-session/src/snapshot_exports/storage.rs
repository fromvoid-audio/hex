//! Actual retained record inventory. No owning root escapes the family guard;
//! enclosing holder/table, other roots, peak and publication remain separate.
use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageError};

pub(super) fn export_result_fields<'a, T>(
    body: &'a T,
    event: Option<&'a crate::service::ExportEvent>,
) -> hex_engine::sampler_host_fields::FamilyExportResultFields<'a> {
    use hex_engine::sampler_host_fields::{BodyRef, FamilyExportResultFields};
    FamilyExportResultFields {
        body: BodyRef::of(body),
        payload: event.map(crate::service::ExportEvent::sampler_fields_on_control),
    }
}

impl crate::service::ExportEvent {
    pub(crate) fn sampler_fields_on_control(
        &self,
    ) -> hex_engine::sampler_host_fields::ExportResultPayloadFields<'_> {
        use hex_engine::sampler_host_fields::{
            BodyRef, ExportResultOutcomeFields, ExportResultPayloadFields,
        };

        let crate::service::ExportEvent {
            revision: _,
            path,
            result,
        } = self;
        let outcome = match result {
            Ok(report) => {
                let hex_export::ExportReport {
                    path,
                    sample_rate: _,
                    frames: _,
                    data_bytes: _,
                    recovery_path,
                } = report;
                ExportResultOutcomeFields::Success {
                    body: BodyRef::of(report),
                    path,
                    recovery: recovery_path.as_ref(),
                }
            }
            Err(message) => ExportResultOutcomeFields::Error {
                body: BodyRef::of(message),
                message,
            },
        };
        ExportResultPayloadFields {
            body: BodyRef::of(self),
            path,
            outcome,
        }
    }
}

impl ViewState {
    /// Actual children of this view, including every paired physical field.
    /// The enclosing inline/shared view body is charged by its actual parent.
    /// Unknown nested storage refuses without declaring the view admitted.
    pub fn visit_children_selected_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        ledger.borrow_sampler_snapshot_children_fields_on_control(self)
    }
}
// Actual fields under the existing family/worker borrow. This helper is
// exhaustive on every owning ViewState field and allocates no projection list.
struct ViewParts<'a> {
    comparison: &'a Option<crate::service::ComparisonAudioBinding>,
    source: &'a Option<crate::service::SharedCanonicalAudioSource>,
    project: &'a crate::SharedProject,
    waveforms: &'a hex_project::SortedEntries<hex_project::AssetId, crate::SharedWaveform>,
    clips: &'a hex_project::SortedEntries<hex_project::ClipId, crate::service::AudioClipWaveform>,
    midi: &'a hex_engine::SharedMidiArrangementPreview,
    gain: &'a Result<hex_engine::SharedGainAutomationPreview, hex_engine::Invalid>,
    path: &'a Option<std::path::PathBuf>,
}
fn view_parts(view: &ViewState) -> ViewParts<'_> {
    let ViewState {
        canonical_audio_source,
        comparison_audio,
        project,
        waveforms,
        audio_clip_waveforms,
        midi_preview,
        gain_preview,
        timeline_frames: _,
        can_undo: _,
        can_redo: _,
        revision: _,
        unsaved: _,
        document_path,
    } = view;
    ViewParts {
        comparison: comparison_audio,
        source: canonical_audio_source,
        project,
        waveforms,
        clips: audio_clip_waveforms,
        midi: midi_preview,
        gain: gain_preview,
        path: document_path,
    }
}
impl hex_engine::sampler_host_fields::SnapshotViewPayload for ViewState {
    fn revision_field(&self) -> &u64 {
        &self.revision
    }
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self)
    }
    fn source(&self) -> Option<hex_engine::sampler_host_fields::CapturedSourceFields<'_>> {
        view_parts(self)
            .source
            .as_ref()
            .map(crate::service::canonical_audio_source::sampler_fields_on_control)
    }
    fn comparison_source(
        &self,
    ) -> Option<hex_engine::sampler_host_fields::CapturedSourceFields<'_>> {
        view_parts(self)
            .comparison
            .as_ref()
            .map(|binding| binding.sampler_source_fields_on_control())
    }
    fn project(&self) -> &crate::SharedProject {
        view_parts(self).project
    }
    fn waveforms(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        hex_engine::sampler_host_fields::VecRef::entries(view_parts(self).waveforms)
    }
    fn waveform(
        &self,
        index: usize,
    ) -> Result<hex_engine::sampler_host_fields::WaveformFields<'_>, AudioStorageError> {
        let (_, waveform) = view_parts(self)
            .waveforms
            .as_slice()
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(hex_media::waveform::sampler_fields_on_control(waveform))
    }
    fn clip_waveforms(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        hex_engine::sampler_host_fields::VecRef::entries(view_parts(self).clips)
    }
    fn clip_waveform(
        &self,
        index: usize,
    ) -> Result<hex_engine::sampler_host_fields::WaveformFields<'_>, AudioStorageError> {
        let (
            _,
            crate::service::AudioClipWaveform {
                waveform,
                offset: _,
                length: _,
            },
        ) = view_parts(self)
            .clips
            .as_slice()
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(hex_media::waveform::sampler_fields_on_control(waveform))
    }
    fn midi_preview(&self) -> &hex_engine::SharedMidiArrangementPreview {
        view_parts(self).midi
    }
    fn gain_preview(&self) -> Option<&hex_engine::SharedGainAutomationPreview> {
        // Invalid stores only its existing static diagnostic; no owning child.
        view_parts(self).gain.as_ref().ok()
    }
    fn document_path(&self) -> Option<&std::path::PathBuf> {
        view_parts(self).path.as_ref()
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
        ledger.borrow_sampler_family_fields_on_control(self)
    }
}
impl hex_engine::sampler_host_fields::FamilyFields for SnapshotExportRecords {
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self)
    }
    fn snapshots(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        let Self {
            family: _,
            revision: _,
            closed: _,
            snapshots,
            reads: _,
            native_compiled: _,
            native_requests: _,
            native_export_result: _,
            source_events: _,
            max_bookkeeping_bytes: _,
        } = self;
        hex_engine::sampler_host_fields::VecRef::of(snapshots)
    }
    fn snapshot(
        &self,
        index: usize,
    ) -> Result<hex_engine::sampler_host_fields::FamilySnapshotFields<'_>, AudioStorageError> {
        use hex_engine::sampler_host_fields::{
            BodyRef, FamilySnapshotFields, SharedMetadataRef, SnapshotViewFields,
        };
        let record = self
            .snapshots
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let SnapshotRecord {
            generation: _,
            snapshot,
            event_metadata,
            retired: _,
            readers: _,
            cleanup_started,
        } = record;
        if *cleanup_started || (snapshot.is_none() && event_metadata.is_some()) {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(FamilySnapshotFields {
            body: BodyRef::of(record),
            event_metadata: event_metadata.as_ref().map(|metadata| {
                hex_engine::sampler_host_fields::FamilyEventMetadataFields {
                    owner: SharedMetadataRef::of(metadata),
                    metadata: metadata.fields(),
                }
            }),
            view: snapshot.as_ref().map(|owner| SnapshotViewFields {
                owner: SharedMetadataRef::of(owner),
                payload: owner.as_ref(),
            }),
        })
    }
    fn reads(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        for ReadRecord {
            generation: _,
            snapshot_index: _,
            snapshot_generation: _,
            occupied: _,
            released: _,
        } in &self.reads
        {}
        hex_engine::sampler_host_fields::VecRef::of(&self.reads)
    }
    fn requests(&self) -> &dyn hex_engine::sampler_host_fields::FamilyRequestRecords {
        &self.native_requests
    }
    fn export_result(
        &self,
    ) -> Result<hex_engine::sampler_host_fields::FamilyExportResultFields<'_>, AudioStorageError>
    {
        self.native_export_result.fields()
    }
    fn compiled(
        &self,
    ) -> Result<Option<hex_engine::sampler_host_fields::FamilyCompiledFields<'_>>, AudioStorageError>
    {
        self.validate_source_events_on_control()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        self.native_compiled
            .as_ref()
            .map(|records| records.fields())
            .transpose()
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
    fn typed_snapshot_view_refuses_foreign_payload_source_project_maps_previews_and_path() {
        use hex_engine::sampler_host_fields::{
            BodyRef, CapturedSourceFields, SharedMetadataRef, SnapshotViewFields,
            SnapshotViewPayload, VecRef, WaveformFields,
        };
        let actual = hex_engine::SharedMetadata::new_on_control(pending(7).unwrap());
        let mut foreign_value = pending(8).unwrap();
        foreign_value.document_path = Some(std::path::PathBuf::from(
            "/disposable/not-opened/foreign.hex",
        ));
        let foreign = hex_engine::SharedMetadata::new_on_control(foreign_value);
        assert!(actual.gain_preview().is_some() && foreign.gain_preview().is_some());
        let mut positive = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        positive
            .borrow_sampler_snapshot_view_fields_on_control(&SnapshotViewFields {
                owner: SharedMetadataRef::of(&actual),
                payload: actual.as_ref(),
            })
            .unwrap();
        positive
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(positive.summary().unwrap().require_complete().is_err());
        struct Wrong<'a> {
            actual: &'a ViewState,
            foreign: &'a ViewState,
            kind: u8,
        }
        impl SnapshotViewPayload for Wrong<'_> {
            fn revision_field(&self) -> &u64 {
                self.actual.revision_field()
            }
            fn body(&self) -> BodyRef<'_> {
                self.actual.body()
            }
            fn source(&self) -> Option<CapturedSourceFields<'_>> {
                if self.kind == 0 {
                    self.foreign.source()
                } else {
                    self.actual.source()
                }
            }
            fn comparison_source(&self) -> Option<CapturedSourceFields<'_>> {
                self.actual.comparison_source()
            }
            fn project(&self) -> &crate::SharedProject {
                if self.kind == 1 {
                    self.foreign.project()
                } else {
                    self.actual.project()
                }
            }
            fn waveforms(&self) -> VecRef<'_> {
                if self.kind == 2 {
                    self.foreign.waveforms()
                } else {
                    self.actual.waveforms()
                }
            }
            fn waveform(&self, index: usize) -> Result<WaveformFields<'_>, AudioStorageError> {
                self.actual.waveform(index)
            }
            fn clip_waveforms(&self) -> VecRef<'_> {
                if self.kind == 3 {
                    self.foreign.clip_waveforms()
                } else {
                    self.actual.clip_waveforms()
                }
            }
            fn clip_waveform(&self, index: usize) -> Result<WaveformFields<'_>, AudioStorageError> {
                self.actual.clip_waveform(index)
            }
            fn midi_preview(&self) -> &hex_engine::SharedMidiArrangementPreview {
                if self.kind == 4 {
                    self.foreign.midi_preview()
                } else {
                    self.actual.midi_preview()
                }
            }
            fn gain_preview(&self) -> Option<&hex_engine::SharedGainAutomationPreview> {
                if self.kind == 5 {
                    self.foreign.gain_preview()
                } else {
                    self.actual.gain_preview()
                }
            }
            fn document_path(&self) -> Option<&std::path::PathBuf> {
                if self.kind == 6 {
                    self.foreign.document_path()
                } else {
                    self.actual.document_path()
                }
            }
        }
        for kind in 0..7 {
            let wrong = Wrong {
                actual: &actual,
                foreign: &foreign,
                kind,
            };
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
            assert_eq!(
                ledger.borrow_sampler_snapshot_view_fields_on_control(&SnapshotViewFields {
                    owner: SharedMetadataRef::of(&actual),
                    payload: &wrong,
                }),
                Err(AudioStorageError::SourceMismatch),
                "foreign field kind {kind}"
            );
            assert!(ledger.summary().is_err());
        }
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
        assert_eq!(
            ledger.borrow_sampler_snapshot_view_fields_on_control(&SnapshotViewFields {
                owner: SharedMetadataRef::of(&actual),
                payload: foreign.as_ref(),
            }),
            Err(AudioStorageError::SourceMismatch)
        );
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
