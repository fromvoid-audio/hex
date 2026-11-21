//! Section-lane clipboard intent. Captures project-local section metadata (no
//! PCM or runtime DSP) and validates against the active project generation.
//! Cut commits its captured snapshot only when canonical deletion succeeds.
use super::*;

#[derive(Clone)]
pub(super) struct Captured {
    pub(super) generation: u64,
    pub(super) clipboard: Arc<hex_project::SectionClipboard>,
}

impl EditorView {
    /// Consumes section-context Copy, including visible refusals.
    pub(in crate::editor) fn copy_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        if !self.sections_available() || self.sections.held() {
            self.error = Some("finish the current edit before copying a section".into());
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(section) = self.sections.selected.filter(|id| {
            self.sections.selection.len() <= 1 && state.project.arrangement_section(*id).is_some()
        }) else {
            self.error = Some("select one section to copy".into());
            return true;
        };
        match state.project.capture_arrangement_section(section) {
            Ok(captured) => {
                self.sections.clipboard = Some(Captured {
                    generation: self.project_generation,
                    clipboard: Arc::new(captured),
                });
                self.error = None;
                self.status = "section copied".into();
            }
            Err(error) => {
                self.error = Some(error.to_string().into());
            }
        }
        true
    }

    /// Consumes section-context Cut. Captures metadata first, deletes the
    /// section through the ordinary fenced edit path, and commits the clipboard
    /// only on exact acknowledgement.
    pub(in crate::editor) fn cut_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        if !self.sections_available() || self.sections.held() {
            self.error = Some("finish the current edit before cutting a section".into());
            return true;
        }
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(section) = self.sections.selected.filter(|id| {
            self.sections.selection.len() <= 1 && state.project.arrangement_section(*id).is_some()
        }) else {
            self.error = Some("select one section to cut".into());
            return true;
        };
        let Some(cut_start) = state
            .project
            .arrangement_section(section)
            .map(|marker| marker.start())
        else {
            return true;
        };
        let captured = match state.project.capture_arrangement_section(section) {
            Ok(captured) => Captured {
                generation: self.project_generation,
                clipboard: Arc::new(captured),
            },
            Err(error) => {
                self.error = Some(error.to_string().into());
                return true;
            }
        };
        self.error = None;
        self.section_metadata(Command::DeleteArrangementSection(section));
        if let Some(pending) = self.sections.pending.as_mut()
            && matches!(pending.command, Command::DeleteArrangementSection(id) if id == section)
        {
            pending.cut_clipboard = Some(captured);
            pending.cut_cursor = Some(cut_start);
        }
        true
    }

    /// Pastes the captured section at the exact section cursor beat, rippling
    /// later music right (splitting an enclosing section or marking a gap past
    /// the final section). Cursor snapping stays owned by the original click.
    pub(in crate::editor) fn paste_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        if !self.sections_available() || self.sections.held() {
            self.error = Some("finish the current edit before pasting a section".into());
            return true;
        }
        let Some(captured) = self
            .sections
            .clipboard
            .as_ref()
            .filter(|captured| captured.generation == self.project_generation)
            .cloned()
        else {
            self.error = Some("section clipboard is empty · copy or cut a section first".into());
            return true;
        };
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let Some(cursor) = self.sections.cursor.filter(|cursor| {
            cursor.generation == self.project_generation && cursor.revision == state.revision
        }) else {
            self.error = Some("click the section lane to place the paste cursor".into());
            return true;
        };
        let (gap_name, gap_color) = section_appearance::new_marker(&state.project);
        let command = Command::PasteArrangementSection {
            clipboard: Arc::clone(&captured.clipboard),
            at: cursor.beat,
            gap_name,
            gap_color,
        };
        self.error = None;
        self.select_section_metadata(None);
        self.section_metadata(command);
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
