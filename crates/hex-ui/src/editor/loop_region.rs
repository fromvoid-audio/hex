//! Loop controls dispatch canonical edits; DSP and validation belong to the engine/model.
use super::{EditorMode, EditorView};
use gpui::{Context, Window};
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    /// Plain L/W remain musical notes while typing is enabled and retain the
    /// existing Session toggle. Command-L toggles the active transport loop.
    pub(super) fn loop_selection_key(&mut self, window: &Window, cx: &Context<Self>) {
        if self.computer_keyboard.enabled || !self.loop_shortcut_clear() {
            return;
        }
        if self.clip_loop_key(window) || !self.focus.is_focused(window) {
            return;
        }
        if self.mode != EditorMode::Arrangement {
            self.toggle_loop();
        } else {
            self.apply_selection_loop(cx);
        }
    }

    fn loop_shortcut_clear(&self) -> bool {
        !self.sections.naming()
            && !self.changelog.is_open()
            && !self.track_name.is_editing()
            && self.clip_name_edit.is_none()
            && self.context_menu.is_none()
            && !self.theme_settings.open
            && !self.tuning.open
            && !self.save_dialog_open
            && !self.export_dialog_open
    }

    /// Modified loop toggle is transport-wide and never derives new bounds.
    pub(super) fn toggle_active_loop_shortcut(&mut self, window: &Window, cx: &Context<Self>) {
        if !self.loop_shortcut_clear()
            || self.opening_project
            || self.close_prompt_open
            || self.controller_input_is_open()
            || self.preset_dialog_open
            || self.send_menu_track.is_some()
            || (self.clip_view_open
                && self.device_panel_active
                && self.device_panel.add_menu_is_open(self.state.as_deref()))
            || window
                .context_stack()
                .iter()
                .any(|context| context.contains("hex_text_editing"))
        {
            return;
        }
        let gates = self.loop_edit_gates();
        if gates.iter().any(|(_, active)| *active) {
            self.record_loop_refusal(cx, &gates, false);
            self.error = Some("finish the current edit before changing the loop".into());
            return;
        }
        if self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
            .is_some()
        {
            self.toggle_loop();
        }
    }

    /// The names are diagnostics only; the same snapshot determines admission.
    fn loop_edit_gates(&self) -> [(&'static str, bool); 28] {
        [
            ("edit_pending", self.edit_pending()),
            ("gain_recording", self.gain_recorder.blocks_edits()),
            ("midi_recording", self.midi_recorder.blocks_edits()),
            (
                "sampler_parameter_pending",
                self.sampler_parameter_pending(),
            ),
            ("opening_project", self.opening_project),
            ("close_prompt", self.close_prompt_open),
            ("device_gesture_pending", self.device_gesture_pending()),
            ("arrangement_pan", self.arrangement_pan.is_some()),
            ("scrub", self.scrub_active()),
            ("gain_lane_pointer", self.gain_lane.pointer_is_active()),
            (
                "device_automation_pointer",
                self.automation_view.devices.pointer_active(),
            ),
            ("device_gesture", self.device_gesture.snapshot().is_some()),
            ("clip_paint", self.clip_paint_gesture.is_some()),
            ("clip_detail_resize", self.clip_detail_resize.is_active()),
            ("spectrum_resize", self.project_spectrum.resize_is_active()),
            (
                "device_navigation",
                self.device_panel.navigation.is_active(),
            ),
            ("time_selection", self.time_selection_gesture.is_some()),
            ("time_range_move", self.time_range_move_gesture.is_some()),
            ("clip_drag", self.active_clip_drag.is_some()),
            (
                "browser_audio_drag",
                self.active_browser_audio_drag.is_some(),
            ),
            (
                "native_device_drag",
                self.active_native_device_drag.is_some(),
            ),
            ("trim_drag", self.active_trim_drag.is_some()),
            ("midi_trim_drag", self.active_midi_trim_drag.is_some()),
            ("fade_drag", self.active_fade_drag.is_some()),
            ("track_resize", self.track_resize_active()),
            ("loop_rail_gesture", self.loop_rail_gesture.is_some()),
            ("section_held", self.sections.held()),
            ("section_install_pending", self.section_install_pending()),
        ]
    }

    fn record_loop_refusal(
        &self,
        cx: &Context<Self>,
        gates: &[(&'static str, bool)],
        section_view_blocked: bool,
    ) {
        if !crate::diagnostics::actions_enabled(cx) {
            return;
        }
        crate::diagnostics::record_loop_refusal(
            cx,
            gates,
            section_view_blocked,
            self.worker_requests_pending,
            self.pending_requests.len(),
            self.sections.pending(),
            self.audio
                .as_ref()
                .and_then(|audio| audio.control.status())
                .is_some_and(|status| status.stopped_edit_pending()),
        );
    }

    fn apply_selection_loop(&mut self, cx: &Context<Self>) {
        let gates = self.loop_edit_gates();
        let edit_blocked = gates.iter().any(|(_, active)| *active);
        let section_view_blocked = (!edit_blocked || crate::diagnostics::actions_enabled(cx))
            && self.section_loop_bounds().ok().flatten().is_some()
            && !self.arrangement_view_change_available(cx);
        if edit_blocked || section_view_blocked {
            self.record_loop_refusal(cx, &gates, section_view_blocked);
            self.error = Some("finish the current edit before setting the loop".into());
            return;
        }
        let existing = self.state.as_ref().and_then(|s| s.project.loop_region());
        match self.selection_loop_bounds() {
            Ok(Some((start, end)))
                if existing.is_some_and(|r| (r.start(), r.end()) == (start, end)) =>
            {
                self.toggle_loop();
            }
            Ok(Some((start, end))) => {
                let section = self.section_loop_bounds() == Ok(Some((start, end)));
                match hex_project::LoopRegion::new(start, end, true) {
                    Ok(region) => self.submit_selection_loop_preview(region, section),
                    Err(error) => self.error = Some(error.0.into()),
                }
            }
            Ok(None) if existing.is_some() => self.toggle_loop(),
            Ok(None) => {}
            Err(error) => self.error = Some(error.into()),
        }
    }

    fn selection_loop_bounds(&self) -> Result<Option<(u64, u64)>, &'static str> {
        if let Some(bounds) = self.section_loop_bounds()? {
            return Ok(Some(bounds));
        }
        // Match range-copy precedence. A stale non-empty rectangle must refuse,
        // rather than silently looping a different whole-clip selection.
        if self
            .time_selection
            .is_some_and(|s| s.anchor_frame != s.end_frame)
        {
            return self
                .normalized_time_selection()
                .map(|range| Some((range.start(), range.end())))
                .ok_or("time selection no longer belongs to current tracks");
        }
        let Some(state) = &self.state else {
            return Ok(None);
        };
        let mut remaining = self
            .selected_clip_ids()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        // Mode switches preserve the primary selection. Session-owned clips
        // have no Arrangement extent and are not deleted/stale Arrangement IDs.
        // Remove them in one bounded pass; unknown IDs must still refuse below.
        for cell in state.project.session_cells() {
            match cell.content() {
                hex_project::SessionCellContent::Audio(clip) => {
                    remaining.remove(&clip.id());
                }
                hex_project::SessionCellContent::Midi(clip) => {
                    remaining.remove(&clip.id());
                }
                hex_project::SessionCellContent::Stop => {}
            }
        }
        if remaining.is_empty() {
            return Ok(None);
        }
        let mut start = u64::MAX;
        let mut end = 0;
        let mut include = |clip_start: u64, clip_end: Option<u64>| {
            let clip_end = clip_end.ok_or("selected clip exceeds the timeline")?;
            if clip_start >= clip_end {
                return Err("selected clip has an empty loop range");
            }
            start = start.min(clip_start);
            end = end.max(clip_end);
            Ok(())
        };
        // Visit the bounded clip collection once, rather than rescanning it per selection.
        for track in state.project.tracks() {
            for clip in track.clips() {
                if remaining.remove(&clip.id()) {
                    include(clip.start(), clip.start().checked_add(clip.duration()))?;
                }
            }
            for clip in track.midi_clips() {
                if remaining.remove(&clip.id()) {
                    let error = "selected midi clip exceeds the timeline";
                    let beat_end = clip
                        .start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(error)?;
                    let map = state.project.tempo_map();
                    let clip_start = map.beat_to_frame(clip.start()).map_err(|_| error)?;
                    let clip_end = map
                        .beat_to_frame(hex_project::BeatTime::from_raw(beat_end))
                        .ok();
                    include(clip_start, clip_end)?;
                }
            }
        }
        if !remaining.is_empty() {
            return Err("selected clip no longer belongs to the arrangement");
        }
        Ok(Some((start, end)))
    }

    pub(super) fn toggle_loop(&mut self) {
        self.enqueue(Request::Edit(Command::ToggleLoopRegion));
    }
}
