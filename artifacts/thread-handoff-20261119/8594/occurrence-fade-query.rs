// Artifact-only proposal, not applied, compiled or verified. Place beside the
// AudioViewProjection owner; child modules can use ProjectedAudioFade's private
// construction fields without a public constructor or a cloned processing root.
impl AudioViewProjection<'_> {
    /// Borrow the authored fade for one forward finite processing occurrence.
    /// Frames remain root-output frames, even when the retained clock is beats.
    /// No allocation, canonical mutation, DSP work or source custody transfer.
    /// Directed content needs its distinct forward destination-fade clock.
    pub fn occurrence_fade(&self, index: u64) -> Result<ProjectedAudioFade<'_>, Invalid> {
        let edit = self
            .clip
            .audio_edit()
            .ok_or(Invalid("audio clip has no projected fade"))?;
        if edit.content_view.is_some() {
            return Err(Invalid(
                "directed content needs its forward destination fade projection",
            ));
        }
        let occurrence = self.occurrence(index)?;
        let tempo = edit.processing_tempo(self.tempo);
        let (origin, first) = match edit.domain.clock {
            AudioDomainClock::Frames(_) => (0, 0),
            AudioDomainClock::Beats(_) => (
                occurrence.root_origin,
                audio_projection_frame(tempo, occurrence.root_origin)?,
            ),
        };
        Ok(ProjectedAudioFade {
            edit,
            tempo,
            first,
            origin,
            length: occurrence.root_frame_length,
        })
    }
}

// This canonical-only query intentionally does NOT claim held-preview support.
// A complete musical UI consumer must use the setter's captured preview law too.
// That may require a bounded borrowed fade-reference context (shape + source
// start + optional borrowed tempo/origin), preserving the existing gain/knots
// formulas; do not clone a whole edit/tempo map per point or cast beats to frames.
