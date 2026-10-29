//! Visible Console pages use one stable ID/index snapshot and following anchor.
#[path = "../src/editor/console_track_page.rs"]
mod console_track_page;

use console_track_page::TrackPage;
use hex_project::{Command, Created, Editor, Project};

#[test]
fn page_captures_folded_columns_and_next_anchor_across_offsets() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut ids = Vec::new();
    for n in 0..5 {
        let Created::Track(id) = editor
            .execute(Command::AddTrack {
                name: format!("track {n}"),
            })
            .unwrap()
        else {
            panic!("expected track")
        };
        ids.push(id);
    }
    let project = editor.project();
    let folded = [0, 2, 4];
    let page = TrackPage::capture(project, &folded, 1, 1);
    assert_eq!(page.range, 1..2);
    assert_eq!(page.columns.len(), 1);
    assert_eq!((page.columns[0].index, page.columns[0].id), (2, ids[2]));
    assert_eq!(page.next, Some(ids[4]));

    let last = TrackPage::capture(project, &folded, usize::MAX, 2);
    assert_eq!(last.range, 1..3);
    assert_eq!(
        last.columns
            .iter()
            .map(|column| column.id)
            .collect::<Vec<_>>(),
        [ids[2], ids[4]]
    );
    assert_eq!(last.next, None);
    let all = TrackPage::capture(project, &folded, 0, 10);
    assert_eq!(all.range, 0..3);
    assert_eq!(
        all.columns
            .iter()
            .map(|column| column.id)
            .collect::<Vec<_>>(),
        [ids[0], ids[2], ids[4]]
    );
    let empty = TrackPage::capture(project, &[], 0, 4);
    assert!(empty.columns.is_empty());
    assert_eq!(empty.next, None);
    let zero_width = TrackPage::capture(project, &folded, 0, 0);
    assert_eq!(zero_width.range, 0..1);
    assert_eq!(zero_width.next, Some(ids[2]));

    let old = page.columns[0];
    editor.execute(Command::RemoveTrack(ids[0])).unwrap();
    assert_eq!((old.index, old.id), (2, ids[2]));
    assert_ne!(editor.project().tracks()[old.index].id(), old.id);
}
