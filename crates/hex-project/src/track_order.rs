//! Ordinary track display order; stable identities own content and routing.
use crate::{Created, Invalid, Project, TrackId};

impl Project {
    pub(crate) fn move_track_before(
        &mut self,
        track: TrackId,
        before: Option<TrackId>,
    ) -> Result<Created, Invalid> {
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
