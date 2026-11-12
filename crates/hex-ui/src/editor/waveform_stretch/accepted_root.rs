//! Invert the accepted stretch root without constructing a view.
use hex_project::{Clip, TempoMap};

/// Invert only the canonical stretch's root-clock retiming. Placement wrap and
/// visibility belong to AudioViewProjection, not this source-coordinate law.
pub(in crate::editor) fn accepted_root_frame(
    original: &Clip,
    preview: &Clip,
    tempo: &TempoMap,
    prepared: bool,
    beat: u64,
) -> Option<u64> {
    let edit = preview.audio_edit()?;
    if let Some(old) = original.audio_edit().filter(|e| e.domain.warp_enabled) {
        // The stretch operation retimes all existing beat coordinates together.
        // Preserve the accepted processed signal, including its transients,
        // pitch, source envelope and loop history, rather than using raw PCM.
        let old_position = u64::try_from(
            u128::from(beat) * u128::from(old.domain.clock.length())
                / u128::from(edit.domain.clock.length()),
        )
        .ok()?;
        return old.root_frame_at(old_position, tempo).ok();
    }
    let phase = edit.domain.warp_phase_beat().ok()?;
    let source_q32 = edit
        .domain
        .warp_source_position_q32(u128::from(beat) + u128::from(phase))
        .ok()?;
    let source = u64::try_from(source_q32 >> 32).ok()?;
    if prepared {
        // A warp-off processed root advances through source frames in order.
        // Its accepted peaks already contain pitch/envelope processing. Do
        // not substitute raw peaks and thereby erase that processing.
        Some(source.saturating_sub(edit.domain.source_phase))
    } else {
        Some(source)
    }
}
