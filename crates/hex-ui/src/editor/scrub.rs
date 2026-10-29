//! Latest-target Arrangement ruler drag seeking through the existing transport.
//! One retained destination and at most one admission attempt per UI tick.
use super::{EditorMode, EditorView, viewport::Viewport};
use gpui::{Bounds, Pixels};

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrubContext {
    generation: u64,
    revision: u64,
    viewport: Viewport,
    bounds: Bounds<Pixels>,
}

#[derive(Clone, Copy, Debug)]
struct ScrubGesture {
    context: ScrubContext,
    latest: Option<u64>,
    last_issued: Option<u64>,
    released: bool,
}

impl ScrubGesture {
    fn target(&mut self, frame: u64, released: bool) {
        self.latest = (self.last_issued != Some(frame)).then_some(frame);
        self.released |= released;
    }

    fn admitted(&mut self, frame: u64) {
        self.last_issued = Some(frame);
        self.latest = None;
    }
}

#[derive(Default)]
pub(super) struct ScrubState {
    bounds: Option<Bounds<Pixels>>,
    gesture: Option<ScrubGesture>,
}

impl EditorView {
    /// Includes a released final target until its existing admission/refusal
    /// retires it: switching modes must not silently discard that target.
    pub(super) fn scrub_active(&self) -> bool {
        self.scrub.gesture.is_some()
    }

    pub(super) fn update_scrub_geometry(&mut self, bounds: Bounds<Pixels>) {
        if self
            .scrub
            .gesture
            .is_some_and(|gesture| gesture.context.bounds != bounds)
        {
            self.cancel_scrub();
        }
        self.scrub.bounds = Some(bounds);
    }

    fn scrub_context(&self, viewport: Viewport, bounds: Bounds<Pixels>) -> Option<ScrubContext> {
        let state = self.state.as_ref()?;
        (self.mode == EditorMode::Arrangement
            && !self.edit_pending()
            && self.context_menu.is_none()
            // The raw ruler handlers also see pointer events over modal overlays.
            && !self.theme_settings.open
            && !self.tuning.open
            && !self.changelog.is_open()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.recovery.generation().is_none()
            && self
                .audio
                .as_ref()
                .is_none_or(|audio| super::arrangement_seek::audio_ready_for_seek(&audio.control))
            && !self.device_gesture_pending()
            && self.device_gesture.snapshot().is_none()
            && self.active_clip_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_fade_drag.is_none()
            && self.active_midi_trim_drag.is_none()
            && self.arrangement_pan.is_none()
            && self.loop_rail_gesture.is_none()
            && self.time_selection_gesture.is_none()
            && self.time_range_move_gesture.is_none()
            && self.visible_viewport(state.timeline_frames) == viewport
            && self.scrub.bounds == Some(bounds))
        .then_some(ScrubContext {
            generation: self.project_generation,
            revision: state.revision,
            viewport,
            bounds,
        })
    }

    pub(super) fn begin_scrub(
        &mut self,
        x: f32,
        viewport: Viewport,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let Some(context) = self.scrub_context(viewport, bounds) else {
            return false;
        };
        let Some(frame) = super::seek::pointer_to_frame(
            x,
            f32::from(bounds.origin.x),
            f32::from(bounds.size.width),
            viewport,
        ) else {
            return false;
        };
        self.latency_rebuild = None;
        self.scrub.gesture = Some(ScrubGesture {
            context,
            latest: Some(frame),
            last_issued: None,
            released: false,
        });
        true
    }

    pub(super) fn update_scrub(&mut self, x: f32, released: bool) -> bool {
        if self.scrub.gesture.is_some_and(|gesture| gesture.released) {
            return false;
        }
        let Some(mut gesture) = self.scrub.gesture.take() else {
            return false;
        };
        if self.scrub_context(gesture.context.viewport, gesture.context.bounds)
            != Some(gesture.context)
        {
            return true;
        }
        let Some(frame) = super::seek::pointer_to_frame(
            x,
            f32::from(gesture.context.bounds.origin.x),
            f32::from(gesture.context.bounds.size.width),
            gesture.context.viewport,
        ) else {
            return true;
        };
        gesture.target(frame, released);
        if !gesture.released || gesture.latest.is_some() {
            self.scrub.gesture = Some(gesture);
        }
        true
    }

    pub(super) fn lose_scrub_pointer(&mut self) -> bool {
        if self.scrub.gesture.is_some_and(|gesture| !gesture.released) {
            self.cancel_scrub()
        } else {
            false
        }
    }

    /// Cancel only future targets. An admitted seek retains its existing
    /// acknowledgment/cancellation lifecycle, including priority Stop fencing.
    pub(super) fn cancel_scrub(&mut self) -> bool {
        self.scrub.gesture.take().is_some()
    }

    pub(super) fn tick_scrub(&mut self) -> bool {
        let Some(gesture) = self.scrub.gesture else {
            return false;
        };
        if self.scrub_context(gesture.context.viewport, gesture.context.bounds)
            != Some(gesture.context)
        {
            return self.cancel_scrub();
        }
        let Some(frame) = gesture.latest else {
            return false;
        };
        if self.seek_pending()
            || !self.pending_audio_commands.is_empty()
            || self.pending_playback.is_some()
            || !self.playback_candidates.is_empty()
        {
            return false;
        }
        let previous_ticket = self.pending_seek_ticket;
        self.request_seek(frame);
        if self.seek_pending() || self.pending_seek_ticket != previous_ticket {
            if let Some(gesture) = self.scrub.gesture.as_mut() {
                gesture.admitted(frame);
            }
            if self.scrub.gesture.is_some_and(|gesture| gesture.released) {
                self.scrub.gesture = None;
            }
        } else {
            // An immediate refusal already explains why in the existing seek
            // error channel. End the gesture so unsupported sources do not
            // retry that refusal on every pointer move or timer tick.
            self.cancel_scrub();
        }
        true
    }
}

#[cfg(test)]
#[path = "scrub_tests.rs"]
mod tests;
