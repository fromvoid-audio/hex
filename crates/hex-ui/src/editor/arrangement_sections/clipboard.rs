//! Section-lane clipboard intent. It borrows canonical identity by generation
//! and revision, never media/PCM. Exact successful copy receipts preserve the
//! original source and refresh the reference; unrelated edits fail closed.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Reference {
    generation: u64,
    revision: u64,
    section: SectionId,
}

impl Reference {
    pub(super) fn acknowledge_copy(
        &mut self,
        command: &Command,
        generation: u64,
        revision: u64,
        state: &ViewState,
    ) {
        if self.generation == generation
            && self.revision == revision
            && (copied_section_source(command).or_else(|| match command {
                Command::DuplicateArrangementSection(section) => Some(*section),
                _ => None,
            })) == Some(self.section)
            && state.project.arrangement_section(self.section).is_some()
        {
            self.revision = state.revision;
        }
    }
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
        self.sections.clipboard = Some(Reference {
            generation: self.project_generation,
            revision: state.revision,
            section,
        });
        self.error = None;
        self.status = "section copied".into();
        true
    }

    /// Pastes one typed copy; cursor snapping is owned by its original pointer
    /// action, so Command-V does not invert the grid a second time.
    pub(in crate::editor) fn paste_section(&mut self, window: &Window) -> bool {
        if !self.section_has_focus(window) {
            return false;
        }
        if !self.sections_available() || self.sections.held() {
            self.error = Some("finish the current edit before pasting a section".into());
            return true;
        }
        let Some(reference) = self.sections.clipboard else {
            self.error = Some("section clipboard is empty · copy a section first".into());
            return true;
        };
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        if reference.generation != self.project_generation
            || reference.revision != state.revision
            || state
                .project
                .arrangement_section(reference.section)
                .is_none()
        {
            self.error = Some("section clipboard changed · copy the section again".into());
            return true;
        }
        let Some(cursor) = self.sections.cursor.filter(|cursor| {
            cursor.generation == self.project_generation && cursor.revision == state.revision
        }) else {
            self.error = Some("click the section lane to place the paste cursor".into());
            return true;
        };
        let Some(last) = state.project.arrangement_sections().last() else {
            self.error = Some("section clipboard changed · copy the section again".into());
            return true;
        };
        let command = if cursor.beat >= last.end() {
            let (gap_name, gap_color) = section_appearance::new_marker(&state.project);
            Command::AppendArrangementSectionCopy {
                section: reference.section,
                start: cursor.beat,
                gap_name,
                gap_color,
            }
        } else {
            match section_reorder::preview(&state.project, reference.section, cursor.beat, true) {
                Ok(Some(preview)) => preview.command,
                Ok(None) => {
                    self.error = Some("no section insertion boundary at the cursor".into());
                    return true;
                }
                Err(error) => {
                    self.error = Some(error.to_string().into());
                    return true;
                }
            }
        };
        // Clipboard paste owns a fresh copy-selection intent without moving
        // the playback cursor; the ordinary receipt handles acknowledgement.
        self.select_section_metadata(Some(reference.section));
        self.section_metadata(command);
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
