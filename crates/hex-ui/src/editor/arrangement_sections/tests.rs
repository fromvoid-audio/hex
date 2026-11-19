use super::*;
use hex_project::Created;

#[test]
fn section_insertion_uses_edges_and_does_not_move_a_clicked_source() {
    let mut e = hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for (start, end) in [(0, 4), (8, 12)] {
        let Created::ArrangementSection(id) = e
            .execute(Command::AddArrangementSection {
                name: format!("section {}", ids.len() + 1),
                color: ElementColor::Cyan,
                start: BeatTime::from_quarters(start).unwrap(),
                end: BeatTime::from_quarters(end).unwrap(),
            })
            .unwrap()
        else {
            panic!("section")
        };
        ids.push(id);
    }
    let p = e.project();
    assert_eq!(
        section_reorder::insertion_target(p, ids[0], BeatTime::from_quarters(2).unwrap(), false),
        None
    );
    assert_eq!(
        section_reorder::insertion_target(p, ids[0], BeatTime::from_quarters(9).unwrap(), false),
        Some((ids[1], false))
    );
    assert_eq!(
        section_reorder::insertion_target(p, ids[0], BeatTime::from_quarters(13).unwrap(), false),
        Some((ids[1], true))
    );
    assert_eq!(
        section_reorder::insertion_target(p, ids[0], BeatTime::from_quarters(1).unwrap(), true),
        Some((ids[0], false))
    );
    assert_eq!(
        section_reorder::insertion_target(p, ids[0], BeatTime::from_quarters(3).unwrap(), true),
        Some((ids[0], true))
    );
    let (name, color) = section_appearance::new_marker(p);
    assert_eq!(name, "section 3");
    assert!(ElementColor::TRACK_CHOICES.contains(&color));
}
