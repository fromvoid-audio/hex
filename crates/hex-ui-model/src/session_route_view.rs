//! Read-only, revision-bound view of accepted Session routes from the audio callback.
use hex_project::Project;

pub const SESSION_ROUTE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRouteMarker {
    pub glyph: &'static str,
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RouteOwnerKind {
    #[default]
    Arrangement,
    Silence,
    Session {
        clip_id: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteTargetKind {
    Arrangement,
    Silence,
    Session { clip_id: u64 },
}

/// Pure route-entry view matching the fields consumed by UI markers and validation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RouteEntryIdentity {
    pub track_id: u64,
    pub owner: RouteOwnerKind,
    pub pending_target: Option<RouteTargetKind>,
}

/// Derives current ownership and a queued Session target independently, so a
/// retrigger of the active cell can show both facts at once.
pub fn cell_route_markers(
    entry: Option<RouteEntryIdentity>,
    clip_id: u64,
    transport_playing: bool,
) -> [Option<CellRouteMarker>; 2] {
    let Some(entry) = entry else {
        return [None, None];
    };
    let current = match entry.owner {
        RouteOwnerKind::Session { clip_id: owner } if owner == clip_id => Some(CellRouteMarker {
            glyph: if transport_playing { "▶" } else { "●" },
            label: if transport_playing {
                "session owns track · transport running"
            } else {
                "session source selected · transport stopped"
            },
        }),
        _ => None,
    };
    let pending = match entry.pending_target {
        Some(RouteTargetKind::Session { clip_id: target }) if target == clip_id => {
            Some(CellRouteMarker {
                glyph: "…",
                label: "session clip launch pending",
            })
        }
        _ => None,
    };
    [current, pending]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteValidationKey {
    pub channel_identity: u64,
    pub audio_revision: u64,
    pub route_generation: u64,
    pub available_capability: bool,
    pub route_count: usize,
    pub project_generation: u64,
    pub project_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteValidationCache {
    pub key: RouteValidationKey,
    pub available: bool,
}

pub fn reset_channel_if_changed<S>(
    channel: &mut Option<u64>,
    snapshot: &mut Option<S>,
    identity: u64,
) -> bool {
    if *channel != Some(identity) {
        *snapshot = None;
        *channel = Some(identity);
        true
    } else {
        false
    }
}

pub fn cache_read_result<S: Copy + PartialEq, E: Copy + PartialEq, const N: usize>(
    snapshot: &mut Option<S>,
    entries: &mut [E; N],
    next: Option<S>,
    next_count: usize,
    scratch: &[E; N],
) -> bool {
    let Some(next) = next else {
        return false;
    };
    let changed = *snapshot != Some(next) || entries.get(..next_count) != scratch.get(..next_count);
    entries.copy_from_slice(scratch);
    *snapshot = Some(next);
    changed
}

pub fn route_entries_match_project(entries: &[RouteEntryIdentity], project: &Project) -> bool {
    if entries.len() != project.tracks().len() {
        return false;
    }
    for (index, entry) in entries.iter().enumerate() {
        let Some(track) = project
            .tracks()
            .iter()
            .find(|track| track.id().value() == entry.track_id)
        else {
            return false;
        };
        if entries[..index]
            .iter()
            .any(|previous| previous.track_id == entry.track_id)
        {
            return false;
        }
        let clip_belongs_to_track = |clip_id| {
            project.session_cells().iter().any(|cell| {
                cell.track() == track.id()
                    && (matches!(cell.content(), hex_project::SessionCellContent::Midi(clip) if clip.id().value() == clip_id)
                        || matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.id().value() == clip_id))
            })
        };
        if let RouteOwnerKind::Session { clip_id } = entry.owner
            && !clip_belongs_to_track(clip_id)
        {
            return false;
        }
        if let Some(RouteTargetKind::Session { clip_id }) = entry.pending_target
            && !clip_belongs_to_track(clip_id)
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        CellRouteMarker, RouteEntryIdentity, RouteOwnerKind, RouteTargetKind,
        SESSION_ROUTE_CAPACITY, cache_read_result, cell_route_markers, reset_channel_if_changed,
        route_entries_match_project,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project};

    fn telemetry_project() -> (
        Project,
        hex_project::TrackId,
        hex_project::TrackId,
        hex_project::ClipId,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 5).unwrap();
        let Created::Track(first_track) = editor
            .execute(Command::AddMidiTrack { name: "one".into() })
            .unwrap()
        else {
            panic!("first track")
        };
        let Created::Track(second_track) = editor
            .execute(Command::AddMidiTrack { name: "two".into() })
            .unwrap()
        else {
            panic!("second track")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionMidiClip {
                track: first_track,
                scene,
                name: "pattern".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("session clip")
        };
        (editor.project().clone(), first_track, second_track, clip)
    }

    fn route(track_id: u64, owner: RouteOwnerKind) -> RouteEntryIdentity {
        RouteEntryIdentity {
            track_id,
            owner,
            pending_target: None,
        }
    }

    #[test]
    fn route_identity_survives_reordering_but_rejects_removed_or_wrong_clips() {
        let (project, first, second, clip) = telemetry_project();
        let rows = [
            route(
                first.value(),
                RouteOwnerKind::Session {
                    clip_id: clip.value(),
                },
            ),
            route(second.value(), RouteOwnerKind::Arrangement),
        ];
        assert!(route_entries_match_project(&rows, &project));
        assert!(route_entries_match_project(&[rows[1], rows[0]], &project));
        let stale_clip = [
            route(
                first.value(),
                RouteOwnerKind::Session {
                    clip_id: clip.value().saturating_add(1),
                },
            ),
            rows[1],
        ];
        assert!(!route_entries_match_project(&stale_clip, &project));
        assert!(!route_entries_match_project(&rows[..1], &project));
        let empty = Project::new(48_000).unwrap();
        assert!(route_entries_match_project(&[], &empty));
    }

    #[test]
    fn cell_markers_show_current_and_pending_targets_independently() {
        let clip_a = 101;
        let clip_b = 202;
        let active = CellRouteMarker {
            glyph: "▶",
            label: "session owns track · transport running",
        };
        let selected = CellRouteMarker {
            glyph: "●",
            label: "session source selected · transport stopped",
        };
        let pending_a = CellRouteMarker {
            glyph: "…",
            label: "session clip launch pending",
        };
        let current_a_retrigger = RouteEntryIdentity {
            pending_target: Some(RouteTargetKind::Session { clip_id: clip_a }),
            ..route(42, RouteOwnerKind::Session { clip_id: clip_a })
        };
        assert_eq!(
            cell_route_markers(Some(current_a_retrigger), clip_a, true),
            [Some(active), Some(pending_a)]
        );

        let current_a_pending_b = RouteEntryIdentity {
            pending_target: Some(RouteTargetKind::Session { clip_id: clip_b }),
            ..route(42, RouteOwnerKind::Session { clip_id: clip_a })
        };
        assert_eq!(
            cell_route_markers(Some(current_a_pending_b), clip_a, true),
            [Some(active), None]
        );
        assert_eq!(
            cell_route_markers(Some(current_a_pending_b), clip_b, true),
            [None, Some(pending_a)]
        );

        let applied_b = route(42, RouteOwnerKind::Session { clip_id: clip_b });
        assert_eq!(
            cell_route_markers(Some(applied_b), clip_b, true),
            [Some(active), None]
        );
        let cancelled = route(42, RouteOwnerKind::Session { clip_id: clip_a });
        assert_eq!(
            cell_route_markers(Some(cancelled), clip_b, false),
            [None, None]
        );
        assert_eq!(
            cell_route_markers(Some(cancelled), clip_a, false),
            [Some(selected), None]
        );
    }

    #[test]
    fn failed_reads_keep_a_same_channel_copy_but_new_channels_and_empty_reads_replace_it() {
        assert_eq!(SESSION_ROUTE_CAPACITY, 256);
        let mut channel = None;
        let mut snapshot: Option<(u64, usize)> = None;
        let mut entries = [RouteEntryIdentity::default(); SESSION_ROUTE_CAPACITY];
        let mut scratch = [RouteEntryIdentity::default(); SESSION_ROUTE_CAPACITY];
        assert!(reset_channel_if_changed(&mut channel, &mut snapshot, 1));
        scratch[0] = route(42, RouteOwnerKind::Arrangement);
        assert!(cache_read_result(
            &mut snapshot,
            &mut entries,
            Some((9, 1)),
            1,
            &scratch,
        ));
        scratch[0] = RouteEntryIdentity {
            track_id: 99,
            ..RouteEntryIdentity::default()
        };
        assert!(!cache_read_result(
            &mut snapshot,
            &mut entries,
            None,
            0,
            &scratch,
        ));
        assert_eq!(snapshot, Some((9, 1)));
        assert_eq!(entries[0].track_id, 42);

        assert!(reset_channel_if_changed(&mut channel, &mut snapshot, 2));
        assert!(snapshot.is_none());
        assert!(cache_read_result(
            &mut snapshot,
            &mut entries,
            Some((0, 0)),
            0,
            &scratch,
        ));
        assert_eq!(snapshot, Some((0, 0)));
    }
}
