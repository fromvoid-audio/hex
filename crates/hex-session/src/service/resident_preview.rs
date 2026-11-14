//! One worker-private audible overlay. Actual source/configuration/factory custody participates
//! in every control union; clip groups retain a bounded shared control identity.
use super::*;
use crate::device_gesture::{GesturePreparation, command_for_snapshot};
use hex_engine::native_device_resources::DeviceResourceAdmission;
pub(super) mod plugin_parameter;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Identity {
    Device(DevicePreviewIdentity),
    Tuning(TuningPreviewIdentity),
    Clip(clip_audition::Target),
}
pub(crate) struct Preview {
    pub(super) project: Arc<Project>,
    pub(super) snapshot: resident_audio::Snapshot,
    pub(super) identity: Identity,
}
impl Preview {
    /// A newer canonical preparation can supersede an overlay before rebase.
    pub(super) fn follows(&self, canonical: &resident_audio::Snapshot) -> bool {
        let overlay = self.snapshot.prepared_context;
        let canonical = canonical.prepared_context;
        overlay.project_generation == canonical.project_generation
            && overlay.editor_revision >= canonical.editor_revision
    }

    pub(super) fn device_identity(&self) -> Option<DevicePreviewIdentity> {
        match &self.identity {
            Identity::Device(id) => Some(*id),
            _ => None,
        }
    }
    pub(super) fn tuning_identity(&self) -> Option<TuningPreviewIdentity> {
        match &self.identity {
            Identity::Tuning(id) => Some(*id),
            _ => None,
        }
    }
    pub(super) fn clip_target(&self) -> Option<clip_audition::Target> {
        match &self.identity {
            Identity::Clip(target) => Some(target.clone()),
            _ => None,
        }
    }
    pub(super) fn clip_move_identity(&self) -> Option<ClipMovePreviewIdentity> {
        self.clip_target()?.move_identity()
    }
    pub(super) fn admission(&self) -> DeviceResourceAdmission<'_> {
        self.snapshot.admission()
    }
    /// Native accounting covers the resource/factory union and conservative
    /// input/control capture, including a retained clip selection. The shared
    /// Project remains in the existing bounded control-model/ViewState scope,
    /// not a claim of whole-model allocation provenance.
    pub(super) fn control_bytes(&self) -> Result<usize, String> {
        self.snapshot
            .inputs
            .capture_bytes()
            .map_err(|e| e.to_string())?
            .checked_add(std::mem::size_of::<Self>())
            .and_then(|bytes| {
                bytes.checked_add(match &self.identity {
                    Identity::Clip(target) => target.retained_bytes(),
                    _ => 0,
                })
            })
            .ok_or_else(|| "preview metadata overflow".into())
    }
}
impl Identity {
    pub(super) fn source_origin(&self) -> ResidentSourceOrigin {
        match self {
            Self::Device(_) => ResidentSourceOrigin::DevicePreview,
            Self::Tuning(_) => ResidentSourceOrigin::TuningPreview,
            Self::Clip(clip_audition::Target::Move(_)) => ResidentSourceOrigin::ClipMovePreview,
            Self::Clip(clip_audition::Target::Stretch(_)) => {
                ResidentSourceOrigin::ClipStretchPreview
            }
            Self::Clip(clip_audition::Target::PlayStart(_)) => {
                ResidentSourceOrigin::ClipPlayStartPreview
            }
        }
    }
}
fn metadata(
    context: hex_engine::AudioSourceContext,
    identity: Option<Identity>,
    prepared: &PreparedRevision,
) -> ResidentPublicationMetadata {
    ResidentPublicationMetadata {
        source_origin: crate::service::ResidentSourceOrigin::Unknown,
        clip_preview: identity.as_ref().and_then(|id| match id {
            Identity::Clip(target) => target.move_identity(),
            _ => None,
        }),
        context,
        device_preview: identity.as_ref().and_then(|id| match id {
            Identity::Device(id) => Some(*id),
            _ => None,
        }),
        tuning_preview: identity.as_ref().and_then(|id| match id {
            Identity::Tuning(id) => Some(*id),
            _ => None,
        }),
        seek_request: None,
        stopped_restore: false,
        computer_keyboard_request: None,
        gain_recording_track: prepared.gain_recording_track(),
        requires_preroll: prepared.arrangement_seek_requires_preroll(),
    }
}
#[allow(clippy::too_many_arguments)] // One canonical worker's actual current/preview owners and final publication hook.
pub(super) fn device_operation(
    session: &mut Session,
    operation: GesturePreparation,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
) -> crate::device_gesture::PreparedGesture {
    match operation {
        GesturePreparation::Plugin(operation) => plugin_parameter::operation(
            session, operation, context, current, preview, sink, admission,
        ),
        GesturePreparation::Preview(draft) => {
            let identity = Identity::Device(DevicePreviewIdentity {
                project_generation: context.project_generation,
                canonical_revision: context.editor_revision,
                key: draft.key,
                sequence: draft.sequence,
            });
            prepare_preview(
                session,
                command_for_snapshot(session, draft)?,
                identity,
                context,
                current,
                preview,
                sink,
                admission,
                None,
            )
        }
        GesturePreparation::Commit(draft) => commit_preview(
            session,
            command_for_snapshot(session, draft)?,
            context,
            current,
            preview,
            sink,
            admission,
        ),
        GesturePreparation::Restore => restore(session, context, current, preview, sink, admission),
    }
}
#[allow(clippy::too_many_arguments)] // Same owner/admission boundary as ordinary device gestures.
pub(super) fn tuning_operation(
    session: &mut Session,
    operation: crate::tuning_gesture::TuningPreparation,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
) -> crate::device_gesture::PreparedGesture {
    use crate::tuning_gesture::TuningPreparation;
    match operation {
        TuningPreparation::Preview(draft) => prepare_preview(
            session,
            Command::SetTuning(draft.tuning),
            Identity::Tuning(TuningPreviewIdentity {
                project_generation: context.project_generation,
                canonical_revision: context.editor_revision,
                key: draft.key,
                sequence: draft.sequence,
            }),
            context,
            current,
            preview,
            sink,
            admission,
            None,
        ),
        TuningPreparation::Commit(draft) => commit_preview(
            session,
            Command::SetTuning(draft.tuning),
            context,
            current,
            preview,
            sink,
            admission,
        ),
        TuningPreparation::Restore => device_operation(
            session,
            GesturePreparation::Restore,
            context,
            current,
            preview,
            sink,
            admission,
        ),
    }
}

pub(super) fn restore(
    session: &Session,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
) -> crate::device_gesture::PreparedGesture {
    let snapshot = current
        .as_mut()
        .ok_or(SessionError::Invalid("canonical playback unavailable"))?;
    let admission = with_preview(admission, preview.as_ref());
    if admission.preview.is_some_and(|overlay| {
        overlay.follows(snapshot) && overlay.snapshot.prepared.has_arrangement_foreign_programs()
    }) {
        *snapshot = session
            .restore_resident_preview_with_foreign_programs(snapshot, sink, admission, context)?;
    } else {
        let selected = snapshot.inputs.clone();
        session.publish_resident_runtime_inputs(
            snapshot,
            sink,
            admission,
            metadata(context, None, &snapshot.prepared),
            ResidentPublicationPolicy::InheritRuntime,
            selected,
            None,
        )?;
    }
    let prepared = Arc::clone(&snapshot.prepared);
    *preview = None;
    Ok(Some((Arc::new(session.project().clone()), prepared)))
}

pub(super) fn clip_audition(
    session: &Session,
    target: clip_audition::Target,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    cancel: &Arc<AtomicBool>,
) -> crate::device_gesture::PreparedGesture {
    session
        .audio_transforms
        .with_preview_cancellation(cancel, || {
            prepare_preview(
                session,
                target.command(),
                Identity::Clip(target.clone()),
                target.context(),
                current,
                preview,
                sink,
                admission,
                Some(cancel),
            )
        })
}
#[allow(clippy::too_many_arguments)] // Last fallible publication remains inside the reserved canonical transaction.
fn commit_preview(
    session: &mut Session,
    command: Command,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
) -> crate::device_gesture::PreparedGesture {
    let next = context
        .editor_revision
        .checked_add(1)
        .ok_or(SessionError::Invalid("preview revision exhausted"))?;
    let (_, snapshot) = session.command_resident_snapshot(
        command,
        hex_engine::AudioSourceContext {
            editor_revision: next,
            ..context
        },
        sink,
        with_preview(admission, preview.as_ref()),
        current.as_ref(),
    )?;
    let prepared = Arc::clone(&snapshot.prepared);
    *current = Some(snapshot);
    *preview = None;
    Ok(Some((Arc::new(session.project().clone()), prepared)))
}
#[allow(clippy::too_many_arguments)] // One finite overlay editor and the actual publication/source union.
fn prepare_preview(
    session: &Session,
    command: Command,
    identity: Identity,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    cancel: Option<&AtomicBool>,
) -> crate::device_gesture::PreparedGesture {
    let canonical = current
        .as_mut()
        .ok_or(SessionError::Invalid("canonical playback unavailable"))?;
    let mut admission = with_preview(admission, preview.as_ref());
    admission.runtime_bytes = admission
        .runtime_bytes
        .checked_add(
            canonical
                .inputs
                .capture_bytes()?
                .checked_add(std::mem::size_of::<Preview>())
                .and_then(|bytes| {
                    bytes.checked_add(match &identity {
                        Identity::Clip(target) => target.retained_bytes(),
                        _ => 0,
                    })
                })
                .ok_or(SessionError::Invalid("preview metadata overflow"))?,
        )
        .ok_or(SessionError::Invalid("preview metadata overflow"))?;
    let (project, snapshot) = session.prepare_resident_preview(
        command,
        context,
        identity.clone(),
        sink,
        admission,
        canonical,
        cancel,
    )?;
    let prepared = Arc::clone(&snapshot.prepared);
    canonical.inputs = snapshot.inputs.clone();
    *preview = Some(Preview {
        project: Arc::clone(&project),
        snapshot,
        identity,
    });
    Ok(Some((project, prepared)))
}
/// Attach monitor inputs to the actual audible overlay, keeping canonical selection/version
/// synchronized only after acceptance. The selected target is visited as the candidate exactly
/// once; the canonical set is an additional real owner while an overlay is active.
#[allow(clippy::too_many_arguments)] // Exact current/preview owners and final publication are borrowed together.
pub(super) fn publish_inputs(
    session: &Session,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    mut metadata: ResidentPublicationMetadata,
    policy: ResidentPublicationPolicy,
    selected: resident_audio::RuntimeInputs,
    required: Option<resident_audio::RequiredInput>,
) -> Result<Option<hex_engine::computer_keyboard::ComputerKeyboardTuningHint>, SessionError> {
    let canonical = current
        .as_mut()
        .ok_or(SessionError::Invalid("native input source unavailable"))?;
    let Some(overlay) = preview.as_mut() else {
        return session.publish_resident_runtime_inputs(
            canonical, sink, admission, metadata, policy, selected, required,
        );
    };
    let registered = admission
        .registered
        .iter()
        .copied()
        .chain(std::iter::once(canonical.admission()))
        .collect::<Vec<_>>();
    let runtime_bytes = admission
        .runtime_bytes
        .checked_add(overlay.control_bytes().map_err(SessionError::NativeAudio)?)
        .and_then(|bytes| bytes.checked_add(selected.capture_bytes().ok()?))
        .ok_or(SessionError::Invalid("preview input metadata overflow"))?;
    metadata.device_preview = overlay.device_identity();
    metadata.tuning_preview = overlay.tuning_identity();
    metadata.clip_preview = overlay.clip_move_identity();
    metadata.requires_preroll = overlay
        .snapshot
        .prepared
        .arrangement_seek_requires_preroll();
    metadata.gain_recording_track = overlay.snapshot.prepared.gain_recording_track();
    let (hint, accepted) = session.prepare_resident_runtime_inputs(
        &overlay.snapshot,
        &overlay.project,
        sink,
        sampler_export::Admission {
            registered: &registered,
            runtime_bytes,
            ..admission
        },
        metadata,
        policy,
        selected,
        required,
    )?;
    drop(registered);
    accepted.cancel_retired_from(&canonical.inputs);
    canonical.inputs = accepted.clone();
    overlay.snapshot.inputs = accepted;
    Ok(hint)
}
fn with_preview<'a>(
    admission: sampler_export::Admission<'a>,
    preview: Option<&'a Preview>,
) -> sampler_export::Admission<'a> {
    sampler_export::Admission {
        preview,
        runtime_bytes: admission.runtime_bytes.saturating_add(
            preview
                .map_or(Ok(0), Preview::control_bytes)
                .unwrap_or(usize::MAX),
        ),
        ..admission
    }
}
