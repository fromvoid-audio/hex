//! Fresh same-worker dispatch borrows. No owner moves, clone or stored permit.
use super::*;
use crate::snapshot_exports::SnapshotWorkerControl;

pub(super) type KeyboardActivation = Option<(
    u64,
    u64,
    hex_project::TrackId,
    Arc<hex_engine::computer_keyboard::ComputerKeyboardInput>,
)>;

// Private dispatch INPUTS, not a public inspection/admission scope. No guard or
// ledger is stored here. A fresh WorkerRootView is built for EACH synchronous
// audio call, ending before try_send, receive/sleep or the next model mutation.
pub(super) struct WorkerDispatchState<'a> {
    pub session: &'a Session,
    pub current: &'a Arc<ViewState>,
    pub compiled: &'a Option<(u64, Arc<PreparedRevision>)>,
    pub midi_preview: &'a hex_engine::SharedMidiArrangementPreview,
    pub generation: u64,
    pub revision: u64,
    pub control: Option<&'a SnapshotWorkerControl>,
    pub clipboard: &'a Option<ArrangementClipboard>,
    pub media_base: &'a Option<PathBuf>,
    pub recovery: &'a Option<PathBuf>,
    pub document: &'a Option<DocumentHandle>,
    pub journal: &'a Option<hex_storage::Journal>,
    pub keyboard: &'a KeyboardActivation,
    pub keyboard_hint: &'a Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>,
    pub audition: &'a Option<Arc<hex_engine::tuning_audition::TuningAuditionInput>>,
    pub expression: &'a Option<Arc<hex_engine::live_expression::LiveExpressionInput>>,
    pub device_gesture: &'a GestureMachine,
    pub tuning_gesture: &'a TuningGestureMachine,
    pub export_job: Option<&'a ExportJob>,
    pub export_completion: Option<&'a ExportEvent>,
}

/// Which actual synchronous dispatch is being observed; never an admission flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerRootDispatch {
    ChangedEvent,
    PendingEvent,
    Idle,
}

/// Sealed borrow of ACTUAL same-worker roots, valid only for this audio call.
/// No Session, Project, source, Arc, waveform or PCM can be extracted. It is not
/// Clone and provides no saved permit or completeness flag. Opaque channels,
/// legacy parent allocations and unvisited children stay Unknown.
/// ```compile_fail
/// let _: hex_session::service::WorkerRootView<'static> =
///     hex_session::service::WorkerRootView {};
/// ```
/// ```compile_fail
/// fn save<'a>(root: &'a hex_session::service::WorkerRootView<'a>)
///     -> &'static hex_session::service::WorkerRootView<'static> { root }
/// ```
/// ```compile_fail
/// fn escape(root: &hex_session::service::WorkerRootView<'_>) {
///     let _ = root.with_original_child_inventory_on_control(|inventory| Ok(inventory));
/// }
/// ```
pub struct WorkerRootView<'a> {
    state: &'a WorkerDispatchState<'a>,
    cancellation: &'a SessionCancellation,
    event: Option<&'a Event>,
    dispatch: WorkerRootDispatch,
}
impl<'a> WorkerRootView<'a> {
    pub(super) fn new(
        state: &'a WorkerDispatchState<'a>,
        cancellation: &'a SessionCancellation,
        event: Option<&'a Event>,
        dispatch: WorkerRootDispatch,
    ) -> Self {
        Self {
            state,
            cancellation,
            event,
            dispatch,
        }
    }
    pub fn dispatch(&self) -> WorkerRootDispatch {
        self.dispatch
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        hex_engine::AudioSourceContext {
            project_generation: self.state.generation,
            editor_revision: self.state.revision,
        }
    }
    pub fn pending_event_revision(&self) -> Option<u64> {
        self.event.map(|event| event.state.revision)
    }
    pub fn has_pending_export(&self) -> bool {
        self.state.export_job.is_some() || self.state.export_completion.is_some()
    }
    /// Actual source correlation only, NOT PCM provenance or an admission seal.
    /// Old async snapshots cannot stand in for this dispatch's current source.
    pub fn matches_current_source_on_control(&self, source: &SharedCanonicalAudioSource) -> bool {
        source.context() == self.context()
            && source.sample_rate() == self.state.session.project().sample_rate()
            && self
                .state
                .current
                .canonical_audio_source
                .as_ref()
                .is_some_and(|current| current.shares_owner_with(source))
    }
    /// Acquire actual Session/cache guards AFTER the caller's outer/inner family
    /// guards, BEFORE the ledger and native barrier. All guards/borrows must stay
    /// through final recheck+commit; no result of this callback is a saved permit.
    pub fn with_original_child_inventory_on_control<T>(
        &self,
        inspect: impl for<'scope> FnOnce(
            &WorkerRootChildInventory<'scope>,
        ) -> Result<T, hex_engine::AudioStorageError>,
    ) -> Result<T, hex_engine::AudioStorageError> {
        self.state
            .session
            .with_resident_pcm_inventory_on_control(|resident| {
                inspect(&WorkerRootChildInventory {
                    roots: self,
                    resident,
                })
            })
    }
}

/// Audited selected children of actual dispatch fields. Enclosing/opaque parents
/// and incomplete prepared/gesture/input/queue inventory still REFUSE closure.
pub struct WorkerRootChildInventory<'a> {
    roots: &'a WorkerRootView<'a>,
    resident: &'a crate::ResidentPcmInventory<'a>,
}
impl WorkerRootChildInventory<'_> {
    pub fn visit_selected_original_children_on_control<'a>(
        &'a self,
        ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.resident
            .visit_original_only_worker_children_on_control(ledger)?;
        self.roots.cancellation.visit(ledger)?;
        if let Some(control) = self.roots.state.control {
            control.visit_cancellation_storage_on_control(ledger)?;
        }
        self.roots
            .state
            .current
            .visit_children_selected_storage_on_control(ledger)?;
        hex_engine::visit_shared_midi_preview_storage_on_control(
            self.roots.state.midi_preview,
            ledger,
        )?;
        if let Some(event) = self.roots.event {
            event
                .state
                .visit_children_selected_storage_on_control(ledger)?;
            if let Some(preview) = &event.device_preview {
                preview
                    .state
                    .visit_children_selected_storage_on_control(ledger)?;
            }
            if let Some(preview) = &event.tuning_preview {
                preview
                    .state
                    .visit_children_selected_storage_on_control(ledger)?;
            }
        }
        for path in [self.roots.state.media_base, self.roots.state.recovery]
            .into_iter()
            .flatten()
        {
            ledger.borrow_metadata_path_backing_on_control(path)?;
        }
        if let Some(job) = self.roots.state.export_job {
            ledger.borrow_metadata_path_backing_on_control(&job.path)?;
        }
        if let Some(completion) = self.roots.state.export_completion {
            ledger.borrow_metadata_path_backing_on_control(&completion.path)?;
        }
        Ok(())
    }
    /// Explicitly refuse unqualified physical parents, prepared/gesture/input
    /// roots and transferred channel custody. Selected child totals cannot open
    /// processing. Each actual field is retained in the view, not reconstructed.
    pub fn require_complete_root_storage_on_control(
        &self,
    ) -> Result<(), hex_engine::AudioStorageError> {
        let WorkerDispatchState {
            session: _,
            current: _,
            compiled,
            midi_preview: _,
            generation: _,
            revision: _,
            control: _,
            clipboard,
            media_base: _,
            recovery: _,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job,
            export_completion,
        } = self.roots.state;
        // Borrow-only marker of the ACTUAL currently unsupported fields. No
        // len/count/type/content heuristic manufactures a storage declaration.
        let _ = (
            compiled,
            clipboard,
            document,
            journal,
            keyboard,
            keyboard_hint,
            audition,
            expression,
            device_gesture,
            tuning_gesture,
            export_job,
            export_completion,
        );
        Err(hex_engine::AudioStorageError::UnknownStorage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
    fn budget() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        }
    }
    #[test]
    fn actual_worker_root_children_detect_omitted_midi_field_and_pending_export_path_capacity() {
        if !hex_engine::audio_storage_constructor_layout_supported()
            || !hex_engine::canonical_midi_storage_layout_supported()
            || !hex_engine::canonical_native_state_storage_layout_supported()
        {
            eprintln!("unsupported root child constructors; no positive root storage proof");
            return;
        }
        eprintln!(
            "SUPPORTED actual worker root Session/view/MIDI/cancellation and pending export path child union"
        );
        let signal =
            hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = hex_engine::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(hex_engine::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let session = Session::new_with_retained_cancellation_on_control(
            48_000,
            8,
            0,
            signal.bind_worker_on_control(job.route()),
        )
        .unwrap();
        drop(session.compile_prepared_revision(None).unwrap());
        let cancellation =
            SessionCancellation::Retained(signal.bind_worker_on_control(job.route()));
        let midi = hex_engine::empty_midi_preview_on_control();
        let current = view_state(&session, midi.clone(), 7, 11, false, None);
        let gesture = GestureMachine::default();
        let tuning = TuningGestureMachine::default();
        let mut path = PathBuf::with_capacity(256);
        path.push("pending-export.wav");
        assert!(path.capacity() > path.as_os_str().len());
        let completion = ExportEvent {
            revision: 7,
            path,
            result: Err("nothing to export".into()),
        };
        let state = WorkerDispatchState {
            session: &session,
            current: &current,
            compiled: &None,
            midi_preview: &midi,
            generation: 11,
            revision: 7,
            control: None,
            clipboard: &None,
            media_base: &None,
            recovery: &None,
            document: &None,
            journal: &None,
            keyboard: &None,
            keyboard_hint: &None,
            audition: &None,
            expression: &None,
            device_gesture: &gesture,
            tuning_gesture: &tuning,
            export_job: None,
            export_completion: Some(&completion),
        };
        let roots = WorkerRootView::new(&state, &cancellation, None, WorkerRootDispatch::Idle);
        assert!(roots.has_pending_export());
        assert_eq!(
            roots.context(),
            hex_engine::AudioSourceContext {
                project_generation: 11,
                editor_revision: 7,
            }
        );
        let collect = |reservation| {
            roots.with_original_child_inventory_on_control(|inventory| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
                inventory
                    .resident
                    .visit_original_only_worker_children_on_control(&mut ledger)?;
                current.visit_children_selected_storage_on_control(&mut ledger)?;
                cancellation.visit(&mut ledger)?;
                signal.visit_storage_on_control(&mut ledger)?;
                // Actual worker MIDI field is missing, even though the view has a
                // physically different handle to the very same immutable body.
                assert!(
                    ledger
                        .validate_selected_shared_handles_on_control()
                        .is_err()
                );
                hex_engine::visit_shared_midi_preview_storage_on_control(&midi, &mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let without_path = ledger.summary()?;
                inventory.visit_selected_original_children_on_control(&mut ledger)?;
                ledger.validate_selected_shared_handles_on_control()?;
                let measured = ledger.summary()?;
                assert_eq!(
                    measured.data_bytes - without_path.data_bytes,
                    completion.path.capacity()
                );
                assert_eq!(measured.data_allocations - without_path.data_allocations, 1);
                inventory.visit_selected_original_children_on_control(&mut ledger)?;
                assert_eq!(ledger.summary()?, measured);
                assert_eq!(
                    inventory.require_complete_root_storage_on_control(),
                    Err(AudioStorageError::UnknownStorage)
                );
                Ok(measured)
            })
        };
        let measured = collect(budget()).unwrap();
        let mut exact = budget();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        assert_eq!(completion.path, PathBuf::from("pending-export.wav"));
        assert_eq!(current.revision, 7);
    }
}
