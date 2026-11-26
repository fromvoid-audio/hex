//! Exact real-runtime demand and held-role transactions. Correlation is copied
//! from the same private State/runtime; it cannot be submitted independently.
use super::*;
use hex_stream::{ProtectionRole, ReadinessProgress, ReadinessStamp};

/// Observe only a demanded audio read/decode failure. Capacity is retryable,
/// source identity has its own report, and unrelated/scalar failures do not
/// describe the audio window that is about to render.
fn current_read_failure(failure: hex_stream::StreamFailure, demanded: bool) -> bool {
    demanded
        && failure.kind == hex_stream::StreamFailureKind::DecodeFailed
        && matches!(failure.key, hex_engine::StreamResourceKey::Audio(_))
}

#[cfg(test)]
mod tests {
    use super::current_read_failure;
    use hex_engine::{ScalarSourceKey, StreamSourceKey};
    use hex_stream::{StreamFailure, StreamFailureKind};

    #[test]
    fn only_current_audio_decode_failures_notify() {
        let mut model =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        let hex_project::Created::Asset(asset) = model
            .execute(hex_project::Command::AddAsset {
                path: "read-policy.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        let audio = StreamSourceKey::new(1, asset).into();
        let scalar = ScalarSourceKey::new(1, 2, 3).unwrap().into();
        for (key, kind, demanded, expected) in [
            (audio, StreamFailureKind::DecodeFailed, true, true),
            (audio, StreamFailureKind::DecodeFailed, false, false),
            (scalar, StreamFailureKind::DecodeFailed, true, false),
            (audio, StreamFailureKind::SourceChanged, true, false),
            (audio, StreamFailureKind::CapacityExhausted, true, false),
            (audio, StreamFailureKind::Cancelled, true, false),
            (audio, StreamFailureKind::UnknownSource, true, false),
        ] {
            assert_eq!(
                current_read_failure(
                    StreamFailure {
                        key,
                        page_index: 0,
                        kind
                    },
                    demanded
                ),
                expected
            );
        }
    }

    #[test]
    fn foreign_tail_source_busy_refuses_instead_of_retrying_forever() {
        use super::super::view::Correlation;
        use super::{StateView, StreamInstall, Workspace};
        use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
        use hex_engine::playback::Playback;
        use hex_engine::{
            AudioSourceContext, PlaybackAction, PlaybackCommand, PlaybackHandoffError,
            PreparedRevision, SessionPlaybackError, SineTable, StreamedSourceMetadata,
        };
        use hex_lattice::{
            DevicePreparationContext, NativeDeviceDefinition, NativeDeviceRegistry, OutputTail,
            PreparedForeignProcessor, PreparedProcessorFactory, ProcessorChainOwner,
            ProcessorInstanceIdentity, StereoMatrix,
        };
        use hex_lattice_contract::{
            DeviceContractError, DeviceTypeId, ForeignProcessorBinding, NativeDeviceMetadata,
            NativeDeviceState, ParameterDescriptor, ParameterId, ParameterValue,
        };
        use hex_project::{Command, Created, Editor, Project};
        use hex_stream::{ProtectionRole, ReadinessArtifact};
        use std::collections::BTreeMap;
        use std::sync::{Arc, atomic::AtomicBool};

        const KIND: DeviceTypeId = DeviceTypeId::from_static("test.foreign.stream.busy");
        struct ForeignDef {
            identity: ProcessorInstanceIdentity,
            binding: ForeignProcessorBinding,
        }
        impl NativeDeviceMetadata for ForeignDef {
            fn type_id(&self) -> DeviceTypeId {
                KIND
            }
            fn state_version(&self) -> u32 {
                1
            }
            fn descriptors(&self) -> &'static [ParameterDescriptor] {
                &[]
            }
            fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
                NativeDeviceState::new(KIND, 1, vec![1])
            }
            fn migrate_state(
                &self,
                value: &NativeDeviceState,
            ) -> Result<NativeDeviceState, DeviceContractError> {
                Ok(value.clone())
            }
            fn read_parameter(
                &self,
                _: &NativeDeviceState,
                _: ParameterId,
            ) -> Result<ParameterValue, DeviceContractError> {
                Err(DeviceContractError::UnknownParameter)
            }
            fn patch_parameter(
                &self,
                _: &NativeDeviceState,
                _: ParameterId,
                _: ParameterValue,
            ) -> Result<NativeDeviceState, DeviceContractError> {
                Err(DeviceContractError::UnknownParameter)
            }
        }
        impl NativeDeviceDefinition for ForeignDef {
            fn prepare_matrix(
                &self,
                _: &NativeDeviceState,
            ) -> Result<StereoMatrix, DeviceContractError> {
                Ok(StereoMatrix::identity())
            }
            fn prepare_runtime_factory(
                &self,
                _: &NativeDeviceState,
                _: &DevicePreparationContext<'_>,
            ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
                Err(DeviceContractError::UnsupportedProcessingClass)
            }
            fn prepare_instance_runtime_factory(
                &self,
                identity: ProcessorInstanceIdentity,
                _: &NativeDeviceState,
                _: &DevicePreparationContext<'_>,
            ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
                if identity != self.identity {
                    return Err(DeviceContractError::InvalidId);
                }
                let prepared = PreparedForeignProcessor::new(8, 0, 16)?
                    .with_binding(self.binding)?
                    .with_output_tail(OutputTail::Unbounded);
                Ok(Some(PreparedProcessorFactory::Foreign(prepared)))
            }
        }

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "clip.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 4096,
                source_offset: 0,
                source_length: 4096,
                looping: false,
            })
            .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: NativeDeviceState::new(KIND, 1, vec![1]).unwrap(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        let identity =
            ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, device.value()).unwrap();
        let binding = ForeignProcessorBinding {
            root: 1,
            instance: device.value(),
            graph: 1,
            epoch: 1,
        };
        let def = ForeignDef { identity, binding };
        let defs: [&dyn NativeDeviceDefinition; 1] = [&def];
        let registry = NativeDeviceRegistry::new(&defs).unwrap();
        let key = StreamSourceKey::new(2, asset);
        let metadata = StreamedSourceMetadata::new(
            key,
            48_000,
            48_000,
            4096,
            4096,
            hex_project::IDENTITY_CONVERSION,
            [2; 32],
        )
        .unwrap();
        let sources = BTreeMap::from([(
            asset,
            hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
        )]);
        let cancel = AtomicBool::new(false);
        let sine = Arc::new(SineTable::new());
        let resources = PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            editor.project(),
            1 << 20,
            &cancel,
        )
        .unwrap();
        let compile = |project: &Project| {
            let revision = PreparedRevision::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
                project,
                &sources,
                key.generation(),
                Arc::clone(&sine),
                None,
                &hex_engine::audio_transform::AudioTransformPreparation::default(),
                &resources,
                100_000,
                &cancel,
                &registry,
                None,
            )
            .unwrap();
            Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources)
                .unwrap()
        };

        let mut previous = compile(editor.project());
        previous
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: NativeDeviceState::new(KIND, 1, vec![2]).unwrap(),
            })
            .unwrap();
        let mut candidate = compile(editor.project());

        let old_context = AudioSourceContext {
            project_generation: 2,
            editor_revision: 1,
        };
        let new_context = AudioSourceContext {
            project_generation: 2,
            editor_revision: 2,
        };
        let mut old_workspace = None;
        let mut old_view = StateView {
            runtime: Some(previous.stream_playback(old_context)),
            streaming: &mut old_workspace,
            domain_refs: [None; 2],
            context_generation: 2,
            correlation: Correlation {
                root: 1,
                owner: ReadinessArtifact::Ordinary { publication: 1 },
            },
        };
        let mut candidate_workspace = Some(Workspace::prepare(8).unwrap());
        let mut candidate_view = StateView {
            runtime: Some(candidate.stream_playback(new_context)),
            streaming: &mut candidate_workspace,
            domain_refs: [None; 2],
            context_generation: 2,
            correlation: Correlation {
                root: 1,
                owner: ReadinessArtifact::Ordinary { publication: 2 },
            },
        };
        let mut domains = [None, None];
        let mut install = StreamInstall {
            domains: &mut domains,
            pending_play: false,
            command_prefix: 0,
            frames: 256,
            fenced_restart: false,
            foreign_continuation: false,
            confirmed_fade: None,
            tempo_cut: None,
        };

        // A live conflicting foreign endpoint is not transient: streamed demand
        // must refuse it immediately so the callback can reject the publication
        // and release publication_pending instead of retrying forever.
        let result = candidate_view.prepare_stream_window(
            Some(&mut old_view),
            &mut install,
            ProtectionRole::Candidate,
        );
        assert!(
            result.is_err(),
            "conflicting live foreign endpoint must be refused, not deferred: {result:?}"
        );
        assert_eq!(
            candidate_workspace
                .as_mut()
                .unwrap()
                .take_handoff_diagnostic(),
            Some(PlaybackHandoffError::Session(
                SessionPlaybackError::ForeignTailSourceBusy
            ))
        );
    }
}
pub(crate) enum TempoStreamWindow<'a> {
    Forecast(&'a hex_engine::session_playback::PreparedTempoAnchor),
    Exact(&'a hex_engine::session_playback::PreparedTempoAnchor),
}

pub(crate) struct StreamInstall<'a> {
    pub(crate) domains: &'a mut [Option<DomainEnvelope>; 2],
    pub(crate) pending_play: bool,
    pub(crate) command_prefix: u64,
    pub(crate) frames: usize,
    pub(crate) fenced_restart: bool,
    pub(crate) foreign_continuation: bool,
    pub(crate) confirmed_fade: Option<u32>,
    pub(crate) tempo_cut: Option<TempoStreamWindow<'a>>,
}
pub(super) fn envelope(
    domains: &mut [Option<DomainEnvelope>; 2],
    token: AudioCandidateDomainToken,
) -> Result<&mut DomainEnvelope, hex_engine::Invalid> {
    domains
        .get_mut(token.slot)
        .and_then(Option::as_mut)
        .filter(|pages| pages.inner.token == token && pages.inner.loan.is_some())
        .ok_or(hex_engine::Invalid("exact stream page loan is unavailable"))
}
impl Workspace {
    pub(super) fn insert(
        extents: &mut [Vec<StreamResourceExtent>; 2],
        limit: usize,
        index: usize,
        extent: StreamResourceExtent,
    ) -> Result<(), hex_engine::Invalid> {
        let rows = &mut extents[index];
        match rows.binary_search_by_key(&(extent.source, extent.page_index), |row| {
            (row.source, row.page_index)
        }) {
            Ok(index) => {
                rows[index].start = rows[index].start.min(extent.start);
                rows[index].end = rows[index].end.max(extent.end);
            }
            Err(index) if rows.len() < limit => rows.insert(index, extent),
            Err(_) => return Err(hex_engine::Invalid("exact stream demand capacity exceeded")),
        }
        Ok(())
    }
    fn cancel_pending(
        &mut self,
        refs: [Option<AudioCandidateDomainToken>; 2],
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        if let Some(role) = self.role.take() {
            for token in refs.into_iter().flatten() {
                if let Some(stamp) = self.stamps[token.slot].take()
                    && let Ok(pages) = envelope(domains, token)
                    && let Some(loan) = pages.inner.loan.as_mut()
                {
                    let _ = loan.cancel(role, stamp.request_epoch);
                }
            }
        }
    }
}
impl StateView<'_> {
    pub(in crate::candidate_custody::eligible::publication) fn cancel_stream_pending(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        if let Some(workspace) = self.streaming.as_mut() {
            workspace.cancel_pending(self.domain_refs, domains);
        }
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_window(
        &mut self,
        previous: Option<&mut StateView<'_>>,
        request: &mut StreamInstall<'_>,
        role: ProtectionRole,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        self.prepare_stream_window_at(previous, request, role, None)
    }
    pub(super) fn prepare_stream_window_at(
        &mut self,
        mut previous: Option<&mut StateView<'_>>,
        request: &mut StreamInstall<'_>,
        role: ProtectionRole,
        due: Option<u64>,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        if request.tempo_cut.is_some() && (previous.is_none() || due.is_some()) {
            return Err(hex_engine::Invalid(
                "tempo cut requires its exact stream predecessor",
            ));
        }
        let refs = self.domain_refs;
        let workspace = self
            .streaming
            .as_mut()
            .filter(|w| w.valid())
            .ok_or(hex_engine::Invalid("stream demand workspace unavailable"))?;
        workspace.handoff_diagnostic = None;
        workspace.source_changed = false;
        workspace.read_failed = false;
        workspace.capacity_conflict = false;
        let runtime = self
            .runtime
            .as_mut()
            .ok_or(hex_engine::Invalid("stream runtime unavailable"))?;
        let context = runtime
            .source_context()
            .ok_or(hex_engine::Invalid("canonical stream context unavailable"))?;
        if context.project_generation != self.context_generation {
            return Err(hex_engine::Invalid("canonical stream context differs"));
        }
        let rate = runtime
            .sample_rate()
            .ok_or(hex_engine::Invalid("stream rate unavailable"))?;
        let predecessor = if role == ProtectionRole::Candidate {
            previous.as_ref().map(|old| old.correlation.owner)
        } else {
            workspace.predecessor
        };
        // A larger transaction may cover more distinct pages than the fixed
        // readiness reservation. At most nine pure attempts halve the 256-frame
        // maximum to one, without allocating or publishing a partial intent.
        // Failure at one frame retains the existing refusal.
        let mut frames = request.frames.min(256);
        // Only an immediate successor shrinks after shared-pool capacity
        // refusal. Each retry replans its exact source and clock; the owner
        // cancels every partial domain intent before retrying. Current demand
        // and scheduled prewarm retain their existing limits and protection.
        if role == ProtectionRole::Candidate && due.is_none() {
            frames = frames.min(workspace.candidate_frame_limit);
        }
        let window = loop {
            for rows in &mut workspace.extents {
                rows.clear();
            }
            let domains = &*request.domains;
            let staged = workspace.pending_action.as_ref();
            let extents = &mut workspace.extents;
            let limit = workspace.limit;
            let overflow = std::cell::Cell::new(false);
            let mut visit = |extent: StreamResourceExtent| {
                let index = refs
                    .iter()
                    .flatten()
                    .find_map(|token| {
                        domains
                            .get(token.slot)
                            .and_then(Option::as_ref)
                            .filter(|pages| pages.inner.token == *token)
                            .and_then(|pages| pages.inner.loan.as_ref())
                            .is_some_and(|loan| loan.covers_resource_key(extent.source))
                            .then_some(token.slot)
                    })
                    .ok_or(hex_engine::Invalid(
                        "stream demand source lacks actual domain",
                    ))?;
                let result = Workspace::insert(extents, limit, index, extent);
                if result.is_err() {
                    overflow.set(true);
                }
                result
            };
            let planned = if let Some(due) = due {
                runtime.visit_pending_stream_chunk(due, frames, &mut visit)
            } else if let Some(previous) = previous.as_deref_mut() {
                let previous = previous
                    .runtime
                    .as_mut()
                    .ok_or(hex_engine::Invalid("stream predecessor unavailable"))?;
                let preview = if let Some(fade_remaining) = request.confirmed_fade {
                    runtime.preview_device_insert(previous, fade_remaining, frames, &mut visit)
                } else if let Some(cut) = &request.tempo_cut {
                    match cut {
                        TempoStreamWindow::Forecast(anchor) => {
                            runtime.forecast_tempo_cut(previous, frames, &mut visit, anchor)
                        }
                        TempoStreamWindow::Exact(anchor) => {
                            runtime.preview_tempo_cut(previous, frames, &mut visit, anchor)
                        }
                    }
                } else if request.fenced_restart {
                    runtime.preview_ordinary_fenced_restart(
                        previous,
                        frames,
                        request.pending_play.then_some(true),
                        &mut visit,
                    )
                } else if request.foreign_continuation {
                    runtime.preview_ordinary_with_stored_foreign_continuation(
                        previous,
                        frames,
                        request.pending_play.then_some(true),
                        &mut visit,
                    )
                } else {
                    runtime.preview_stream_handoff(
                        previous,
                        frames,
                        request.pending_play.then_some(true),
                        &mut visit,
                    )
                };
                match preview {
                    Ok(window) => Ok(window),
                    Err(_) if overflow.get() => {
                        Err(hex_engine::Invalid("exact stream demand capacity exceeded"))
                    }
                    Err(error) if error.is_transient() => {
                        // Preview has the same temporary release/tail pressure
                        // as the actual handoff. Keep the exact candidate in its
                        // deferred carrier and let the predecessor drain before
                        // replanning; this is not a page-readiness rejection.
                        workspace.cancel_pending(refs, request.domains);
                        workspace.handoff_diagnostic = Some(error);
                        return Ok(None);
                    }
                    Err(error) => {
                        workspace.handoff_diagnostic = Some(error);
                        return Err(hex_engine::Invalid("stream handoff demand refused"));
                    }
                }
            } else if let Some(staged) = staged.filter(|_| role == ProtectionRole::Transition) {
                runtime.visit_prepared_stream_action(&staged.prepared, frames, &mut visit)
            } else {
                runtime.visit_next_stream_chunk(
                    frames,
                    request.pending_play.then_some(true),
                    &mut visit,
                )
            };
            if overflow.get() && frames > 1 {
                frames = frames.div_ceil(2);
                continue;
            }
            break planned?;
        };
        if request.frames == 0
            && (request.pending_play
                || workspace
                    .pending_action
                    .as_ref()
                    .is_some_and(|staged| staged.prepared.playing_after_commit()))
        {
            return Ok(None);
        }
        if due.is_none() {
            if workspace.role.is_some_and(|old| old != role) {
                workspace.cancel_pending(refs, request.domains);
            }
            workspace.role = Some(role);
            if role == ProtectionRole::Candidate {
                workspace.candidate_role = CandidateRole::Publication;
            }
        }
        let stamps = if due.is_some() {
            &mut workspace.prewarm_stamps
        } else {
            &mut workspace.stamps
        };
        let mut complete = true;
        for token in refs.into_iter().flatten() {
            let pages = envelope(request.domains, token)?;
            let rows = &workspace.extents[token.slot];
            let loan = pages
                .inner
                .loan
                .as_mut()
                .ok_or(hex_engine::Invalid("stream loan unavailable"))?;
            // Bounded by the caller's actual demand capacity. In particular a
            // worker CapacityExhausted receipt must release its pending key so
            // an old active window can retry after speculative pins clear.
            let mut failed_window = false;
            for _ in 0..workspace.limit {
                let Some((failure, outgoing_revision)) = loan.take_failure_with_outgoing_revision()
                else {
                    break;
                };
                // Pool pressure is retryable after ready pages are pumped and
                // stale buffers returned. Consuming its receipt releases the
                // failed pending key; continue bounded service below. Actual
                // source/read failures still refuse the affected window.
                let demanded = rows
                    .iter()
                    .any(|row| row.source == failure.key && row.page_index == failure.page_index);
                failed_window |=
                    failure.kind != hex_stream::StreamFailureKind::CapacityExhausted && demanded;
                workspace.source_changed |= demanded
                    && failure.kind == hex_stream::StreamFailureKind::SourceChanged
                    && matches!(failure.key, hex_engine::StreamResourceKey::Audio(_));
                workspace.read_failed |= current_read_failure(failure, demanded);
                if let Some(revision) = outgoing_revision
                    && !matches!(
                        failure.kind,
                        hex_stream::StreamFailureKind::CapacityExhausted
                            | hex_stream::StreamFailureKind::Cancelled
                    )
                {
                    pages.inner.outgoing_failure = Some((revision, failure));
                }
                pages.inner.last_failure = Some(failure);
            }
            if failed_window {
                return Err(hex_engine::Invalid("stream worker readiness failed"));
            }
            if let Some(stamp) = stamps[token.slot] {
                let same = stamp.artifact == self.correlation.owner
                    && stamp.project_generation == context.project_generation
                    && stamp.editor_revision == context.editor_revision
                    && stamp.device_epoch == self.correlation.root
                    && stamp.device_rate == rate
                    && stamp.predecessor == predecessor
                    && stamp.admitted_command_prefix == request.command_prefix
                    && stamp.window == window;
                if !same || !loan.matches_resource_intent(role, stamp, rows) {
                    let _ = loan.cancel(role, stamp.request_epoch);
                    stamps[token.slot] = None;
                }
            }
            if stamps[token.slot].is_none() {
                let epoch = pages
                    .inner
                    .epoch
                    .checked_add(1)
                    .ok_or(hex_engine::Invalid("stream request epoch exhausted"))?;
                let stamp = ReadinessStamp {
                    request_epoch: epoch,
                    artifact: self.correlation.owner,
                    project_generation: context.project_generation,
                    editor_revision: context.editor_revision,
                    device_epoch: self.correlation.root,
                    device_rate: rate,
                    predecessor,
                    admitted_command_prefix: request.command_prefix,
                    window,
                };
                #[cfg(all(test, feature = "allocation-probe", feature = "stream-test-support"))]
                if role == ProtectionRole::Candidate && due.is_none() {
                    crate::candidate_window_probe::attempt(window.frames);
                }
                let submitted = loan.submit_resources(role, stamp, rows);
                #[cfg(all(test, feature = "allocation-probe", feature = "stream-test-support"))]
                if role == ProtectionRole::Candidate && due.is_none() && submitted.is_ok() {
                    crate::candidate_window_probe::accepted(token.slot);
                }
                if let Err(refusal) = submitted {
                    #[cfg(all(
                        test,
                        feature = "allocation-probe",
                        feature = "stream-test-support"
                    ))]
                    if role == ProtectionRole::Candidate
                        && due.is_none()
                        && refusal == hex_stream::ReadinessRefusal::Capacity
                    {
                        crate::candidate_window_probe::capacity(window.frames, token.slot);
                    }
                    workspace.capacity_conflict = refusal == hex_stream::ReadinessRefusal::Capacity;
                    if workspace.capacity_conflict
                        && role == ProtectionRole::Candidate
                        && due.is_none()
                        && window.frames > 1
                    {
                        workspace.candidate_frame_limit = window.frames.div_ceil(2);
                    }
                    return Err(hex_engine::Invalid("stream protection reservation refused"));
                }
                pages.inner.epoch = epoch;
                stamps[token.slot] = Some(stamp);
            }
            let stamp =
                stamps[token.slot].ok_or(hex_engine::Invalid("stream intent unavailable"))?;
            if !loan.is_resource_protected(role, stamp, rows) {
                match loan
                    .service(role, stamp, workspace.limit, workspace.limit)
                    .map_err(|_| hex_engine::Invalid("stream readiness service refused"))?
                {
                    ReadinessProgress::Pending => complete = false,
                    ReadinessProgress::InstalledProtected => {
                        if !loan.acknowledge_resources(role, stamp, rows) {
                            return Err(hex_engine::Invalid("stream readiness receipt differs"));
                        }
                    }
                }
            }
        }
        if !complete {
            return Ok(None);
        }
        for token in refs.into_iter().flatten() {
            let stamp =
                stamps[token.slot].ok_or(hex_engine::Invalid("stream stamp unavailable"))?;
            if !envelope(request.domains, token)?
                .inner
                .loan
                .as_ref()
                .is_some_and(|loan| {
                    loan.can_promote_resources(role, stamp, &workspace.extents[token.slot])
                })
            {
                return Err(hex_engine::Invalid("stream protected role cannot commit"));
            }
        }
        if due.is_none() {
            workspace.predecessor = predecessor;
        }
        Ok(Some(window))
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_commit<'a>(
        &mut self,
        domains: &'a mut [Option<DomainEnvelope>; 2],
    ) -> Result<[Option<hex_stream::PreparedRolePromotion<'a>>; 2], hex_engine::Invalid> {
        let workspace = self
            .streaming
            .as_ref()
            .ok_or(hex_engine::Invalid("stream workspace unavailable"))?;
        let role = workspace
            .role
            .ok_or(hex_engine::Invalid("stream role unavailable"))?;
        let (left, right) = domains.split_at_mut(1);
        let prepare = |index: usize, pages: &'a mut Option<DomainEnvelope>| {
            let token = self
                .domain_refs
                .into_iter()
                .flatten()
                .find(|token| token.slot == index);
            let Some(token) = token else {
                return Ok(None);
            };
            let stamp =
                workspace.stamps[index].ok_or(hex_engine::Invalid("stream stamp unavailable"))?;
            let loan = pages
                .as_mut()
                .filter(|pages| pages.inner.token == token)
                .and_then(|pages| pages.inner.loan.as_mut())
                .ok_or(hex_engine::Invalid("stream exact loan unavailable"))?;
            loan.prepare_resource_promotion(role, stamp, &workspace.extents[index])
                .map(Some)
                .ok_or(hex_engine::Invalid("stream protected commit refused"))
        };
        Ok([prepare(0, &mut left[0])?, prepare(1, &mut right[0])?])
    }
    pub(in crate::candidate_custody::eligible::publication) fn record_stream_commit(&mut self) {
        if let Some(workspace) = self.streaming.as_mut() {
            workspace.active = workspace.stamps;
            workspace.stamps = [None; 2];
            if matches!(workspace.candidate_role, CandidateRole::Publication) {
                workspace.candidate_role = CandidateRole::None;
            }
            workspace.role = None;
        }
    }
}

impl StateView<'_> {
    pub(in crate::candidate_custody::eligible::publication) fn clear_stream_roles(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.cancel_stream_pending(domains);
        if let Some(workspace) = self.streaming.as_mut() {
            workspace.page_fade = Default::default();
            for token in self.domain_refs.into_iter().flatten() {
                if let Some(stamp) = workspace.active[token.slot].take()
                    && let Ok(pages) = envelope(domains, token)
                    && let Some(loan) = pages.inner.loan.as_mut()
                {
                    let _ = loan.cancel(ProtectionRole::Active, stamp.request_epoch);
                }
            }
        }
    }
}
struct PagePair<'a> {
    left: Option<hex_stream::ProtectedStreamPages<'a>>,
    right: Option<hex_stream::ProtectedStreamPages<'a>>,
}
impl hex_engine::MediaPages for PagePair<'_> {
    fn page(
        &mut self,
        key: hex_engine::StreamSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        if let Some(page) = self.left.as_mut().and_then(|pages| pages.page(key, index)) {
            return Some(page);
        }
        self.right.as_mut()?.page(key, index)
    }
    fn scalar_page(
        &mut self,
        key: hex_engine::ScalarSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedScalarPage<'_>> {
        if let Some(page) = self
            .left
            .as_mut()
            .and_then(|pages| pages.scalar_page(key, index))
        {
            return Some(page);
        }
        self.right.as_mut()?.scalar_page(key, index)
    }
    fn request_scalar_page(&mut self, _key: hex_engine::ScalarSourceKey, _index: u64) {}
    fn request_page(&mut self, _key: hex_engine::StreamSourceKey, _index: u64) {}
    fn prefetch_page(&mut self, key: hex_engine::StreamSourceKey, index: u64) {
        // Each endpoint ignores sources outside its catalog.
        for pages in [self.left.as_mut(), self.right.as_mut()]
            .into_iter()
            .flatten()
        {
            pages.prefetch_page(key, index);
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamProgress {
    Complete,
    Buffering { committed_frames: usize },
}
pub(crate) struct StreamOutcome {
    pub(crate) progress: StreamProgress,
    pub(crate) play_applied: bool,
    pub(crate) readiness_refused: bool,
    pub(crate) source_changed: bool,
    pub(crate) read_failed: bool,
    pub(crate) fault: Option<hex_engine::Invalid>,
    pub(crate) action_rejection: Option<(u64, hex_engine::PlaybackActionRejection)>,
    pub(crate) action_applied: Option<u64>,
    pub(crate) activated: Option<u64>,
}
impl ActiveView<'_> {
    pub(crate) fn clear_stream_roles(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        self.state.clear_stream_roles(domains);
    }
    fn silence_stream_suffix(&mut self, output: &mut [hex_engine::Frame], waiting_for_pages: bool) {
        output.fill([0.0; 2]);
        if let Some(workspace) = self.state.streaming.as_mut() {
            if waiting_for_pages {
                workspace.page_fade.apply(output, true);
            } else {
                workspace.page_fade = Default::default();
            }
        }
    }

    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn render_streamed(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        command_prefix: u64,
        output: &mut [hex_engine::Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
    ) -> StreamOutcome {
        self.render_streamed_with_foreign(
            domains,
            pending_play,
            command_prefix,
            output,
            observer,
            None,
            None,
            None,
        )
    }
    // The window/transport arguments and borrowed observation lanes have
    // independent owners; foreign endpoints add no stored or adopted resource.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_streamed_with_foreign(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        command_prefix: u64,
        output: &mut [hex_engine::Frame],
        mut observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
        mut project: Option<&mut hex_engine::ProjectOutputTap<'_, '_>>,
        mut foreign: Option<&mut dyn hex_lattice::ForeignProcessors>,
        mut wave: Option<&mut dyn hex_engine::WaveExecutor>,
    ) -> StreamOutcome {
        if let Some(workspace) = self.state.streaming.as_mut() {
            workspace.capacity_conflict = false;
            if pending_play || workspace.pending_action.is_some() {
                workspace.page_fade = Default::default();
            }
        }
        let mut request = StreamInstall {
            domains,
            pending_play: false,
            command_prefix,
            frames: output.len(),
            fenced_restart: false,
            foreign_continuation: false,
            confirmed_fade: None,
            tempo_cut: None,
        };
        let (mut action_applied, mut action_rejection) = match self
            .resolve_stream_action(&mut request)
        {
            super::actions::ActionResolution::Ready(Some((ticket, Ok(())))) => (Some(ticket), None),
            super::actions::ActionResolution::Ready(Some((ticket, Err(reason)))) => {
                (None, Some((ticket, reason)))
            }
            super::actions::ActionResolution::Ready(None) => (None, None),
            super::actions::ActionResolution::Pending => {
                self.silence_stream_suffix(output, false);
                return StreamOutcome {
                    progress: StreamProgress::Buffering {
                        committed_frames: 0,
                    },
                    play_applied: false,
                    readiness_refused: false,
                    source_changed: false,
                    read_failed: false,
                    fault: None,
                    action_rejection: None,
                    action_applied: None,
                    activated: None,
                };
            }
        };
        let mut committed = 0;
        let mut read_failed = false;
        let mut play_applied = false;
        let mut activated = None;
        self.service_launch_prewarm(domains, command_prefix, output.len());
        while committed < output.len() {
            let seek_fade_remaining = self
                .state
                .streaming
                .as_ref()
                .and_then(|workspace| workspace.pending_seek_fade)
                .map(|fade| {
                    usize::try_from(fade.total.saturating_sub(fade.elapsed)).unwrap_or(usize::MAX)
                });
            let chunk_frames = match seek_fade_remaining {
                Some(remaining) => (output.len() - committed).min(remaining.max(1)),
                None => output.len() - committed,
            };
            let mut request = StreamInstall {
                domains,
                pending_play: pending_play && !play_applied,
                command_prefix,
                frames: chunk_frames,
                fenced_restart: false,
                foreign_continuation: false,
                confirmed_fade: None,
                tempo_cut: None,
            };
            let mut planned =
                self.state
                    .prepare_stream_window(None, &mut request, ProtectionRole::Transition);
            let mut source_changed = self
                .state
                .streaming
                .as_mut()
                .is_some_and(|workspace| std::mem::take(&mut workspace.source_changed));
            read_failed |= self
                .state
                .streaming
                .as_mut()
                .is_some_and(|workspace| std::mem::take(&mut workspace.read_failed));
            if !matches!(planned, Ok(Some(_))) && self.revoke_launch_prewarm(request.domains) {
                if let Some(scheduled) = self
                    .state
                    .streaming
                    .as_mut()
                    .and_then(|w| w.scheduled.as_mut())
                {
                    scheduled.pressure_blocked = true;
                }
                // Do not re-pin on a later callback until actual old audio
                // commits; a worker request must get a chance to use the pool.
                // Current audio has priority over revocable future pins. One
                // bounded retry, retaining the actual original PendingRoute due.
                planned = self.state.prepare_stream_window(
                    None,
                    &mut request,
                    ProtectionRole::Transition,
                );
                source_changed |= self
                    .state
                    .streaming
                    .as_mut()
                    .is_some_and(|workspace| std::mem::take(&mut workspace.source_changed));
                read_failed |= self
                    .state
                    .streaming
                    .as_mut()
                    .is_some_and(|workspace| std::mem::take(&mut workspace.read_failed));
            }
            let window = match planned {
                Ok(Some(window)) => window,
                result => {
                    let refused = result.is_err();
                    // Keep partial ready pages under their exact pending intent
                    // across buffering callbacks. Changed demand or commands
                    // invalidate the stamp on the next preparation; failures
                    // still release it immediately.
                    if refused {
                        self.state.cancel_stream_pending(domains);
                    }
                    // A changed file cannot contribute more PCM. Fade only the
                    // last verified output, even if refusal beats the first hold.
                    self.silence_stream_suffix(
                        &mut output[committed..],
                        !pending_play && (source_changed || (!refused && !read_failed)),
                    );
                    let capacity_conflict = self
                        .state
                        .streaming
                        .as_ref()
                        .is_some_and(|workspace| workspace.capacity_conflict);
                    if refused
                        && !capacity_conflict
                        && self
                            .state
                            .streaming
                            .as_ref()
                            .is_some_and(|workspace| workspace.pending_seek_fade.is_some())
                    {
                        match self.complete_pending_seek_fade() {
                            Some(Ok(Some(ticket))) => action_applied = Some(ticket),
                            Some(Err((ticket, reason))) => {
                                action_rejection = Some((ticket, reason));
                            }
                            _ => {}
                        }
                    }
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: refused,
                        source_changed,
                        read_failed,
                        fault: None,
                        action_rejection,
                        action_applied,
                        activated,
                    };
                }
            };
            let mut promotions = match self.state.prepare_stream_commit(domains) {
                Ok(promotions) => promotions,
                Err(_) => {
                    self.silence_stream_suffix(&mut output[committed..], false);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: true,
                        source_changed: false,
                        read_failed,
                        fault: None,
                        action_rejection,
                        action_applied,
                        activated,
                    };
                }
            };
            if pending_play && !play_applied {
                let play = self
                    .state
                    .runtime
                    .as_mut()
                    .ok_or(hex_engine::Invalid("stream current owner missing"))
                    .and_then(|runtime| {
                        runtime
                            .apply_declicked_action(hex_engine::PlaybackAction::Transport(
                                hex_engine::PlaybackCommand::Play,
                            ))
                            .map_err(|_| hex_engine::Invalid("ready stream Play refused"))
                    });
                if let Err(error) = play {
                    self.silence_stream_suffix(&mut output[committed..], false);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: false,
                        source_changed: false,
                        read_failed,
                        fault: Some(error),
                        action_rejection,
                        action_applied,
                        activated,
                    };
                }
                play_applied = true;
            }
            let rendered_frames = if window.frames == 0
                && self
                    .state
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.renders_when_paused())
            {
                output.len() - committed
            } else {
                window.frames
            };
            if rendered_frames == 0 {
                self.silence_stream_suffix(&mut output[committed..], false);
            } else {
                let (left, right) = promotions.split_at_mut(1);
                let mut pages = PagePair {
                    left: left[0].as_mut().map(|p| p.pages()),
                    right: right[0].as_mut().map(|p| p.pages()),
                };
                let mut shifted = observer
                    .as_deref_mut()
                    .map(|inner| hex_engine::ShiftedNodeSignalObserver::new(inner, committed));
                let observer: Option<&mut hex_engine::NodeSignalObserver<'_>> = shifted
                    .as_mut()
                    .map(|obs| obs as &mut hex_engine::NodeSignalObserver<'_>);
                let project_selection = project.as_ref().map(|tap| (tap.tracks(), tap.returns()));
                let mut observe_project = |signal, offset, frames: &[hex_engine::Frame]| {
                    if let Some(project) = project.as_mut() {
                        project.observe(signal, committed + offset, frames);
                    }
                };
                let mut project_tap = project_selection.and_then(|(tracks, returns)| {
                    hex_engine::ProjectOutputTap::with_returns(
                        tracks,
                        returns,
                        &mut observe_project,
                    )
                });
                let rendered = self
                    .state
                    .runtime
                    .as_mut()
                    .ok_or(hex_engine::Invalid("stream current owner missing"))
                    .and_then(|runtime| {
                        let wave = wave
                            .as_mut()
                            .map(|exec| &mut **exec as &mut dyn hex_engine::WaveExecutor);
                        if let Some(foreign) = foreign.as_mut() {
                            let mut window =
                                crate::prepared::NativeEffectWindow::new(&mut **foreign, committed);
                            runtime.render_with_foreign_processors_and_wave(
                                &mut output[committed..committed + rendered_frames],
                                Some(&mut pages),
                                observer,
                                project_tap.as_mut(),
                                &mut window,
                                wave,
                            )
                        } else {
                            runtime.render_with_pages_and_signal_observers_and_wave(
                                &mut output[committed..committed + rendered_frames],
                                &mut pages,
                                observer,
                                project_tap.as_mut(),
                                wave,
                            )
                        }
                    });
                if let Err(error) = rendered {
                    self.silence_stream_suffix(&mut output[committed..], false);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: false,
                        source_changed: false,
                        read_failed,
                        fault: Some(error),
                        action_rejection,
                        action_applied,
                        activated,
                    };
                }
                if let Some(workspace) = self.state.streaming.as_mut() {
                    let rendered_slice = &mut output[committed..committed + rendered_frames];
                    if window.frames != 0 {
                        workspace.page_fade.apply(rendered_slice, false);
                    } else {
                        workspace.page_fade = Default::default();
                    }
                    if let Some(fade) = workspace.pending_seek_fade.as_mut() {
                        let total = fade.total.max(1);
                        let scale = 1.0 / total as f32;
                        for frame in rendered_slice.iter_mut() {
                            fade.elapsed = fade.elapsed.saturating_add(1).min(total);
                            let gain = (total - fade.elapsed) as f32 * scale;
                            frame[0] *= gain;
                            frame[1] *= gain;
                        }
                        if fade.elapsed >= total {
                            workspace.page_fade = Default::default();
                        } else if let Some(&last) = rendered_slice.last() {
                            workspace.page_fade.sync_last(last);
                        }
                    }
                }
                committed += rendered_frames;
            }
            for promotion in promotions.into_iter().flatten() {
                promotion.commit();
            }
            self.state.record_stream_commit();
            if window.frames != 0 {
                if let Some(scheduled) = self
                    .state
                    .streaming
                    .as_mut()
                    .and_then(|w| w.scheduled.as_mut())
                {
                    scheduled.pressure_blocked = false;
                }
                activated = self.observe_launch_activation(domains).or(activated);
            }
            if self
                .state
                .streaming
                .as_ref()
                .and_then(|workspace| workspace.pending_seek_fade)
                .is_some_and(|fade| fade.elapsed >= fade.total)
            {
                match self.complete_pending_seek_fade() {
                    Some(Ok(Some(ticket))) => action_applied = Some(ticket),
                    Some(Err((ticket, reason))) => action_rejection = Some((ticket, reason)),
                    Some(Ok(None)) if committed < output.len() => {
                        let mut resolve_request = StreamInstall {
                            domains,
                            pending_play: false,
                            command_prefix,
                            frames: output.len() - committed,
                            fenced_restart: false,
                            foreign_continuation: false,
                            confirmed_fade: None,
                            tempo_cut: None,
                        };
                        match self.resolve_stream_action(&mut resolve_request) {
                            super::actions::ActionResolution::Ready(Some((ticket, Ok(())))) => {
                                action_applied = Some(ticket);
                            }
                            super::actions::ActionResolution::Ready(Some((
                                ticket,
                                Err(reason),
                            ))) => {
                                action_rejection = Some((ticket, reason));
                            }
                            super::actions::ActionResolution::Ready(None) => {}
                            super::actions::ActionResolution::Pending => {
                                self.silence_stream_suffix(&mut output[committed..], false);
                                return StreamOutcome {
                                    progress: StreamProgress::Buffering {
                                        committed_frames: committed,
                                    },
                                    play_applied,
                                    readiness_refused: false,
                                    source_changed: false,
                                    read_failed,
                                    fault: None,
                                    action_rejection,
                                    action_applied,
                                    activated,
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
            if window.frames == 0 {
                break;
            }
        }
        StreamOutcome {
            progress: StreamProgress::Complete,
            play_applied,
            readiness_refused: false,
            source_changed: false,
            read_failed,
            fault: None,
            action_rejection,
            action_applied,
            activated,
        }
    }
}
