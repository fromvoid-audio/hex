//! Section launch edits the ordinary Arrangement loop, then starts the existing
//! quantized seek from that exact revision once audio has applied it. Only the
//! launch is cancellable after the loop edit is admitted.
use super::{
    EditorView,
    audio_admission::{AdmissionState, ProjectRevision},
};
use hex_project::{LoopRegion, SectionId};
use hex_session::arrangement_seek::SectionLaunch;

gpui::actions!(hex_section_launch, [ActivateSectionLaunch]);
pub(super) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "enter",
        ActivateSectionLaunch,
        Some("hex_section_launch"),
    )]);
}

pub(super) struct SectionLaunchAfterLoop {
    project: ProjectRevision,
    section: SectionLaunch,
}

impl EditorView {
    pub(super) fn toggle_section_autoplay(&mut self) {
        self.section_autoplay = !self.section_autoplay;
        if !self.section_autoplay
            && (self.section_launch_waiting()
                || self
                    .prepared_seek
                    .as_ref()
                    .is_some_and(|seek| seek.kind.section().is_some())
                || self
                    .latched_seek_intent
                    .as_ref()
                    .is_some_and(|seek| seek.kind.section().is_some()))
        {
            self.cancel_section_launch_wait();
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
        self.launch_arrangement_section_once(id);
    }

    /// Explicit momentary action, independent of header-click autoplay mode.
    pub(super) fn launch_arrangement_section_once(&mut self, id: SectionId) {
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
            Ok(region) => self.set_loop_and_launch_section(SectionLaunch {
                region,
                quantization: self.session_quantization,
            }),
            Err(error) => self.error = Some(error.into()),
        }
    }

    fn set_loop_and_launch_section(&mut self, section: SectionLaunch) {
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error = Some("section playback is unavailable in verified streaming mode".into());
            return;
        }
        if self.audible_playback.is_none() || self.audio_status().is_none() {
            self.error = Some("section playback source is unavailable".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if state.project.loop_region() == Some(section.region) {
            self.section_loop_committed(section);
        } else {
            self.submit_section_launch_loop(section);
        }
    }

    pub(super) fn section_loop_committed(&mut self, section: SectionLaunch) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if state.project.loop_region() != Some(section.region) {
            return;
        }
        self.section_launch_after_loop = Some(SectionLaunchAfterLoop {
            project: ProjectRevision {
                generation: self.project_generation,
                revision: state.revision,
            },
            section,
        });
        self.wake_poll();
    }

    pub(super) fn tick_section_launch(&mut self) -> bool {
        let Some(pending) = self.section_launch_after_loop.as_ref() else {
            return false;
        };
        if self.mode != super::EditorMode::Arrangement
            || self.opening_project
            || self.close_prompt_open
            || self
                .audio
                .as_ref()
                .is_none_or(|audio| !super::arrangement_seek::audio_ready_for_seek(&audio.control))
            || self.project_generation != pending.project.generation
            || self.state.as_ref().is_none_or(|state| {
                state.revision != pending.project.revision
                    || state.project.loop_region() != Some(pending.section.region)
            })
            || self.audio_admission.current().is_some_and(|admission| {
                admission.project == pending.project
                    && matches!(admission.state, AdmissionState::Rejected(_))
            })
        {
            self.section_launch_after_loop = None;
            return true;
        }
        if self.applied_project_revision() != Some(pending.project.revision) {
            return false;
        }
        let section = pending.section;
        self.section_launch_after_loop = None;
        self.request_section_launch(section);
        true
    }

    pub(super) fn section_launch_waiting(&self) -> bool {
        self.section_launch_after_loop.is_some() || self.pending_loop_launch()
    }

    pub(super) fn cancel_section_launch_wait(&mut self) {
        self.section_launch_after_loop = None;
        self.cancel_pending_loop_launch();
    }
}

#[path = "section_launch/geometry.rs"]
mod geometry;
pub(super) use geometry::{ACTION_LEFT, ACTION_SIZE, ACTION_TOP, action_bounds};
