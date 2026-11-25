//! Audio-status adapter over [`hex_ui_model::session_route_view`].
use hex_audio::prepared::{
    SESSION_ROUTE_CAPACITY, SessionRouteEntry, SessionRouteOwner, SessionRouteSnapshot,
    SessionRouteTarget, Status,
};
use hex_project::Project;
pub(super) use hex_ui_model::session_route_view::CellRouteMarker;
use hex_ui_model::session_route_view::{
    RouteEntryIdentity, RouteOwnerKind, RouteTargetKind, RouteValidationCache, RouteValidationKey,
    cache_read_result, reset_channel_if_changed, route_entries_match_project,
};

fn entry_identity(entry: SessionRouteEntry) -> RouteEntryIdentity {
    RouteEntryIdentity {
        track_id: entry.track_id,
        owner: match entry.owner {
            SessionRouteOwner::Arrangement => RouteOwnerKind::Arrangement,
            SessionRouteOwner::Silence => RouteOwnerKind::Silence,
            SessionRouteOwner::Session { clip_id } => RouteOwnerKind::Session { clip_id },
        },
        pending_target: entry.pending.map(|pending| match pending.target {
            SessionRouteTarget::Arrangement => RouteTargetKind::Arrangement,
            SessionRouteTarget::Silence => RouteTargetKind::Silence,
            SessionRouteTarget::Session { clip_id } => RouteTargetKind::Session { clip_id },
        }),
    }
}

/// Derives current ownership and a queued Session target independently, so a
/// retrigger of the active cell can show both facts at once.
pub(super) fn cell_route_markers(
    entry: Option<SessionRouteEntry>,
    clip_id: u64,
    transport_playing: bool,
) -> [Option<CellRouteMarker>; 2] {
    hex_ui_model::session_route_view::cell_route_markers(
        entry.map(entry_identity),
        clip_id,
        transport_playing,
    )
}

/// Bounded UI cache. Failed reads leave the last good entries untouched on the
/// same live channel; all successful snapshots replace them, including empty ones.
pub(super) struct RouteView {
    channel: Option<u64>,
    snapshot: Option<SessionRouteSnapshot>,
    scratch: [SessionRouteEntry; SESSION_ROUTE_CAPACITY],
    entries: [SessionRouteEntry; SESSION_ROUTE_CAPACITY],
    validation: Option<RouteValidationCache>,
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
        let count = read.map_or(0, |snapshot| snapshot.count);
        cache_read_result(
            &mut self.snapshot,
            &mut self.entries,
            read,
            count,
            &self.scratch,
        ) || channel_changed
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
        let available_capability =
            snapshot.capability == hex_engine::SessionCapabilityStatus::Available;
        let key = RouteValidationKey {
            channel_identity: channel,
            audio_revision: snapshot.audio_revision,
            route_generation: snapshot.route_state_generation,
            available_capability,
            route_count: snapshot.count,
            project_generation,
            project_revision,
        };
        if let Some(cached) = self.validation.filter(|cached| cached.key == key) {
            return cached.available;
        }
        let count = snapshot.count.min(self.entries.len());
        let mut identities = [RouteEntryIdentity::default(); SESSION_ROUTE_CAPACITY];
        for (slot, entry) in identities.iter_mut().zip(&self.entries[..count]) {
            *slot = entry_identity(*entry);
        }
        let available = available_capability
            && snapshot.count == count
            && route_entries_match_project(&identities[..count], project);
        self.validation = Some(RouteValidationCache { key, available });
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

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{RouteView, cache_read_result};
    use hex_audio::prepared::{
        SessionPendingRoute, SessionRouteEntry, SessionRouteOwner, SessionRouteSnapshot,
        SessionRouteTarget, Status,
    };
    use hex_engine::SessionCapabilityStatus;
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::sync::Arc;

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
            exact.count,
            &scratch,
        ));
        let matching = view.matching_snapshot(1, Some((4, 8, 2)), 8, 2);
        assert_eq!(matching, Some(exact));
        assert!(view.matching_snapshot(1, Some((5, 8, 2)), 8, 2).is_none());
        assert!(view.matching_snapshot(1, Some((4, 9, 2)), 8, 2).is_none());
        assert!(view.matching_snapshot(1, Some((4, 8, 3)), 8, 2).is_none());
    }

    #[test]
    fn pending_unavailable_and_clear_lifecycle_match_audio_status() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 5).unwrap();
        let Created::Track(first) = editor
            .execute(Command::AddMidiTrack { name: "one".into() })
            .unwrap()
        else {
            panic!("first")
        };
        let Created::Track(second) = editor
            .execute(Command::AddMidiTrack { name: "two".into() })
            .unwrap()
        else {
            panic!("second")
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
                track: first,
                scene,
                name: "pattern".into(),
                source_length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut view = RouteView::default();
        let status = Arc::new(Status::default());
        let _ = view.poll(1, &status);
        assert_eq!(Arc::strong_count(&status), 1);
        let entries = [
            SessionRouteEntry {
                track_id: first.value(),
                owner: SessionRouteOwner::Arrangement,
                active_origin: None,
                audio_launch_frame: None,
                pending: Some(SessionPendingRoute {
                    target: SessionRouteTarget::Session {
                        clip_id: clip.value(),
                    },
                    performance_frame: 100,
                    beat_raw: 64,
                }),
            },
            SessionRouteEntry {
                track_id: second.value(),
                owner: SessionRouteOwner::Silence,
                active_origin: None,
                audio_launch_frame: None,
                pending: None,
            },
        ];
        let mut scratch = std::array::from_fn(|_| SessionRouteEntry::default());
        scratch[..entries.len()].copy_from_slice(&entries);
        let available = make_snapshot(2, 2, SessionCapabilityStatus::Available);
        assert!(cache_read_result(
            &mut view.snapshot,
            &mut view.entries,
            Some(available),
            available.count,
            &scratch,
        ));
        assert!(view.routes_available(available, editor.project(), 10, 3));
        assert_eq!(
            view.entry_for_track(available, true, first.value())
                .and_then(|entry| entry.pending.map(|pending| pending.target)),
            Some(SessionRouteTarget::Session {
                clip_id: clip.value()
            })
        );
        assert!(!view.routes_available(
            make_snapshot(2, 2, SessionCapabilityStatus::Legacy),
            editor.project(),
            10,
            3,
        ));
        view.clear();
        assert!(view.channel.is_none());
        assert!(view.snapshot.is_none());
        assert!(view.validation.is_none());
    }
}
