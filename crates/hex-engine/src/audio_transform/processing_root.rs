//! Private source-processing identity, excluding only an Outer instance fade.
//! This carrier is neither a canonical edit nor destination playback output.
//! Source/content/event/retained-child data is borrowed unchanged. Legacy keeps
//! its exact root envelope and key; only explicit Outer metadata is normalized.
use super::{AudioClipEdit, Invalid};
use hex_project::{AudioClipFadeClock, AudioFadeProjection, ClipFade, TempoMap};
use std::{borrow::Cow, sync::Arc};

pub(super) fn unfaded_outer_edit<'a>(
    edit: &'a AudioClipEdit,
    tempo: &TempoMap,
) -> Result<Cow<'a, AudioClipEdit>, Invalid> {
    if edit.domain.fade_clock != AudioClipFadeClock::Outer {
        return Ok(Cow::Borrowed(edit));
    }
    let (_, length, _, _) = edit.projected_domain(tempo)?;
    // The neutral legacy carrier still has a valid clock-shaped projection.
    // Derive it from source processing, never the ignored Outer author map.
    let projection = if edit.reference_start_raw().is_some() {
        Some(AudioFadeProjection {
            tempo: edit.processing_tempo(tempo).clone(),
            origin_beat_raw: edit.processing_origin()?,
            local_start_raw: 0,
        })
    } else {
        None
    };
    let mut processing = edit.clone();
    let domain = Arc::make_mut(&mut processing.domain);
    domain.fade = ClipFade::neutral(length)?;
    domain.fade_clock = AudioClipFadeClock::LegacyRoot;
    domain.fade_start = 0;
    domain.fade_projection = projection;
    Ok(Cow::Owned(processing))
}

#[cfg(test)]
mod tests;
