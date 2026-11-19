//! Ordinary track display order; stable identities own content and routing.
use crate::{Created, Invalid, Project, TrackId};
use std::sync::Arc;

/// Frozen command membership, not editor selection state. At most MAX_TRACKS
/// distinct IDs share one immutable, exactly sized allocation across a captured
/// gesture, its queued request and receipt. Project membership is checked when
/// applying the command; input order never changes the members' relative order.
/// GroupTracks uses the last member to choose the new group's destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackSelectionIds(Arc<[TrackId]>);

impl TrackSelectionIds {
    pub fn new(ids: &[TrackId]) -> Result<Self, Invalid> {
        if ids.is_empty() || ids.len() > crate::MAX_TRACKS {
            return Err(Invalid(
                "track operation needs a bounded non-empty selection",
            ));
        }
        let mut members = Vec::new();
        members
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve track selection"))?;
        for id in ids {
            if members.contains(id) {
                return Err(Invalid("track selection contains duplicate tracks"));
            }
            members.push(*id);
        }
        Ok(Self(members.into()))
    }

    pub fn ids(&self) -> &[TrackId] {
        &self.0
    }
}

impl Project {
    pub(crate) fn move_tracks_before(
        &mut self,
        selection: &TrackSelectionIds,
        before: Option<TrackId>,
    ) -> Result<Created, Invalid> {
        if self.has_group_tracks() {
            let destination = self.track_destination_before(before)?;
            return self.reparent_tracks(selection, destination);
        }
        let ids = selection.ids();
        if let [track] = ids {
            return self.move_track_before(*track, before);
        }
        if ids.iter().any(|id| self.track(*id).is_none()) {
            return Err(Invalid("track selection no longer exists"));
        }
        if before.is_some_and(|id| self.track(id).is_none()) {
            return Err(Invalid("track order target no longer exists"));
        }
        if before.is_some_and(|id| ids.contains(&id)) {
            return Ok(Created::None);
        }
        let first = self
            .tracks
            .iter()
            .position(|track| ids.contains(&track.id()))
            .ok_or(Invalid("track selection no longer exists"))?;
        let after = first
            .checked_add(ids.len())
            .filter(|index| *index <= self.tracks.len())
            .ok_or(Invalid("track selection exceeds track inventory"))?;
        if self.tracks[first..after]
            .iter()
            .all(|track| ids.contains(&track.id()))
            && self.tracks.get(after).map(|track| track.id()) == before
        {
            return Ok(Created::None);
        }

        // Reserve before mutation, then move complete values in their original
        // order. The inventory retains its existing capacity, so reinsertion
        // neither clones musical/device state nor allocates new identities.
        let mut moved = Vec::new();
        moved
            .try_reserve_exact(ids.len())
            .map_err(|_| Invalid("could not reserve reordered tracks"))?;
        for index in (0..self.tracks.len()).rev() {
            if ids.contains(&self.tracks[index].id()) {
                moved.push(self.tracks.remove(index));
            }
        }
        moved.reverse();
        let destination = match before {
            Some(anchor) => self
                .tracks
                .iter()
                .position(|track| track.id() == anchor)
                .ok_or(Invalid("track order target no longer exists"))?,
            None => self.tracks.len(),
        };
        for (offset, track) in moved.into_iter().enumerate() {
            let index = destination
                .checked_add(offset)
                .filter(|index| *index <= self.tracks.len())
                .ok_or(Invalid("track insertion exceeds track inventory"))?;
            self.tracks.insert(index, track);
        }
        Ok(Created::None)
    }

    pub(crate) fn move_track_before(
        &mut self,
        track: TrackId,
        before: Option<TrackId>,
    ) -> Result<Created, Invalid> {
        if self.has_group_tracks() {
            let selection = TrackSelectionIds::new(&[track])?;
            let destination = self.track_destination_before(before)?;
            return self.reparent_tracks(&selection, destination);
        }
        let source = self
            .tracks
            .iter()
            .position(|row| row.id() == track)
            .ok_or(Invalid("track no longer exists"))?;
        let destination = match before {
            Some(anchor) => self
                .tracks
                .iter()
                .position(|row| row.id() == anchor)
                .ok_or(Invalid("track order target no longer exists"))?,
            None => self.tracks.len(),
        };
        // Resolve both IDs in the original order before mutation. Rotating the
        // intervening rows moves complete Track values without cloning content,
        // allocating IDs, or changing any ID-owned reference.
        if source < destination {
            self.tracks[source..destination].rotate_left(1);
        } else if source > destination {
            self.tracks[destination..=source].rotate_right(1);
        }
        Ok(Created::None)
    }
}
