//! Launch a section with the quantized seek worker. L-click enables the
//! canonical Arrangement loop. A plain launch disables an enabled loop in
//! place and plays to the section end on the transport's content clock.
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SectionLoopKey {
    #[default]
    Up,
    Held,
    Used,
}

pub(super) struct SectionLaunchAfterLoop {
    project: ProjectRevision,
    section: SectionLaunch,
    expected_loop: Option<LoopRegion>,
}

impl EditorView {
    /// Delay the ordinary L shortcut until release so a section click can
    /// consume the held key without first looping the current selection.
    pub(super) fn section_loop_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &gpui::Window,
    ) -> bool {
        if event.keystroke.key != "l" || event.keystroke.modifiers != gpui::Modifiers::default() {
            return false;
        }
        if self.section_loop_key != SectionLoopKey::Up {
            return true;
        }
        if event.is_held
            || self.mode != super::EditorMode::Arrangement
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.project.arrangement_sections().is_empty())
        {
            return false;
        }
        let context = window.context_stack();
        if !context.iter().any(|key| key.contains("hex_editor"))
            || context.iter().any(|key| {
                key.contains("hex_text_editing")
                    || key.contains("hex_text_input")
                    || key.contains("hex_browser_search")
            })
        {
            return false;
        }
        if self.computer_keyboard.enabled {
            let position = window.mouse_position();
            if !self.pointer_hits_section_launch(position.x.into(), position.y.into()) {
                return false;
            }
        }
        self.section_loop_key = SectionLoopKey::Held;
        true
    }

    pub(super) fn section_loop_key_up(
        &mut self,
        event: &gpui::KeyUpEvent,
        window: &gpui::Window,
        cx: &gpui::Context<Self>,
    ) -> bool {
        if event.keystroke.key != "l" {
            return false;
        }
        let prior = std::mem::take(&mut self.section_loop_key);
        if prior == SectionLoopKey::Held && self.mode == super::EditorMode::Arrangement {
            self.loop_selection_key(window, cx);
        }
        prior != SectionLoopKey::Up
    }

    pub(super) fn use_section_loop_key(&mut self) -> bool {
        if self.section_loop_key == SectionLoopKey::Up {
            return false;
        }
        self.section_loop_key = SectionLoopKey::Used;
        true
    }

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
        self.launch_section(id, true);
    }

    /// Explicit momentary action, independent of header-click autoplay mode.
    pub(super) fn launch_section(&mut self, id: SectionId, looped: bool) {
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
            LoopRegion::new(start, end, looped)
                .map_err(|_| "section must contain a positive playback interval")
        })();
        match region {
            Ok(region) => self.prepare_section_launch(SectionLaunch {
                region,
                quantization: self.session_quantization,
            }),
            Err(error) => self.error = Some(error.into()),
        }
    }

    fn prepare_section_launch(&mut self, section: SectionLaunch) {
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
        let current = state.project.loop_region();
        if section.matches_loop(current) {
            self.section_loop_committed(section);
            return;
        }
        let region = if section.region.enabled() {
            Some(section.region)
        } else {
            current.and_then(|current| LoopRegion::new(current.start(), current.end(), false).ok())
        };
        if let Some(region) = region {
            self.submit_section_launch_loop(section, region);
        }
    }

    pub(super) fn section_loop_committed(&mut self, section: SectionLaunch) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let expected_loop = state.project.loop_region();
        if !section.matches_loop(expected_loop) {
            return;
        }
        self.section_launch_after_loop = Some(SectionLaunchAfterLoop {
            project: ProjectRevision {
                generation: self.project_generation,
                revision: state.revision,
            },
            section,
            expected_loop,
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
                    || state.project.loop_region() != pending.expected_loop
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

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn test_section_loop_key(visual: &mut gpui::VisualTestContext, down: bool) {
    let keystroke = gpui::Keystroke::parse("l").unwrap();
    if down {
        visual.simulate_event(gpui::KeyDownEvent {
            keystroke,
            is_held: false,
        });
    } else {
        visual.simulate_event(gpui::KeyUpEvent { keystroke });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn test_tap_loop_key(visual: &mut gpui::VisualTestContext) {
    test_section_loop_key(visual, true);
    test_section_loop_key(visual, false);
    visual.run_until_parked();
}
