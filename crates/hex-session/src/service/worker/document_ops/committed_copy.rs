//! A copy of the committed document leaves its open file and parameter preview in custody.
use super::*;
use crate::device_gesture::{GestureKey, GestureTarget};

pub(super) fn save(
    ctx: &mut WorkerDispatch<'_>,
    path: PathBuf,
    revision: u64,
    key: GestureKey,
    sequence: u64,
) -> Result<PathBuf, String> {
    let worker = &mut ctx.worker;
    let matches_preview = worker.device_gesture.snapshot().is_some_and(|active| {
        active.key == key
            && active.sequence == sequence
            && matches!(
                active.target,
                GestureTarget::DeviceParameter { .. }
                    | GestureTarget::DeviceParameterPair { .. }
                    | GestureTarget::DeviceParameterSnapshot { .. }
            )
    });
    if key.generation != worker.project_generation
        || revision != worker.revision
        || !matches_preview
    {
        return Err("project or pending parameter changed · choose the copy again".into());
    }
    if !worker.plugin_recording.close_safe() || !worker.native_automation_take.close_safe() {
        return Err("finish or recover the automation take before saving a copy".into());
    }
    if worker.tuning_gesture.snapshot().is_some()
        || worker.pending_sampler_parameter.is_some()
        || worker.pending_plugin_effect.is_some()
        || worker.pending_device_insert.is_some()
    {
        return Err("finish the other pending device edit before saving a copy".into());
    }
    let path = absolute_document_path(&path)?;
    if worker
        .document
        .as_ref()
        .is_some_and(|document| is_open_document(document, &path))
    {
        return Err("the copy needs a new filename · choose a different path".into());
    }
    let cancellation = RequestCancellation::new(
        Arc::clone(&worker.session.shutdown),
        Arc::clone(&worker.document_intent),
        ctx.outcome.request_document_intent,
    );
    if cancellation.is_cancelled() {
        return Err("project changed · copy cancelled".into());
    }
    let project = plugin_state_save::project_for_save_with_capture(
        &worker.session,
        worker.media_base.as_deref(),
        hex_engine::AudioSourceContext {
            project_generation: worker.project_generation,
            editor_revision: revision,
        },
        &cancellation,
        worker.events.sampler_edit_audio(),
    )?;
    if cancellation.is_cancelled() {
        return Err("project changed · copy cancelled".into());
    }
    project_save::save_copy(path, project, worker.recovery.as_deref(), &cancellation)
}
