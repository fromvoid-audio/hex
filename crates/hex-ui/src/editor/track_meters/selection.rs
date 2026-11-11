//! Track identities to observe for the current console page.
use super::super::console_track_page::{TrackPage, visible_track_range};
use hex_project::{Project, TrackId};

fn selected_console_meter(project: &Project, selected: Option<TrackId>) -> Option<TrackId> {
    selected.filter(|id| project.track(*id).is_some())
}

/// Use the grid's bounded page, including its frozen reorder projection. The
/// selected fallback keeps its observation warm across Console page changes.
/// Reuse the caller's staging storage; this helper owns no meter or audio state.
pub(super) fn console_meter_tracks(
    project: &Project,
    frozen: Option<&TrackPage>,
    indices: &[usize],
    start: usize,
    count: usize,
    selected: Option<TrackId>,
    output: &mut Vec<TrackId>,
) {
    if let Some(page) = frozen {
        output.extend(page.columns.iter().filter_map(|column| {
            project
                .tracks()
                .get(column.index)
                .filter(|track| track.id() == column.id)
                .map(hex_project::Track::id)
        }));
    } else {
        let range = visible_track_range(indices.len(), start, count);
        output.extend(
            indices[range]
                .iter()
                .filter_map(|&index| project.tracks().get(index).map(hex_project::Track::id)),
        );
    }
    if let Some(id) = selected_console_meter(project, selected)
        && !output.contains(&id)
    {
        output.push(id);
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};

    #[test]
    fn console_meter_page_matches_folded_paging_and_frozen_reorder_ids() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let tracks: Vec<_> = (0..4)
            .map(|index| {
                let Created::Track(id) = editor
                    .execute(Command::AddTrack {
                        name: format!("track {index}"),
                    })
                    .unwrap()
                else {
                    panic!("track fixture")
                };
                id
            })
            .collect();
        let indices = [0, 2, 3]; // The folded projection omits track 1.
        let frozen = TrackPage::capture(editor.project(), &indices, 0, 2);
        let mut output = Vec::with_capacity(8);
        let pointer = output.as_ptr();
        for (page, projection, start, count, selected, expected) in [
            (
                None,
                &indices[..],
                1,
                2,
                Some(tracks[1]),
                vec![tracks[2], tracks[3], tracks[1]],
            ),
            (
                None,
                &indices[..],
                0,
                2,
                Some(tracks[0]),
                vec![tracks[0], tracks[2]],
            ),
            (None, &indices[..], usize::MAX, 0, None, vec![tracks[3]]),
            (None, &[usize::MAX][..], 0, 2, None, vec![]),
            (
                Some(&frozen),
                &[3][..],
                99,
                1,
                None,
                vec![tracks[0], tracks[2]],
            ),
        ] {
            output.clear();
            console_meter_tracks(
                editor.project(),
                page,
                projection,
                start,
                count,
                selected,
                &mut output,
            );
            assert_eq!(output, expected);
            assert_eq!(
                output.as_ptr(),
                pointer,
                "warm observations reuse staging storage"
            );
        }
        editor.execute(Command::RemoveTrack(tracks[0])).unwrap();
        output.clear();
        console_meter_tracks(
            editor.project(),
            Some(&frozen),
            &indices,
            0,
            2,
            None,
            &mut output,
        );
        assert!(
            output.is_empty(),
            "frozen index/ID pairs cannot read a shifted or deleted track"
        );
    }

    #[test]
    fn console_meter_tracks_the_selected_live_identity_including_groups() {
        use hex_project::TrackSelectionIds;
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        assert_eq!(
            selected_console_meter(editor.project(), Some(group)),
            Some(group)
        );
        assert_eq!(
            selected_console_meter(editor.project(), Some(track)),
            Some(track)
        );
        assert_eq!(selected_console_meter(editor.project(), None), None);
        let mut visible = Vec::new();
        console_meter_tracks(editor.project(), None, &[0], 0, 8, None, &mut visible);
        assert_eq!(visible, vec![group], "folded child meters are not observed");
        editor.execute(Command::RemoveTrack(group)).unwrap();
        assert_eq!(selected_console_meter(editor.project(), Some(group)), None);
        assert_eq!(selected_console_meter(editor.project(), Some(track)), None);
    }
}
