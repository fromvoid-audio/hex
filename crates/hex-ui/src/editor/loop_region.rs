//! Loop controls dispatch canonical edits; DSP and validation belong to the engine/model.
use super::EditorView;
use hex_project::Command;
use hex_session::service::Request;

impl EditorView {
    pub(super) fn set_loop_from_clip(&mut self) {
        match self.selected_midi_clip_data() {
            Ok(Some(_)) => {
                self.error = Some("looping a midi clip is not supported yet".into());
                return;
            }
            Ok(None) => {}
            Err(_) => {
                self.error = Some("project read is unavailable · cannot set loop region".into());
                return;
            }
        }
        let (start, duration) = match self.selected_clip_data() {
            Ok(Some((_, _, start, duration, ..))) => (start, duration),
            Ok(None) => {
                self.error = Some("select a clip to set the loop region".into());
                return;
            }
            Err(_) => {
                self.error = Some("project read is unavailable · cannot set loop region".into());
                return;
            }
        };
        let Some(end) = start.checked_add(duration) else {
            self.error = Some("loop region exceeds the timeline".into());
            return;
        };
        self.enqueue(Request::Edit(Command::SetLoopRegion {
            start,
            end,
            enabled: true,
        }));
    }

    pub(super) fn toggle_loop(&mut self) {
        self.enqueue(Request::Edit(Command::ToggleLoopRegion));
    }
}
