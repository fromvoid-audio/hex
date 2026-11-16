//! Initial appearance for a newly painted arrangement section.
//!
//! Hosts choose once at capture and retain the result through preview and commit.
use hex_project::{ElementColor, Project};
use std::hash::BuildHasher;

pub(crate) fn new_marker(project: &Project) -> (String, ElementColor) {
    let ticket = std::collections::hash_map::RandomState::new()
        .hash_one(project.arrangement_sections().len());
    let color =
        ElementColor::TRACK_CHOICES[(ticket % ElementColor::TRACK_CHOICES.len() as u64) as usize];
    let number = (1..=hex_project::MAX_ARRANGEMENT_SECTIONS + 1)
        .find(|n| {
            let name = format!("section {n}");
            !project
                .arrangement_sections()
                .iter()
                .any(|s| s.name() == name)
        })
        .unwrap_or(hex_project::MAX_ARRANGEMENT_SECTIONS + 1);
    (format!("section {number}"), color)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Editor};

    #[test]
    fn chooses_first_unused_name_and_a_palette_color() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        for (name, start) in [("section 1", 0), ("section 3", 2)] {
            editor
                .execute(Command::AddArrangementSection {
                    name: name.into(),
                    color: ElementColor::Cyan,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(start + 1).unwrap(),
                })
                .unwrap();
        }
        let before = editor.project().snapshot();
        let (name, color) = new_marker(editor.project());
        assert_eq!(name, "section 2");
        assert!(ElementColor::TRACK_CHOICES.contains(&color));
        assert_eq!(editor.project().snapshot(), before);
    }
}
