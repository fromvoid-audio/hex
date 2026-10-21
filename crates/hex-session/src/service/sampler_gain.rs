//! Captured gain intent validation. The native sink owns the callback fence and
//! sealed pending cell; canonical state/history commit stays with Session.
use super::*;
pub(super) fn validate(
    session: &Session,
    generation: u64,
    revision: u64,
    gain: &SamplerGainEdit,
) -> Result<(), String> {
    if gain.request_id == 0
        || gain.project_generation != generation
        || gain.revision != revision
        || gain.cancel.load(Ordering::Acquire)
    {
        return Err("sampler gain target changed or edit was cancelled".into());
    }
    let device = session
        .project()
        .track(gain.track)
        .and_then(|t| t.instrument())
        .filter(|d| d.id() == gain.device && !d.bypassed())
        .ok_or("sampler gain target is unavailable")?;
    hex_engine::patch_sampler_gain_on_control(device.state(), gain.gain)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
