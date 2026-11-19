//! Bind the eligible Arrangement waveform to the same original source in
//! canonical, held and pending phases. Input and edit receipts keep their owners.
use super::{
    EditorView, waveform, waveform_frame_source::Projection, waveform_geometry::WaveformMapping,
};
use gpui::Div;
use hex_project::Clip;
use hex_session::service::ViewState;
use std::sync::Arc;

impl EditorView {
    pub(super) fn frame_source_audio_clip_body(
        &self,
        state: &ViewState,
        clip: &Clip,
        visible: std::ops::Range<u64>,
        grid: Option<waveform::WaveformGrid>,
        ink: u32,
    ) -> Option<Div> {
        let source = state.waveforms.get(&clip.asset())?;
        let asset = state.project.asset(clip.asset())?;
        let trim = self.fade_trim_preview(clip.id());
        let displayed = trim.unwrap_or(clip);
        // A lower handle's prospective Clip already owns its full trim/fade law.
        let fade = trim
            .is_none()
            .then(|| self.fade_preview(clip.id()))
            .flatten();
        let gain = self.waveform_clip_gain(displayed);
        let events = self.arrangement_fade_event_preview(clip.id(), state);
        let projection = Arc::new(Projection::new(
            displayed,
            asset,
            state.project.tempo_map(),
            source,
            fade,
            events.as_ref(),
            gain,
        )?);
        let fallback = if let Some(prepared) = state.audio_clip_waveforms.get(&clip.id()) {
            let offset = trim
                .and_then(|clip| clip.audio_edit())
                .and_then(|edit| edit.projected_domain(state.project.tempo_map()).ok())
                .map_or(prepared.offset, |(_, _, offset, _)| offset);
            (
                Arc::clone(&prepared.waveform),
                WaveformMapping::Prepared {
                    timeline_origin: displayed.start(),
                    root_offset: offset,
                    gain: gain.unwrap_or(displayed.gain()),
                },
            )
        } else {
            (
                Arc::clone(source),
                WaveformMapping::Clip {
                    clip: displayed.clone(),
                    timeline_origin: displayed.start(),
                    gain: gain.map(hex_project::ClipGain::linear),
                    fade: Some(fade.unwrap_or(displayed.retained_fade())),
                },
            )
        };
        Some(waveform::frame_source_waveform_view_cached(
            Arc::clone(source),
            projection,
            fallback,
            visible,
            std::rc::Rc::clone(&self.waveform_geometry_cache),
            grid,
            ink,
        ))
    }
}
