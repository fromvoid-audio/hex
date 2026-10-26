//! One worker-private audible overlay. Actual source/configuration/factory custody participates
//! in every control union; UI sees only its existing read-only view and exact scalar identity.
use super::*;
use crate::device_gesture::{GesturePreparation, command_for_snapshot};
use hex_engine::native_device_resources::DeviceResourceAdmission;
mod plugin_parameter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Identity {
    Device(DevicePreviewIdentity),
    Tuning(TuningPreviewIdentity),
}
pub(crate) struct Preview {
    pub(super) project: Arc<Project>,
    pub(super) snapshot: resident_audio::Snapshot,
    pub(super) identity: Identity,
}
impl Preview {
    pub(super) fn device_identity(&self) -> Option<DevicePreviewIdentity> {
        match self.identity {
            Identity::Device(id) => Some(id),
            _ => None,
        }
    }
    pub(super) fn tuning_identity(&self) -> Option<TuningPreviewIdentity> {
        match self.identity {
            Identity::Tuning(id) => Some(id),
            _ => None,
        }
    }
    pub(super) fn admission(&self) -> DeviceResourceAdmission<'_> {
        DeviceResourceAdmission::prepared_on_control(
            &self.snapshot.resources,
            &self.snapshot.prepared,
        )
    }
    /// Native accounting covers the actual resource/factory union and this conservative fixed
    /// input/control capture. The shared read-only Project remains in the existing bounded
    /// control-model/ViewState scope, not a claim of whole-model allocation provenance.
    pub(super) fn control_bytes(&self) -> Result<usize, String> {
        self.snapshot
            .inputs
            .capture_bytes()
            .map_err(|e| e.to_string())?
            .checked_add(std::mem::size_of::<Self>())
            .ok_or_else(|| "preview metadata overflow".into())
    }
}
fn metadata(
    context: hex_engine::AudioSourceContext,
    identity: Option<Identity>,
    prepared: &PreparedRevision,
) -> ResidentPublicationMetadata {
    ResidentPublicationMetadata {
        context,
        device_preview: identity.and_then(|id| match id {
            Identity::Device(id) => Some(id),
            Identity::Tuning(_) => None,
        }),
        tuning_preview: identity.and_then(|id| match id {
            Identity::Tuning(id) => Some(id),
            Identity::Device(_) => None,
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
        GesturePreparation::Restore => {
            let snapshot = current
                .as_mut()
                .ok_or(SessionError::Invalid("canonical playback unavailable"))?;
            let selected = snapshot.inputs.clone();
            let admission = with_preview(admission, preview.as_ref());
            session.publish_resident_runtime_inputs(
                snapshot,
                sink,
                admission,
                metadata(context, None, &snapshot.prepared),
                ResidentPublicationPolicy::InheritRuntime,
                selected,
                None,
            )?;
            let prepared = Arc::clone(&snapshot.prepared);
            *preview = None;
            Ok(Some((Arc::new(session.project().clone()), prepared)))
        }
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
                .ok_or(SessionError::Invalid("preview metadata overflow"))?,
        )
        .ok_or(SessionError::Invalid("preview metadata overflow"))?;
    let (project, snapshot) =
        session.prepare_resident_preview(command, context, identity, sink, admission, canonical)?;
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
        .chain(std::iter::once(
            DeviceResourceAdmission::prepared_on_control(&canonical.resources, &canonical.prepared),
        ))
        .collect::<Vec<_>>();
    let runtime_bytes = admission
        .runtime_bytes
        .checked_add(overlay.control_bytes().map_err(SessionError::NativeAudio)?)
        .and_then(|bytes| bytes.checked_add(selected.capture_bytes().ok()?))
        .ok_or(SessionError::Invalid("preview input metadata overflow"))?;
    metadata.device_preview = overlay.device_identity();
    metadata.tuning_preview = overlay.tuning_identity();
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
