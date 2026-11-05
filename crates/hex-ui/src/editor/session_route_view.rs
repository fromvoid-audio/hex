//! Read-only, revision-bound view of accepted Session routes from the audio callback.
use hex_audio::prepared::{
    SESSION_ROUTE_CAPACITY, SessionRouteEntry, SessionRouteOwner, SessionRouteSnapshot,
    SessionRouteTarget, Status,
};
use hex_project::Project;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CellRouteMarker {
    pub glyph: &'static str,
    pub label: &'static str,
}

/// Derives current ownership and a queued Session target independently, so a
/// retrigger of the active cell can show both facts at once.
pub(super) fn cell_route_markers(
    entry: Option<SessionRouteEntry>,
    clip_id: u64,
    transport_playing: bool,
) -> [Option<CellRouteMarker>; 2] {
    let Some(entry) = entry else {
        return [None, None];
    };
    let current = match entry.owner {
        SessionRouteOwner::Session { clip_id: owner } if owner == clip_id => {
            Some(CellRouteMarker {
                glyph: if transport_playing { "▶" } else { "●" },
                label: if transport_playing {
                    "session owns track · transport running"
                } else {
                    "session source selected · transport stopped"
                },
            })
        }
        _ => None,
    };
    let pending = match entry.pending.map(|pending| pending.target) {
        Some(SessionRouteTarget::Session { clip_id: target }) if target == clip_id => {
            Some(CellRouteMarker {
                glyph: "…",
                label: "session clip launch pending",
            })
        }
        _ => None,
    };
    [current, pending]
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ValidationKey {
    channel_identity: u64,
    audio_revision: u64,
    route_generation: u64,
    capability: hex_engine::SessionCapabilityStatus,
    route_count: usize,
    project_generation: u64,
    project_revision: u64,
}

#[derive(Clone, Copy)]
struct ValidationCache {
    key: ValidationKey,
    available: bool,
}

/// Bounded UI cache. Failed reads leave the last good entries untouched on the
/// same live channel; all successful snapshots replace them, including empty ones.
pub(super) struct RouteView {
    channel: Option<u64>,
    snapshot: Option<SessionRouteSnapshot>,
    scratch: [SessionRouteEntry; SESSION_ROUTE_CAPACITY],
    entries: [SessionRouteEntry; SESSION_ROUTE_CAPACITY],
    validation: Option<ValidationCache>,
}

impl Default for RouteView {
    fn default() -> Self {
        Self {
            channel: None,
            snapshot: None,
            scratch: std::array::from_fn(|_| SessionRouteEntry::default()),
            entries: std::array::from_fn(|_| SessionRouteEntry::default()),
            validation: None,
        }
    }
}

impl RouteView {
    pub fn poll(&mut self, identity: u64, status: &Status) -> bool {
        let channel_changed =
            reset_channel_if_changed(&mut self.channel, &mut self.snapshot, identity);
        if channel_changed {
            self.validation = None;
        }
        let read = status.session_routes(&mut self.scratch);
        cache_read_result(&mut self.snapshot, &mut self.entries, read, &self.scratch)
            || channel_changed
    }

    pub fn clear(&mut self) {
        self.channel = None;
        self.snapshot = None;
        self.validation = None;
    }

    pub fn matching_snapshot(
        &self,
        identity: u64,
        audible: Option<(u64, u64, u64)>,
        displayed_revision: u64,
        displayed_generation: u64,
    ) -> Option<SessionRouteSnapshot> {
        if self.channel? != identity {
            return None;
        }
        let snapshot = self.snapshot?;
        let (audio_revision, project_revision, project_generation) = audible?;
        (snapshot.audio_revision == audio_revision
            && project_revision == displayed_revision
            && project_generation == displayed_generation)
            .then_some(snapshot)
    }

    pub fn routes_available(
        &mut self,
        snapshot: SessionRouteSnapshot,
        project: &Project,
        project_revision: u64,
        project_generation: u64,
    ) -> bool {
        let Some(channel) = self.channel else {
            return false;
        };
        let key = ValidationKey {
            channel_identity: channel,
            audio_revision: snapshot.audio_revision,
            route_generation: snapshot.route_state_generation,
            capability: snapshot.capability,
            route_count: snapshot.count,
            project_generation,
            project_revision,
        };
        if let Some(cached) = self.validation.filter(|cached| cached.key == key) {
            return cached.available;
        }
        let count = snapshot.count.min(self.entries.len());
        let available = snapshot.capability == hex_engine::SessionCapabilityStatus::Available
            && snapshot.count == count
            && route_entries_match_project(&self.entries[..count], project);
        self.validation = Some(ValidationCache { key, available });
        available
    }

    pub fn entry_for_track(
        &self,
        snapshot: SessionRouteSnapshot,
        routes_ready: bool,
        track_id: u64,
    ) -> Option<SessionRouteEntry> {
        if !routes_ready {
            return None;
        }
        self.entries
            .get(..snapshot.count)?
            .iter()
            .find(|entry| entry.track_id == track_id)
            .copied()
    }
}

fn reset_channel_if_changed(
    channel: &mut Option<u64>,
    snapshot: &mut Option<SessionRouteSnapshot>,
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

fn cache_read_result(
    snapshot: &mut Option<SessionRouteSnapshot>,
    entries: &mut [SessionRouteEntry; SESSION_ROUTE_CAPACITY],
    next: Option<SessionRouteSnapshot>,
    scratch: &[SessionRouteEntry; SESSION_ROUTE_CAPACITY],
) -> bool {
    let Some(next) = next else {
        return false;
    };
    let changed = *snapshot != Some(next) || entries.get(..next.count) != scratch.get(..next.count);
    entries.copy_from_slice(scratch);
    *snapshot = Some(next);
    changed
}

fn route_entries_match_project(entries: &[SessionRouteEntry], project: &Project) -> bool {
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
        if let SessionRouteOwner::Session { clip_id } = entry.owner
            && !clip_belongs_to_track(clip_id)
        {
            return false;
        }
        if let Some(SessionRouteTarget::Session { clip_id }) =
            entry.pending.map(|pending| pending.target)
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
        CellRouteMarker, RouteView, cache_read_result, cell_route_markers,
        reset_channel_if_changed, route_entries_match_project,
    };
    use hex_audio::prepared::{
        SESSION_ROUTE_CAPACITY, SessionPendingRoute, SessionRouteEntry, SessionRouteOwner,
        SessionRouteSnapshot, SessionRouteTarget, Status,
    };
    use hex_engine::SessionCapabilityStatus;
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::sync::Arc;

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

    fn route(track_id: u64, owner: SessionRouteOwner) -> SessionRouteEntry {
        SessionRouteEntry {
            track_id,
            owner,
            active_origin: None,
            audio_launch_frame: None,
            pending: None,
        }
    }

    fn make_snapshot(
        audio_revision: u64,
        count: usize,
        capability: SessionCapabilityStatus,
    ) -> SessionRouteSnapshot {
        SessionRouteSnapshot {
            audio_revision,
            count,
            capability,
            route_state_generation: 7,
            performance_at_update: 100,
            clock_anchor: None,
        }
    }

    #[test]
    fn snapshot_requires_exact_audio_revision_project_revision_and_generation() {
        let mut view = RouteView::default();
        let status = Arc::new(Status::default());
        let _ = view.poll(1, &status);
        let scratch = std::array::from_fn(|_| SessionRouteEntry::default());
        let exact = make_snapshot(4, 0, SessionCapabilityStatus::Available);
        assert!(cache_read_result(
            &mut view.snapshot,
            &mut view.entries,
            Some(exact),
            &scratch,
        ));
        let matching = view.matching_snapshot(1, Some((4, 8, 2)), 8, 2);
        assert_eq!(matching, Some(exact));
        assert!(view.matching_snapshot(1, Some((5, 8, 2)), 8, 2).is_none());
        assert!(view.matching_snapshot(1, Some((4, 9, 2)), 8, 2).is_none());
        assert!(view.matching_snapshot(1, Some((4, 8, 3)), 8, 2).is_none());
    }

    #[test]
    fn route_identity_survives_reordering_but_rejects_removed_or_wrong_clips() {
        let (project, first, second, clip) = telemetry_project();
        let rows = [
            route(
                first.value(),
                SessionRouteOwner::Session {
                    clip_id: clip.value(),
                },
            ),
            route(second.value(), SessionRouteOwner::Arrangement),
        ];
        assert!(route_entries_match_project(&rows, &project));
        assert!(route_entries_match_project(&[rows[1], rows[0]], &project));
        let stale_clip = [
            route(
                first.value(),
                SessionRouteOwner::Session {
                    clip_id: clip.value().saturating_add(1),
                },
            ),
            rows[1],
        ];
        assert!(!route_entries_match_project(&stale_clip, &project));
        assert!(!route_entries_match_project(&rows[..1], &project));
    }

    #[test]
    fn pending_unavailable_and_empty_routes_are_distinguished() {
        let (project, first, second, clip) = telemetry_project();
        let mut view = RouteView::default();
        let status = Arc::new(Status::default());
        let _ = view.poll(1, &status);
        let entries = [
            SessionRouteEntry {
                pending: Some(SessionPendingRoute {
                    target: SessionRouteTarget::Session {
                        clip_id: clip.value(),
                    },
                    performance_frame: 100,
                    beat_raw: 64,
                }),
                ..route(first.value(), SessionRouteOwner::Arrangement)
            },
            route(second.value(), SessionRouteOwner::Silence),
        ];
        let mut scratch = std::array::from_fn(|_| SessionRouteEntry::default());
        scratch[..entries.len()].copy_from_slice(&entries);
        let available = make_snapshot(2, 2, SessionCapabilityStatus::Available);
        assert!(cache_read_result(
            &mut view.snapshot,
            &mut view.entries,
            Some(available),
            &scratch,
        ));
        assert!(view.routes_available(available, &project, 10, 3));
        assert_eq!(
            view.entry_for_track(available, true, first.value())
                .and_then(|entry| entry.pending.map(|pending| pending.target)),
            Some(SessionRouteTarget::Session {
                clip_id: clip.value()
            })
        );
        assert!(!view.routes_available(
            make_snapshot(2, 2, SessionCapabilityStatus::Legacy),
            &project,
            10,
            3,
        ));
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
        let current_a_retrigger = SessionRouteEntry {
            pending: Some(SessionPendingRoute {
                target: SessionRouteTarget::Session { clip_id: clip_a },
                performance_frame: 120,
                beat_raw: 64,
            }),
            ..route(42, SessionRouteOwner::Session { clip_id: clip_a })
        };
        assert_eq!(
            cell_route_markers(Some(current_a_retrigger), clip_a, true),
            [Some(active), Some(pending_a)]
        );

        let current_a_pending_b = SessionRouteEntry {
            pending: Some(SessionPendingRoute {
                target: SessionRouteTarget::Session { clip_id: clip_b },
                performance_frame: 140,
                beat_raw: 96,
            }),
            ..route(42, SessionRouteOwner::Session { clip_id: clip_a })
        };
        assert_eq!(
            cell_route_markers(Some(current_a_pending_b), clip_a, true),
            [Some(active), None]
        );
        assert_eq!(
            cell_route_markers(Some(current_a_pending_b), clip_b, true),
            [None, Some(pending_a)]
        );

        // Applying a launch replaces the queued target with the new owner.
        let applied_b = route(42, SessionRouteOwner::Session { clip_id: clip_b });
        assert_eq!(
            cell_route_markers(Some(applied_b), clip_b, true),
            [Some(active), None]
        );
        // Cancel removes only the queued target; the existing owner remains selected.
        let cancelled = route(42, SessionRouteOwner::Session { clip_id: clip_a });
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
        let mut snapshot = None;
        let mut entries = std::array::from_fn(|_| SessionRouteEntry::default());
        let mut scratch = std::array::from_fn(|_| SessionRouteEntry::default());
        let first = 1;
        assert!(reset_channel_if_changed(&mut channel, &mut snapshot, first));
        let good = make_snapshot(9, 1, SessionCapabilityStatus::Available);
        scratch[0] = route(42, SessionRouteOwner::Arrangement);
        assert!(cache_read_result(
            &mut snapshot,
            &mut entries,
            Some(good),
            &scratch,
        ));
        // A failed bounded read may have partially overwritten scratch; it is ignored.
        scratch[0] = SessionRouteEntry {
            track_id: 99,
            ..SessionRouteEntry::default()
        };
        assert!(!cache_read_result(
            &mut snapshot,
            &mut entries,
            None,
            &scratch
        ));
        assert_eq!(snapshot, Some(good));
        assert_eq!(entries[0].track_id, 42);

        let second = 2;
        assert!(reset_channel_if_changed(
            &mut channel,
            &mut snapshot,
            second
        ));
        assert!(snapshot.is_none());
        let empty = make_snapshot(0, 0, SessionCapabilityStatus::Legacy);
        assert!(cache_read_result(
            &mut snapshot,
            &mut entries,
            Some(empty),
            &scratch,
        ));
        assert_eq!(snapshot, Some(empty));
        assert_eq!(snapshot.expect("empty snapshot").count, 0);
    }

    #[test]
    fn recovery_and_fault_reset_drop_channel_and_validation() {
        let mut view = RouteView::default();
        let status = Arc::new(Status::default());
        let _ = view.poll(1, &status);
        assert!(view.channel.is_some());
        assert_eq!(Arc::strong_count(&status), 1); // no escaping observer owner
        view.validation = Some(super::ValidationCache {
            key: super::ValidationKey {
                channel_identity: 1,
                audio_revision: 1,
                route_generation: 1,
                capability: SessionCapabilityStatus::Available,
                route_count: 1,
                project_generation: 1,
                project_revision: 1,
            },
            available: true,
        });
        view.clear();
        assert!(view.channel.is_none());
        assert!(view.snapshot.is_none());
        assert!(view.validation.is_none());
    }
}
