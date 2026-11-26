//! Desktop-injected native publication, driven only by the canonical worker.
//! Session owns no device or page endpoints and never interprets their tokens.
use super::*;
#[cfg(test)]
use std::time::Duration;
mod file_sources;
pub(super) mod native_sampler;
pub use file_sources::{
    ResidentFileInput, ResidentFileMetadata, ResidentFilePitch, ResidentFilePublication,
    ResidentFileSeekPreparer, ResidentFileSeekRequest, ResidentFileSet,
};
pub use native_sampler::{
    NativeSamplerInput, NativeSamplerReservationHold, NativeSamplerRetryFacts,
};
#[cfg(test)]
mod tests;
mod types;
mod worker_events;
pub(in crate::service) use types::ResidentHostedMove;
pub use types::{
    ArrangementEditPosition, DeviceInsertAudioReceipt, DeviceInsertAudioState, OfflineBounceSource,
    PausedInputMonitorRequest, PausedNoteGraph, PluginEditorRecordingInput, PluginParameterCapture,
    PluginParameterDelivery, PluginParameterGestureError, PluginParameterGestureInput,
    PluginParameterGestureResult, PluginParameterObservation, PluginParameterOverride,
    ResidentEffectPreparation, ResidentFreshRuntimeFence, ResidentGraphSubmission,
    ResidentHostedInstrumentMove, ResidentHostedTrackEffectMove, ResidentHostedTrackEffectReorder,
    ResidentPowerOn, ResidentPowerOnInterval, ResidentPublicationAcceptance,
    ResidentPublicationFailure, ResidentPublicationInputs, ResidentPublicationMetadata,
    ResidentPublicationPolicy, ResidentPublicationReadiness, ResidentPublicationRefusal,
    ResidentRunningPowerOff, ResidentRuntimeLoop, ResidentSourceOrigin, ResidentTempoEdit,
    ResidentTempoEditOutcome,
};
pub(super) use types::{requires_plugin_profile, sole_hosted_return};
#[cfg(test)]
use worker_events::poll_interval;
pub(super) use worker_events::{ControlPoll, PendingOutcome, WorkerEvents};

/// A single worker-owned desktop adapter. Initialization and every method run
/// on the existing Session worker, including while the UI event queue is full.
/// Construction must not start device work or hold an opened output: a failed
/// thread spawn may drop the uninitialized adapter on the constructing thread.
pub trait WorkerAudioSink: Send {
    /// Whether the sink uses installed-graph commits for resident insertions.
    fn confirms_device_inserts(&self) -> bool {
        false
    }

    fn confirms_verified_device_inserts(&self) -> bool {
        false
    }

    /// Poll the exact Verified canonical install after an independent EQ edit.
    /// A pending receipt waits, and a failed output terminates that insertion.
    /// Graph admission rechecks the installed receipt.
    fn verified_hosted_predecessor_ready(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<bool, &'static str> {
        Ok(true)
    }

    /// Whether the verified transport is running. The stage request
    /// separately proves the canonical source and captures its exact fence.
    fn verified_tempo_playing(&mut self) -> bool {
        false
    }

    /// Prepare the immutable source of an uncommitted editor transaction.
    fn begin_verified_graph_edit(
        &mut self,
        _attempt: super::VerifiedGraphEditAttempt,
    ) -> Result<(), String> {
        Err("verified graph edits are unavailable on this output".into())
    }

    fn finish_verified_graph_edit(
        &mut self,
        _attempt: &super::VerifiedGraphEditAttempt,
        _installed: bool,
    ) {
    }

    /// Borrow the exact receipt immediately after submitting a DeviceInsert.
    fn device_insert_receipt(
        &mut self,
        _submission: ResidentGraphSubmission,
    ) -> Option<Box<dyn DeviceInsertAudioReceipt>> {
        None
    }

    /// Obtain the fixed deadline of the exact refused insert's consent window.
    /// An unavailable or changed output grants no offer.
    fn offer_device_insert_consent(
        &mut self,
        _submission: ResidentGraphSubmission,
    ) -> Option<std::time::Instant> {
        None
    }

    fn retiring_editor_replay_ready(&self) -> bool {
        false
    }

    /// Start the ordinary stopped-source renewal before borrowing a pending
    /// retiring receipt. The Session owns compilation and publication of the
    /// unchanged canonical graph through its existing restore continuation.
    fn prepare_retiring_editor_replay_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Close or document replacement must preserve the Session while this
    /// source still needs replay or an explicit disposition.
    fn retiring_editor_close_blocker(&self) -> Option<super::RetiringEditorCloseBlocker> {
        None
    }

    /// Poll reversible admission holds for exact current editor producers.
    /// Release an admitted hold after the caller's last strong owner drops,
    /// including when it drops before the original query has replied.
    fn prepare_plugin_editor_close(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _attempt: &std::sync::Weak<super::PluginEditorCloseAttempt>,
    ) -> super::PluginEditorCloseAdmission {
        super::PluginEditorCloseAdmission::Held {
            admitted_edits: false,
        }
    }

    fn next_retiring_editor_replay(&mut self) -> Option<super::RetiringEditorBatch> {
        None
    }

    /// VerifyExisting must inspect the unchanged successor without admitting
    /// any new parameter input. Apply verifies earlier values before its later
    /// guarded write or isolated manual-base capture.
    fn capture_retiring_editor_replay(
        &mut self,
        _batch: &super::RetiringEditorBatch,
        _mode: super::RetiringEditorReplayMode<'_>,
    ) -> Result<super::PluginParameterCapture, String> {
        Err("retiring vst3 editor replay is unavailable".into())
    }

    /// End the exact replay attempt after its Session disposition. A refusal
    /// leaves the original receipt in native custody and reports the transfer
    /// failure, or defers one retry after a proved stopped transport fence.
    fn finish_retiring_editor_replay(
        &mut self,
        _batch: &super::RetiringEditorBatch,
        _disposition: super::RetiringEditorReplayDisposition,
    ) -> Result<super::RetiringEditorReplayCompletion, String> {
        Ok(super::RetiringEditorReplayCompletion::Finished)
    }

    /// Release this exact sequence only after Session disposes every event.
    /// An accepted disposition covers the original source and successor.
    /// An error must leave the original receipt available for the same retry.
    fn accept_retiring_editor_batch(
        &mut self,
        _batch: &super::RetiringEditorBatch,
    ) -> Result<(), String> {
        Err("retiring vst3 editor batches are unavailable".into())
    }

    /// Before Begin, request renewal only when the exact stopped source has
    /// returned its endpoints. Session republishes its unchanged canonical
    /// snapshot through the existing stopped-restore owner before admission.
    /// An already owned restoration must refuse this independent request.
    fn plugin_parameter_requires_stopped_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _origin: crate::device_gesture::PluginParameterOrigin,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Bind a selected target without affecting SDK PCM. The caller opens the
    /// shared durable draft using Ready before it may start this exact scope.
    fn prepare_plugin_editor_recording(
        &mut self,
        _input: PluginEditorRecordingInput,
    ) -> Result<PluginParameterObservation, String> {
        Err("plugin editor recording is unavailable for this output".into())
    }
    /// Publish an exact previously prepared editor scope to the original SDK.
    fn start_plugin_editor_recording(
        &mut self,
        _key: crate::device_gesture::GestureKey,
    ) -> Result<(), String> {
        Err("plugin editor recording is unavailable for this output".into())
    }
    /// Stop admission, retaining the accepted prefix until its terminal.
    fn cancel_plugin_editor_recording(&mut self, _key: crate::device_gesture::GestureKey) -> bool {
        false
    }

    /// Drain bounded final-output observations after each confirmed preparation,
    /// including refusal and output loss. The requesting control owner retains
    /// every returned record until durable recording admission; never UI time.
    fn take_plugin_parameter_observation(&mut self) -> Option<PluginParameterObservation> {
        None
    }
    /// Use the existing actual source for one keyed plugin operation. Begin
    /// revalidates observed metadata/context. Preview/Restore must apply the
    /// real point before returning Applied; they never edit canonical state.
    /// Commit captures/stages that same source and returns DATA only. A refusal
    /// must preserve the last acknowledged preview or report source loss; it
    /// must not fabricate application, restore or a canonical terminal.
    fn prepare_plugin_parameter_gesture(
        &mut self,
        _input: PluginParameterGestureInput<'_>,
    ) -> Result<PluginParameterGestureResult, PluginParameterGestureError> {
        Err(PluginParameterGestureError::Refused(
            "plugin parameter controls are unavailable for this output".into(),
        ))
    }
    /// Correlate the exact graph's callback terminal before the editor writes history.
    fn confirm_plugin_parameter_publication(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), PluginParameterGestureError> {
        Err(PluginParameterGestureError::Refused(
            "vst3 publication confirmation is unavailable".into(),
        ))
    }
    /// Wait for the exact audio terminal of an edit that changes a revision
    /// with active source effects. A submission alone cannot commit history.
    fn confirm_source_effect_publication(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        match acceptance {
            ResidentPublicationAcceptance::OfflineRetained => Ok(()),
            ResidentPublicationAcceptance::Submitted { .. } => {
                Err("source effect publication confirmation is unavailable".into())
            }
        }
    }

    /// If the host scheduled this submission, await its exact callback
    /// terminal before committing the canonical tempo edit.
    fn confirm_resident_tempo_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _acceptance: ResidentPublicationAcceptance,
    ) -> Result<ResidentTempoEditOutcome, String> {
        Ok(ResidentTempoEditOutcome::Confirmed)
    }

    /// Called for every Commit attempt, including capture/compilation/publication
    /// refusal. Retire only the exact key/sequence/input context's pending
    /// compiler permission. This is the canonical worker's actual terminal,
    /// never a request to force acceptance or roll back foreign DSP.
    fn finish_plugin_parameter_commit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _key: crate::device_gesture::GestureKey,
        _sequence: u64,
        _accepted: bool,
    ) {
    }
    /// Capture every active hosted instance required by this exact canonical
    /// capture input. The existing worker retains request order while the adapter
    /// drains each admitted native token to its matching terminal, including
    /// failure/cancellation. No stop, canonical edit or source owner transfer.
    /// An output without native hosting has no runtime states to capture and
    /// preserves the project's opaque unavailable state. Native hosts must
    /// override this method; failed capture must never fall back to empty data.
    fn capture_plugin_states(
        &mut self,
        _input: PluginStateCaptureInput<'_>,
    ) -> Result<Vec<PluginStateCapture>, String> {
        Ok(Vec::new())
    }
    /// Current ordered worker request's read-only shutdown/document-intent
    /// cancellation. Hosts may poll it during bounded native preparation; it
    /// never authorizes an edit or changes ordinary FIFO completion order.
    fn begin_request(&mut self, _cancellation: RequestCancellation) {}
    /// Exact prepared canonical transition terminal, including compilation or
    /// publication refusal. Native custody remains with the sink's Root.
    fn finish_resident_effect_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _accepted: bool,
    ) {
    }

    /// The fresh output's plug-in restoration owns publication for this
    /// context. Restart must wait for its ready receipt before compiling.
    fn plugin_state_restore_pending(&self, _context: hex_engine::AudioSourceContext) -> bool {
        false
    }

    /// A fresh-output owner has restored the unchanged canonical plugin state
    /// through its actual native/endpoint ACKs. The worker may now recompile
    /// that exact context; this scalar transfers no source or graph custody.
    fn plugin_state_restore_ready(&self) -> Option<hex_engine::AudioSourceContext> {
        None
    }
    /// Exact same-context recompilation terminal. Before acceptance, the sink
    /// retains its admitted unpublished document graph and original resources.
    /// Refusals carry the reason for the matching output's recovery consumer.
    fn finish_plugin_state_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _result: Result<(), &str>,
    ) {
    }
    /// Renew stopped native sources through their original output owner. None
    /// keeps the ordinary restore path; Some(false) retains the operation until
    /// its exact callback returns and fresh endpoint ACKs, Some(true) requests
    /// recompilation of the unchanged canonical project before publication.
    fn prepare_stopped_playback_restore(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<Option<bool>, String> {
        Ok(None)
    }
    fn stopped_playback_restore_ready(&self) -> bool {
        false
    }
    /// Refusal before publication releases only the matching held operation.
    /// Accepted publication retains its gate until actual callback installation.
    fn cancel_stopped_playback_restore(&mut self) {}
    /// Start/poll this exact explicit source request on the canonical worker.
    /// The implementation retains actual native preparation under its Root;
    /// Session receives captured state only after real native and endpoint ACKs.
    fn prepare_plugin_effect(
        &mut self,
        _input: PluginEffectPreparation<'_>,
    ) -> Result<PluginEffectPreparationState, String> {
        Err("vst3 hosting unavailable for this output".into())
    }
    fn plugin_effect_ready(&self) -> bool {
        false
    }
    /// Send a window action to the existing hosted instance, without a project edit.
    fn toggle_plugin_editor(&mut self, _generation: u64, _device: u64) -> Result<(), String> {
        Err("vst3 editor unavailable for this output".into())
    }
    fn plugin_editor_ready(&self) -> bool {
        false
    }
    fn take_plugin_editor_result(&mut self, _generation: u64) -> Option<Result<bool, String>> {
        None
    }
    fn query_plugin_parameters(&mut self, _query: PluginParameterQuery) -> Result<(), String> {
        Err("vst3 parameters unavailable for this output".into())
    }
    fn plugin_parameters_ready(&self) -> bool {
        false
    }
    fn take_plugin_parameters(&mut self) -> Option<PluginParameterResult> {
        None
    }
    fn query_plugin_parameter_conversion(
        &mut self,
        _query: PluginParameterConversionQuery,
    ) -> Result<(), String> {
        Err("vst3 conversion unavailable for this output".into())
    }
    fn plugin_parameter_conversion_ready(&self) -> bool {
        false
    }
    fn take_plugin_parameter_conversion(&mut self) -> Option<PluginParameterConversionResult> {
        None
    }
    /// Match the exact request control; never cancel an unrelated later source.
    /// Exact canonical terminal. Rejected requests retire only their matching native preparation;
    /// acceptance transfers display/request custody, never the native source's lifetime.
    fn finish_plugin_effect(&mut self, _control: &Arc<PluginEffectControl>, _accepted: bool) {}
    /// Borrow the actual host's effect facts for one synchronous registered
    /// compiler invocation on the canonical worker. None means no host facts
    /// were supplied, so builtin-only preparation remains unchanged. The
    /// callback must run at most once and must not publish or call back into
    /// the sink: native source custody/Work can remain borrowed throughout it.
    /// A returned revision is compilation only, never native admission or ACK.
    fn prepare_resident_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        _prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<Option<hex_engine::PreparedRevision>, String> {
        Ok(None)
    }
    /// Describe saved hosted effects for editor compilation before a new
    /// output exists. No native admission or audible substitution is authorized.
    fn prepare_saved_verified_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        _prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        Err("verified vst3 effects are unavailable".into())
    }
    /// Compile editor-facing Verified Streaming metadata against exact hosted
    /// definitions for one synchronous borrow. An exact catalog proposal may
    /// retry transient contention within its attempt's preparation budget.
    /// Publication and callback receipts stay with the verified edit pipeline.
    fn prepare_verified_effects(
        &mut self,
        _input: ResidentEffectPreparation<'_>,
        _attempt: Option<&super::VerifiedGraphEditAttempt>,
        _prepare: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        Err("verified vst3 effects are unavailable".into())
    }
    /// Optionally hold the exact stopped predecessor while a saturated history
    /// is prepared. False preserves the ordinary conservative history path.
    /// A successful lease must remain held through its publication terminal.
    fn begin_stopped_foreign_history(
        &mut self,
        _predecessor: ResidentGraphSubmission,
        _successor: hex_engine::AudioSourceContext,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Confirm a leased successor before canonical history commits. Hosts with
    /// no lease leave the existing publication path unchanged.
    fn confirm_stopped_foreign_history(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _accepted: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Actual original-worker pickup only, for bounded immutable program
    /// retention. Unknown/Empty retain existing history; only an actual empty
    /// worker receipt can qualify a first programme. Never synthesize this from
    /// a queued graph key or final Main publication.
    fn received_foreign_program_on_control(
        &mut self,
        _binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> hex_engine::ForeignProgramFrontier {
        hex_engine::ForeignProgramFrontier::Unknown
    }

    /// A host with no private offline capability must refuse foreign bounce
    /// instead of using its live source or publishing unprocessed audio.
    fn prepare_offline_bounce(
        &mut self,
        input: ResidentEffectPreparation<'_>,
        _control: &hex_export::ExportControl,
    ) -> Result<Option<Box<dyn OfflineBounceSource>>, String> {
        if input.requires_plugin_profile() {
            return Err("offline vst3 bounce unavailable for this host".into());
        }
        Ok(None)
    }
    /// Rate of the current default interface for an un-authored document.
    /// An unavailable observation leaves selection to the existing fallback.
    fn default_project_sample_rate(&self) -> Option<u32> {
        None
    }
    /// Ordinary source-planning defaults, independent of the document mode.
    /// A host opts in only with bounded file preparation/publication consumers.
    fn ordinary_source_reservation(&self) -> Option<crate::SourcePlanReservation> {
        None
    }
    /// Supply compiler facts for this captured precommit source set. This may
    /// reserve a catalog generation but grants no source/page custody. Actual
    /// catalog verification and endpoint delivery precede final publication.
    fn prepare_resident_file_metadata(
        &mut self,
        input: ResidentFileInput<'_>,
    ) -> Result<Option<ResidentFileMetadata>, String> {
        if input.files.is_empty() {
            Ok(None)
        } else {
            Err("ordinary file-backed playback is unavailable for this output".into())
        }
    }
    /// Optional stateless offline renderer. Query only on the Session worker;
    /// the returned function runs on the existing export worker with an exact
    /// canonical snapshot. It must use independent bounded pages/verification,
    /// never borrow native output/catalog/readiness. Optional ordinary compiler
    /// resources remain borrowed from the admitted job; private constructor
    /// source custody is not composed with page rendering.
    fn streamed_exporter(&self) -> Option<super::StreamedExporter> {
        None
    }
    /// Optional stateless offline bounce renderer for file-backed sources, with
    /// the same independence rules as [`Self::streamed_exporter`]. It runs on the
    /// Session worker during a bounce job and never touches the playing transport.
    fn streamed_bouncer(&self) -> Option<crate::StreamedBouncer> {
        None
    }

    /// Prepare ordinary file-backed history on the existing seek worker, using
    /// independent bounded offline pages and the exact captured compiler scope.
    /// This stateless function grants no native submission/readiness authority.
    fn resident_file_seek_preparer(&self) -> Option<ResidentFileSeekPreparer> {
        None
    }
    /// Conservative listed peak for the independent offline page owner, quoted
    /// before starting a worker/pool. Opaque allocator/thread/OS costs remain
    /// separate bounded obligations, not a process-memory receipt.
    fn resident_file_seek_storage_bytes(
        &self,
        _prepared: &PreparedRevision,
        _sources: &ResidentFileSet,
    ) -> Result<usize, String> {
        Err("file-backed seek preparation is unavailable".into())
    }

    /// Output ownership policy, independent of device presence or availability. A native consumer
    /// opts in only after routing every resident graph through this worker-private boundary.
    /// This capability survives document mode changes: protocol dispatch must also require
    /// Resident mode, while cross-mode admission still borrows actual resident/retired owners.
    fn resident_publication_enabled(&self) -> bool {
        false
    }
    /// Observe the exact resident publication without granting graph custody.
    /// The default denotes an output that does not support resident graphs.
    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        ResidentPublicationReadiness::NoOutput
    }
    /// Observe the exact Main device and source in a settled Stop interval for
    /// stopped power or replacement history.
    /// This grants no custody; publication must revalidate the complete witness.
    fn stopped_resident_power_fence(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _device: u64,
    ) -> Result<Option<ResidentFreshRuntimeFence>, String> {
        Ok(None)
    }

    /// Observe the exact resource-free Return Off on a settled native output.
    /// An installed Off source with a stale stop interval must refuse On.
    fn stopped_resident_return_power_on_fence(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _owner: hex_engine::ProcessorChainOwner,
        _device: u64,
    ) -> Result<Option<ResidentFreshRuntimeFence>, String> {
        Ok(None)
    }

    /// Observe an accepted Track or Return worker on the exact stopped source.
    /// This grants no custody; publication must revalidate the same witness.
    fn stopped_resident_effect_history_fence(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _owner: hex_engine::ProcessorChainOwner,
        _device: u64,
    ) -> Option<ResidentFreshRuntimeFence> {
        None
    }

    /// Observe the exact playing Resident source with the accepted effect
    /// that the staged history replaces. Grants no worker custody.
    fn running_resident_effect_history_source(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _owner: hex_engine::ProcessorChainOwner,
        _device: u64,
    ) -> Option<ResidentGraphSubmission> {
        None
    }

    /// Copy the exact currently playing hosted source; no endpoint custody escapes.
    fn running_resident_power_off(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _owner: hex_engine::ProcessorChainOwner,
        _device: u64,
    ) -> Result<Option<ResidentRunningPowerOff>, String> {
        Ok(None)
    }

    /// Observe the exact workerless Off graph of a playing Resident output.
    fn running_resident_main_power_on(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _device: u64,
    ) -> Option<ResidentGraphSubmission> {
        None
    }

    /// Observe the exact reopened workerless saved-Off Return of a playing Resident output.
    fn running_resident_return_power_on(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _owner: hex_engine::ProcessorChainOwner,
        _device: u64,
    ) -> Option<ResidentGraphSubmission> {
        None
    }

    /// Shape-only Return eligibility never stages a running power command
    /// while the sink's ordinary playback transport is stopped.
    fn resident_playing(&mut self) -> bool {
        false
    }

    /// Actual output/source/revision-bound loop, never the saved loop marker.
    /// None means no active ordinary output; offline edits remain available.
    /// A witness may become stale during preparation and must be revalidated by
    /// `publish_resident` before admitting `InheritRuntimeAt`.
    fn resident_runtime_loop(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<Option<ResidentRuntimeLoop>, String> {
        Ok(None)
    }
    /// Exact current armed and playing ordinary output. A copied observation
    /// cannot reserve an audio input or substitute for a delivery receipt.
    fn device_automation_source(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _target: super::DeviceCaptureTarget,
    ) -> Result<Option<super::DeviceAutomationSource>, String> {
        Ok(None)
    }
    /// Synchronously borrow actual active/retiring/unpublished owners for every precommit check.
    /// No borrow may survive this worker call; each actual root is selected once per union.
    fn resident_admission(
        &self,
    ) -> [hex_engine::native_device_resources::DeviceResourceAdmission<'_>; 3] {
        [hex_engine::native_device_resources::DeviceResourceAdmission::default(); 3]
    }
    /// Prepare one control-owned monitor under the existing resident union.
    /// The sink retains its corresponding storage hold until retirement.
    fn prepare_paused_input_monitor(
        &mut self,
        _request: PausedInputMonitorRequest,
        _revision: &Arc<PreparedRevision>,
        _resources: &hex_engine::native_device_resources::PreparedDeviceResources,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<hex_engine::PausedTrackMonitor, String> {
        Err("paused input monitor output is unavailable".into())
    }
    /// Advance a bounded preparation step. No output ownership survives the
    /// call except in the sink's own pending state. Return true while busy.
    fn poll_paused_note_monitor(
        &mut self,
        _graph: Option<PausedNoteGraph<'_>>,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> bool {
        false
    }
    /// Final fallible prepared-transition operation. An active output must refuse queue/resource
    /// pressure unchanged; explicit unavailable/opening state may accept one admitted latest owner
    /// privately with no submitted/applied revision. Cancellation checks precede this call.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Exact graph/resources/observation custody and unchanged refusal remain explicit.
    fn publish_resident(
        &mut self,
        playback: Playback,
        _resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        _observation: Arc<hex_engine::PreparedPlaybackObservation>,
        _metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        _metadata_limit: usize,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        Err(ResidentPublicationRefusal {
            kind: crate::service::ResidentPublicationFailure::Other,
            playback,
            error: "ordinary worker publication is unavailable".into(),
        })
    }
    /// Same ordinary resource publisher with captured file facts and borrowed
    /// cancellation. Hosts without file consumers keep the resident-only path;
    /// metadata alone cannot authorize a streamed graph or offline recovery.
    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn publish_resident_transition(
        &mut self,
        playback: Playback,
        resources: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
        observation: Arc<hex_engine::PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
        inputs: ResidentPublicationInputs<'_>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        if inputs.device_insert_control.is_some() {
            return Err(ResidentPublicationRefusal {
                kind: ResidentPublicationFailure::Other,
                playback,
                error: "cancellable device insertion is unavailable for this output".into(),
            });
        }
        if playback.contains_streamed_sources() {
            return Err(ResidentPublicationRefusal {
                kind: crate::service::ResidentPublicationFailure::Other,
                playback,
                error: "ordinary file-backed publication is unavailable for this output".into(),
            });
        }
        self.publish_resident(
            playback,
            resources,
            observation,
            metadata,
            policy,
            metadata_limit,
            admit,
        )
    }
    /// Finish an already submitted document replacement before the canonical swap.
    /// A host that opened a private candidate output must observe its exact
    /// installation or retire that candidate on refusal. Offline retention
    /// and hosts without a private output have no additional terminal to wait for.
    fn finish_resident_document_replacement(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Retry an already admitted offline candidate only while the canonical worker lends the
    /// current source/export/control union. Pending custody is not released by cancellation/ACK.
    fn poll_resident_pending(
        &mut self,
        _admit: &mut dyn FnMut(
            &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
        ) -> Result<(), String>,
    ) {
    }
    /// Start audio without waiting for device opening and return the initial
    /// canonical project rate. Polling owns delayed completion and errors; an
    /// opened output must honor this rate without retiming intervening edits.
    /// An immediate failure leaves editing available at 48000 Hz. Observe
    /// shutdown before starting; never await a device or UI response here.
    fn initialize(&mut self, shutdown: &AtomicBool) -> Result<u32, String>;
    /// Called before the corresponding canonical event can block on UI delivery.
    /// On generation replacement relinquish the previous publication/output
    /// first. Replacement opening must be polled, not awaited in this method.
    fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>);
    /// One coalesced verified gain overlay/terminal, constructed by the worker
    /// from its actual canonical source and validated gesture. Metadata only;
    /// acceptance is not callback application or new source admission.
    fn verified_track_gain_source(&mut self, _source: Arc<VerifiedTrackGainSource>) {}
    /// Worker-derived native-effect metadata, anchored to the canonical source.
    /// A consumer uses its existing reserved builder; this grants no source,
    /// output or callback owner. Offline/default sinks retain model editing.
    fn verified_device_parameter_source(&mut self, _source: Arc<VerifiedDeviceParameterSource>) {}
    /// Accept source-bound clip intent on control. This is not graph admission
    /// or callback acknowledgement; false retains no worker-side delivery claim.
    fn verified_clip_move_update(&mut self, _update: VerifiedClipMoveUpdate) -> bool {
        false
    }
    /// Replace or retire temporary Arrangement gain metadata. Acceptance is
    /// a bounded control handoff, independent of callback application.
    fn verified_arrangement_gain_update(&mut self, _update: VerifiedArrangementGainUpdate) -> bool {
        false
    }
    /// Replace or retire temporary Arrangement play-start metadata. Acceptance
    /// is a bounded control handoff, independent of callback application.
    fn verified_arrangement_play_start_update(
        &mut self,
        _update: VerifiedArrangementPlayStartUpdate,
    ) -> bool {
        false
    }
    /// Synchronous private canonical/original PCM input; no borrow may survive.
    fn native_sampler_input(&mut self, _input: NativeSamplerInput<'_>) {}
    /// A bounded stopped Arrangement handshake, shared by structural edits and
    /// in-place bounce/history. The historical method name transfers no custody.
    fn begin_section_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _allow_playing: bool,
    ) -> Result<(), String> {
        Err("native arrangement edits are unavailable".into())
    }
    /// Callback-observed transport. None means the command prefix is pending.
    /// A structural section edit can recover from a refused same-project
    /// publication by fencing the exact runtime still installed on the output.
    fn begin_recoverable_section_edit(
        &mut self,
        context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        self.begin_section_edit(context, true)
    }
    /// A recovered section edit must reach its callback terminal before commit.
    /// An ordinary section edit uses the existing publication contract.
    fn confirm_section_edit_recovery(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _acceptance: ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        Ok(())
    }
    fn section_edit_ready(&mut self) -> Option<Result<ArrangementEditPosition, String>> {
        None
    }
    /// Precommit cancellation only. Accepted publication retains its gate.
    fn cancel_section_edit(&mut self) {}
    fn begin_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit requires native audio".into())
    }
    /// None while awaiting the bounded callback fence; never wait here.
    fn sampler_edit_ready(&mut self) -> Option<Result<(), String>> {
        None
    }
    fn prepare_sampler_edit(&mut self, _input: NativeSamplerInput<'_>) -> Result<(), String> {
        Err("sampler edit preparation is unavailable".into())
    }
    fn commit_sampler_edit(
        &mut self,
        _context: hex_engine::AudioSourceContext,
    ) -> Result<(), String> {
        Err("sampler edit publication is unavailable".into())
    }
    fn cancel_sampler_edit(&mut self) {}
    /// Scalar activation only; implementations must verify the actual installed
    /// native CURRENT before accepting this exact output/context/track/device.
    fn prepare_sampler_keyboard(
        &mut self,
        _context: hex_engine::AudioSourceContext,
        _output: u64,
        _epoch: u64,
        _track: u64,
        _device: u64,
    ) -> Result<(), String> {
        Err("sampler keyboard is unavailable".into())
    }
    /// Hold remains charged through actual native teardown, never a UI ACK.
    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        NativeSamplerReservationHold::default()
    }
    /// Advance bounded preparation/return work; no UI polling is required.
    fn poll(&mut self);
    /// Whether `poll` currently has no time-sensitive work: no playback,
    /// open, edit, preparation, queued event or return awaiting progress.
    /// Only then may the worker lengthen its wait to [`IDLE_POLL_INTERVAL`];
    /// a queued request still wakes it at once. The default `false` keeps
    /// the [`BUSY_POLL_INTERVAL`] cadence for every sink that does not opt in.
    /// Must be a cheap read of worker-owned state; it must not do work
    /// (`&mut` only reaches borrowed native status views).
    fn worker_idle(&mut self) -> bool {
        false
    }
    /// Test-only hook to observe each scheduled poll wait and optionally shorten
    /// waits so cadence tests remain deterministic under load.
    #[cfg(test)]
    fn test_poll_wait(&mut self, wait: Duration) -> Duration {
        wait
    }
    /// Start a correlated reopen on the existing lifecycle lane. Implementations
    /// must relinquish the old output before returning; readiness is reported
    /// separately without waiting for a Client or UI event delivery.
    fn restart_audio(&mut self, _project_generation: u64, _request_id: u64) -> Result<(), String> {
        Err("audio restart is unavailable".into())
    }
    /// Relinquish output/publication custody without waiting for UI collection.
    /// Idempotent; called on the Session worker on every ordinary exit.
    fn close(&mut self);
}
