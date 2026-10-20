//! Scoped retained/prospective warp projection using the canonical marker law.
use super::*;

/// Projection metadata cannot leave its scoped read callback.
/// ```compile_fail
/// fn escape(edit: &hex_project::AudioClipEdit, tempo: &hex_project::TempoMap) {
///     let _escaped = edit.with_warp_map_read(tempo, |read| read);
/// }
/// ```
pub struct AudioWarpMapRead<'a> {
    source: Option<&'a AudioClipEdit>,
    edit: EditPlan,
    tempo: &'a TempoMap,
    source_offset: u64,
    source_length: u64,
    source_phase: u64,
    looping: bool,
    pitch_cents: i16,
}
impl AudioClipEdit {
    pub fn with_warp_map_read<R>(
        &self,
        tempo: &TempoMap,
        inspect: impl for<'read> FnOnce(AudioWarpMapRead<'read>) -> R,
    ) -> R {
        inspect(AudioWarpMapRead {
            source: Some(self),
            edit: EditPlan::original(self),
            tempo,
            source_offset: self.domain.source_offset,
            source_length: self.domain.source_length,
            source_phase: self.domain.source_phase,
            looping: self.domain.looping,
            pitch_cents: self.domain.pitch_cents,
        })
    }
}
impl AudioClipPreviewRead<'_> {
    /// The prospective mapping, not the currently prepared endpoint. None is
    /// exactly the allocating preview's absent edit; no model is materialized.
    pub fn with_warp_map_read<R>(
        &self,
        inspect: impl for<'read> FnOnce(Option<AudioWarpMapRead<'read>>) -> R,
    ) -> R {
        inspect(self.plan.edit.map(|edit| {
            let source = self.clip.audio_edit();
            AudioWarpMapRead {
                source,
                edit,
                tempo: self.tempo,
                source_offset: source.map_or(self.clip.source_offset, |s| s.domain.source_offset),
                source_length: source.map_or(self.clip.source_length, |s| s.domain.source_length),
                source_phase: source.map_or(self.clip.source_phase, |s| s.domain.source_phase),
                looping: source.map_or(self.clip.looping, |s| s.domain.looping),
                pitch_cents: source.map_or(0, |s| s.domain.pitch_cents),
            }
        }))
    }
}
impl AudioWarpMapRead<'_> {
    pub fn projected_domain(&self) -> Result<(i128, u64, u64, u64), Invalid> {
        self.edit.projected_domain(self.tempo)
    }
    pub fn clock(&self) -> AudioDomainClock {
        self.edit.clock
    }
    pub fn warp_enabled(&self) -> bool {
        self.edit.warp_enabled
    }
    pub fn source_length(&self) -> u64 {
        self.source_length
    }
    pub fn source_phase(&self) -> u64 {
        self.source_phase
    }
    pub fn looping(&self) -> bool {
        self.looping
    }
    pub fn pitch_cents(&self) -> i16 {
        self.pitch_cents
    }
    pub fn projection_origin(&self) -> Result<i128, Invalid> {
        self.edit.projection_origin()
    }
    pub fn project_frame(&self, beat: i128) -> Result<i128, Invalid> {
        audio_projection_frame(self.tempo, beat)
    }
    pub fn tempo_points(&self) -> impl ExactSizeIterator<Item = crate::TempoPoint> + '_ {
        self.tempo.points().iter().copied()
    }
    pub fn marker_count(&self) -> Result<usize, Invalid> {
        Ok(self.edit.warp_read(self.source)?.len())
    }
    pub fn marker(&self, index: usize) -> Result<WarpMarker, Invalid> {
        self.edit.warp_read(self.source)?.marker(index)
    }
    pub fn warp_mode(&self) -> Result<WarpMode, Invalid> {
        Ok(self.edit.warp_read(self.source)?.mode)
    }
    pub fn phase_beat(&self) -> Result<u64, Invalid> {
        crate::audio_edit::warp_projection::beat_for_source_frame(
            self.marker_count()?,
            |i| self.marker(i),
            self.source_offset
                .checked_add(self.source_phase)
                .ok_or(Invalid("warp phase overflow"))?,
        )
        .map(BeatTime::raw)
    }
    pub fn source_position_q32(&self, beat: u128) -> Result<i128, Invalid> {
        crate::audio_edit::warp_projection::source_position_q32(
            self.marker_count()?,
            |i| self.marker(i),
            self.source_offset,
            self.source_length,
            self.looping,
            beat,
        )
    }
}
