//! Captured edit intent validation. The native sink owns the callback fence and
//! sealed pending cell; canonical state/history commit stays with Session.
use super::*;
pub(super) fn validate(
    session: &Session,
    generation: u64,
    revision: u64,
    edit: &SamplerParameterEdit,
) -> Result<(), String> {
    if edit.request_id == 0
        || edit.project_generation != generation
        || edit.revision != revision
        || edit.cancel.load(Ordering::Acquire)
    {
        return Err("sampler edit target changed or edit was cancelled".into());
    }
    let device = session
        .project()
        .track(edit.track)
        .and_then(|t| t.instrument())
        .filter(|d| d.id() == edit.device && !d.bypassed())
        .ok_or("sampler edit target is unavailable")?;
    hex_engine::patch_sampler_parameter_on_control(edit.parameter, device.state(), edit.value)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
