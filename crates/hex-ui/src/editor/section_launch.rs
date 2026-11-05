//! Transient section audition intent; seek preparation/publication retains all
//! graph/resource custody. Pointer gestures dispatch only stationary ordinary
//! section clicks. A newer click cancels the previous seek through its exact
//! preparation/publication token; toolbar mode does not own a playback timer.
use super::EditorView;
use hex_project::{LoopRegion, SectionId};
use hex_session::arrangement_seek::SectionLaunch;

impl EditorView {
    pub(super) fn toggle_section_autoplay(&mut self) {
        self.section_autoplay = !self.section_autoplay;
        if !self.section_autoplay
            && (self
                .prepared_seek
                .as_ref()
                .is_some_and(|seek| seek.section.is_some())
                || self
                    .latched_seek_intent
                    .as_ref()
                    .is_some_and(|seek| seek.section.is_some()))
        {
            self.invalidate_prepared_seek();
        }
        self.status = if self.section_autoplay {
            "section autoplay and loop on"
        } else {
            "section autoplay off"
        }
        .into();
    }

    pub(super) fn launch_arrangement_section(&mut self, id: SectionId) {
        if !self.section_autoplay {
            return;
        }
        let region = (|| {
            let state = self.state.as_ref().ok_or("section source is unavailable")?;
            let section = state
                .project
                .arrangement_section(id)
                .ok_or("section no longer exists")?;
            let tempo = state.project.tempo_map();
            let start = tempo
                .beat_to_frame(section.start())
                .map_err(|_| "section start exceeds playback timing")?;
            let end = tempo
                .beat_to_frame(section.end())
                .map_err(|_| "section end exceeds playback timing")?;
            LoopRegion::new(start, end, true)
                .map_err(|_| "section must contain a positive playback interval")
        })();
        match region {
            Ok(region) => self.request_section_launch(SectionLaunch {
                region,
                quantization: self.session_quantization,
            }),
            Err(error) => self.error = Some(error.into()),
        }
    }
}
