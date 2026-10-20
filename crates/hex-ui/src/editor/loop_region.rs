//! Loop controls dispatch canonical edits; DSP and validation belong to the engine/model.
use super::{EditorMode, EditorView};
use gpui::Window;
use hex_project::Command;
use hex_session::service::Request;
use hex_session::snapshot_exports::ui_consumers::SnapshotSessionCellContentRead as CellContent;

impl EditorView {
    /// Existing Command-L action now owns the complete Arrangement selection.
    pub(super) fn set_loop_from_clip(&mut self, window: &Window) {
        if self.mode == EditorMode::Arrangement && self.loop_shortcut_context(window) {
            self.set_selection_loop();
        }
    }

    /// Plain L remains a musical note while typing is enabled, and retains its
    /// existing Session toggle. Command-L is the explicit modified shortcut.
    pub(super) fn loop_selection_key(&mut self, window: &Window) {
        if self.computer_keyboard.enabled || !self.loop_shortcut_context(window) {
            return;
        }
        if self.mode != EditorMode::Arrangement {
            self.toggle_loop();
        } else {
            self.set_selection_loop();
        }
    }

    fn loop_shortcut_context(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
            && !self.track_name.is_editing()
            && self.clip_name_edit.is_none()
            && self.context_menu.is_none()
            && !self.theme_settings.open
            && !self.tuning.open
            && !self.save_dialog_open
            && !self.export_dialog_open
    }

    fn set_selection_loop(&mut self) {
        if self.edit_pending()
            || self.arrangement_pan.is_some()
            || self.scrub_active()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.track_resize_active()
            || self.loop_rail_gesture.is_some()
        {
            self.error = Some("finish the current edit before setting the loop".into());
            return;
        }
        match self.selection_loop_bounds() {
            Ok(Some((start, end))) => {
                self.enqueue(Request::Edit(Command::SetLoopRegion {
                    start,
                    end,
                    enabled: true,
                }));
            }
            Ok(None) => {}
            Err(error) => self.error = Some(error.into()),
        }
    }

    fn selection_loop_bounds(&self) -> Result<Option<(u64, u64)>, &'static str> {
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
        state
            .read_with(|snapshot| {
                let project = snapshot.model();
                // Mode switches preserve the primary selection. Session-owned clips
                // have no Arrangement extent and are not deleted/stale Arrangement IDs.
                // Remove them in one bounded pass; unknown IDs must still refuse below.
                for cell in project.session_cells() {
                    match cell.content() {
                        CellContent::Audio(clip) => {
                            remaining.remove(&clip.id());
                        }
                        CellContent::Midi(clip) => {
                            remaining.remove(&clip.id());
                        }
                        CellContent::Stop | CellContent::Empty => {}
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
                for track in project.tracks() {
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
                                .checked_add(clip.arrangement_length().raw())
                                .ok_or(error)?;
                            let map = project.tempo();
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
            })
            .map_err(|_| "project read is unavailable · cannot set loop region")?
    }

    pub(super) fn toggle_loop(&mut self) {
        self.enqueue(Request::Edit(Command::ToggleLoopRegion));
    }
}
