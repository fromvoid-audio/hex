//! Canonical track hierarchy and subtree edits; no playback or view ownership.
//!
//! Tracks remain one bounded preorder inventory. Parent identities name earlier
//! group rows whose complete subtrees are contiguous. Read-only projections and
//! destinations carry stable IDs, not permission to edit stale row indices.
use crate::{Created, Invalid, Project, Track, TrackId, TrackSelectionIds};
use std::ops::Range;

/// Groups share the existing total track limit. Eight group ancestors may
/// contain an ordinary leaf; a ninth group is refused before publication.
pub const MAX_GROUP_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackKind {
    #[default]
    Ordinary,
    Group,
}

/// A boundary between immediate children of one parent (None means Main).
/// `before: None` appends inside that parent. A named boundary may also be the
/// first row after the parent's entire subtree. The command validates both IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackDestination {
    pub parent_group: Option<TrackId>,
    pub before: Option<TrackId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackHierarchyRow {
    id: TrackId,
    kind: TrackKind,
    parent_group: Option<TrackId>,
    depth: usize,
    start: usize,
    end: usize,
}
impl TrackHierarchyRow {
    pub fn id(&self) -> TrackId {
        self.id
    }
    pub fn kind(&self) -> TrackKind {
        self.kind
    }
    pub fn parent_group(&self) -> Option<TrackId> {
        self.parent_group
    }
    pub fn depth(&self) -> usize {
        self.depth
    }
    /// Indices belong only to this immutable projection. Commands use IDs.
    pub fn subtree_range(&self) -> Range<usize> {
        self.start..self.end
    }
}

/// Bounded read-only projection of one Project. Consumers must invalidate it
/// with their canonical revision; it is not an independently editable tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackHierarchy {
    rows: Vec<TrackHierarchyRow>,
}
impl TrackHierarchy {
    pub fn rows(&self) -> &[TrackHierarchyRow] {
        &self.rows
    }
    pub fn row(&self, id: TrackId) -> Option<&TrackHierarchyRow> {
        self.rows.iter().find(|row| row.id == id)
    }
    fn roots(&self, selection: &TrackSelectionIds) -> Result<Vec<usize>, Invalid> {
        if selection.ids().iter().any(|id| self.row(*id).is_none()) {
            return Err(Invalid("selected track no longer exists"));
        }
        let mut roots = Vec::new();
        roots
            .try_reserve_exact(selection.ids().len())
            .map_err(|_| Invalid("could not reserve track selection roots"))?;
        let mut covered_end = 0;
        for (index, row) in self.rows.iter().enumerate() {
            if index >= covered_end && selection.ids().contains(&row.id) {
                roots.push(index);
                covered_end = row.end;
            }
        }
        Ok(roots)
    }
    fn selected(&self, roots: &[usize]) -> Result<[bool; crate::MAX_TRACKS], Invalid> {
        let mut selected = [false; crate::MAX_TRACKS];
        for &index in roots {
            let row = self
                .rows
                .get(index)
                .ok_or(Invalid("track selection root is missing"))?;
            let flags = selected
                .get_mut(row.start..row.end)
                .ok_or(Invalid("track subtree exceeds inventory"))?;
            flags.fill(true);
        }
        Ok(selected)
    }
    pub(crate) fn insertion(&self, destination: TrackDestination) -> Result<usize, Invalid> {
        let parent = destination
            .parent_group
            .map(|id| {
                self.row(id)
                    .filter(|row| row.kind == TrackKind::Group)
                    .ok_or(Invalid("track parent must be an existing group"))
            })
            .transpose()?;
        let index = match destination.before {
            Some(id) => {
                self.row(id)
                    .ok_or(Invalid("track insertion anchor no longer exists"))?
                    .start
            }
            None => parent.map_or(self.rows.len(), |row| row.end),
        };
        let valid = match parent {
            Some(parent) => {
                index == parent.end
                    || (index > parent.start
                        && self
                            .rows
                            .get(index)
                            .is_some_and(|row| row.parent_group == Some(parent.id)))
            }
            None => {
                index == self.rows.len()
                    || self
                        .rows
                        .get(index)
                        .is_some_and(|row| row.parent_group.is_none())
            }
        };
        if !valid {
            return Err(Invalid("track insertion anchor belongs to another group"));
        }
        Ok(index)
    }
}

impl Project {
    pub fn has_group_tracks(&self) -> bool {
        self.tracks.iter().any(Track::is_group)
    }

    pub fn track_hierarchy(&self) -> Result<TrackHierarchy, Invalid> {
        if self.tracks.len() > crate::MAX_TRACKS {
            return Err(Invalid("track limit reached"));
        }
        let mut rows: Vec<TrackHierarchyRow> = Vec::new();
        rows.try_reserve_exact(self.tracks.len())
            .map_err(|_| Invalid("could not reserve track hierarchy"))?;
        let mut stack: [Option<usize>; MAX_GROUP_DEPTH] = [None; MAX_GROUP_DEPTH];
        let mut open = 0usize;
        for (index, track) in self.tracks.iter().enumerate() {
            let depth = match track.parent_group {
                None => 0,
                Some(parent) => stack
                    .get(..open)
                    .and_then(|ancestors| {
                        ancestors.iter().position(|entry| {
                            entry
                                .and_then(|i| rows.get(i))
                                .is_some_and(|row| row.id == parent)
                        })
                    })
                    .and_then(|position| position.checked_add(1))
                    .ok_or(Invalid("track parent must precede its contiguous children"))?,
            };
            while open > depth {
                open -= 1;
                let group = stack
                    .get_mut(open)
                    .and_then(Option::take)
                    .ok_or(Invalid("track hierarchy ancestor is missing"))?;
                rows.get_mut(group)
                    .ok_or(Invalid("track hierarchy group is missing"))?
                    .end = index;
            }
            if track.is_group()
                && (!track.clips().is_empty()
                    || !track.midi_clips().is_empty()
                    || track.instrument().is_some())
            {
                return Err(Invalid(
                    "group tracks accept audio effects, not clips or instruments",
                ));
            }
            let end = index
                .checked_add(1)
                .ok_or(Invalid("track hierarchy index overflow"))?;
            rows.push(TrackHierarchyRow {
                id: track.id(),
                kind: track.kind,
                parent_group: track.parent_group,
                depth,
                start: index,
                end,
            });
            if track.is_group() {
                let entry = stack
                    .get_mut(depth)
                    .ok_or(Invalid("group nesting limit reached"))?;
                *entry = Some(index);
                open = depth
                    .checked_add(1)
                    .ok_or(Invalid("group nesting overflow"))?;
            }
        }
        while open > 0 {
            open -= 1;
            let group = stack
                .get_mut(open)
                .and_then(Option::take)
                .ok_or(Invalid("track hierarchy ancestor is missing"))?;
            rows.get_mut(group)
                .ok_or(Invalid("track hierarchy group is missing"))?
                .end = self.tracks.len();
        }
        Ok(TrackHierarchy { rows })
    }

    pub(crate) fn validate_track_hierarchy(&self) -> Result<(), Invalid> {
        // Preserve the ordinary editing path without allocating a projection or
        // scanning Session content when the document contains no hierarchy.
        if self
            .tracks
            .iter()
            .all(|t| t.kind == TrackKind::Ordinary && t.parent_group.is_none())
        {
            return Ok(());
        }
        let hierarchy = self.track_hierarchy()?;
        let mut groups = [None; crate::MAX_TRACKS];
        let mut count = 0usize;
        for row in hierarchy
            .rows()
            .iter()
            .filter(|row| row.kind == TrackKind::Group)
        {
            *groups
                .get_mut(count)
                .ok_or(Invalid("group inventory exceeds track limit"))? = Some(row.id);
            count = count
                .checked_add(1)
                .ok_or(Invalid("group count overflow"))?;
        }
        let groups = groups
            .get_mut(..count)
            .ok_or(Invalid("group inventory exceeds track limit"))?;
        groups.sort_unstable();
        if self
            .session
            .cells
            .iter()
            .any(|cell| groups.binary_search(&Some(cell.track())).is_ok())
        {
            return Err(Invalid("group tracks cannot contain console cells"));
        }
        Ok(())
    }

    pub fn expand_track_selection(
        &self,
        selection: &TrackSelectionIds,
    ) -> Result<TrackSelectionIds, Invalid> {
        if !self.has_group_tracks() {
            if selection.ids().iter().any(|id| self.track(*id).is_none()) {
                return Err(Invalid("selected track no longer exists"));
            }
            return Ok(selection.clone());
        }
        let hierarchy = self.track_hierarchy()?;
        let roots = hierarchy.roots(selection)?;
        let flags = hierarchy.selected(&roots)?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.tracks.len())
            .map_err(|_| Invalid("could not reserve subtree selection"))?;
        for (index, row) in hierarchy.rows().iter().enumerate() {
            if flags
                .get(index)
                .copied()
                .ok_or(Invalid("track selection exceeds inventory"))?
            {
                ids.push(row.id);
            }
        }
        TrackSelectionIds::new(&ids)
    }

    pub fn track_destination_before(
        &self,
        before: Option<TrackId>,
    ) -> Result<TrackDestination, Invalid> {
        let parent_group = before
            .map(|id| {
                self.track(id)
                    .ok_or(Invalid("track insertion anchor no longer exists"))
                    .map(Track::parent_group)
            })
            .transpose()?
            .flatten();
        Ok(TrackDestination {
            parent_group,
            before,
        })
    }

    pub fn track_destination_after_selection(
        &self,
        selection: &TrackSelectionIds,
    ) -> Result<TrackDestination, Invalid> {
        let hierarchy = self.track_hierarchy()?;
        let roots = hierarchy.roots(selection)?;
        let row = roots
            .last()
            .and_then(|index| hierarchy.rows.get(*index))
            .ok_or(Invalid("track selection is empty"))?;
        Ok(TrackDestination {
            parent_group: row.parent_group,
            before: self.tracks.get(row.end).map(Track::id),
        })
    }

    pub(crate) fn group_tracks(
        &mut self,
        selection: &TrackSelectionIds,
        name: String,
    ) -> Result<Created, Invalid> {
        let hierarchy = self.track_hierarchy()?;
        let roots = hierarchy.roots(selection)?;
        let first = roots
            .first()
            .and_then(|i| hierarchy.rows.get(*i))
            .ok_or(Invalid("track selection is empty"))?;
        if roots.iter().any(|index| {
            hierarchy
                .rows
                .get(*index)
                .is_none_or(|row| row.parent_group != first.parent_group)
        }) {
            return Err(Invalid("grouped tracks must share an immediate parent"));
        }
        let flags = hierarchy.selected(&roots)?;
        if first.depth >= MAX_GROUP_DEPTH
            || hierarchy.rows.iter().enumerate().any(|(index, row)| {
                flags.get(index).copied().unwrap_or(false)
                    && row.kind == TrackKind::Group
                    && row
                        .depth
                        .checked_add(1)
                        .is_none_or(|depth| depth >= MAX_GROUP_DEPTH)
            })
        {
            return Err(Invalid("group nesting limit reached"));
        }
        let capacity = self
            .tracks
            .len()
            .checked_add(1)
            .filter(|n| *n <= crate::MAX_TRACKS)
            .ok_or(Invalid("track limit reached"))?;
        let mut kept = Vec::new();
        let mut moved = Vec::new();
        kept.try_reserve_exact(capacity)
            .map_err(|_| Invalid("could not reserve grouped tracks"))?;
        moved
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| Invalid("could not reserve grouped tracks"))?;
        let color = self
            .tracks
            .get(first.start)
            .ok_or(Invalid("selected track is missing"))?
            .color;
        let mut group = crate::edit::new_track(self, name, TrackKind::Group, first.parent_group)?;
        group.color = color;
        let id = group.id();
        for (index, mut track) in std::mem::take(&mut self.tracks).into_iter().enumerate() {
            if flags
                .get(index)
                .copied()
                .ok_or(Invalid("track selection exceeds inventory"))?
            {
                if roots.iter().any(|root| {
                    hierarchy
                        .rows
                        .get(*root)
                        .is_some_and(|row| row.id == track.id())
                }) {
                    track.parent_group = Some(id);
                }
                moved.push(track);
            } else {
                kept.push(track);
            }
        }
        let insertion = first.start;
        if insertion > kept.len() {
            return Err(Invalid("group insertion exceeds inventory"));
        }
        kept.insert(insertion, group);
        for (offset, track) in moved.into_iter().enumerate() {
            let index = insertion
                .checked_add(1)
                .and_then(|i| i.checked_add(offset))
                .filter(|i| *i <= kept.len())
                .ok_or(Invalid("group insertion exceeds inventory"))?;
            kept.insert(index, track);
        }
        self.tracks = kept;
        Ok(Created::Track(id))
    }

    pub(crate) fn reparent_tracks(
        &mut self,
        selection: &TrackSelectionIds,
        destination: TrackDestination,
    ) -> Result<Created, Invalid> {
        if !self.has_group_tracks() && destination.parent_group.is_none() {
            return self.move_tracks_before(selection, destination.before);
        }
        let hierarchy = self.track_hierarchy()?;
        let roots = hierarchy.roots(selection)?;
        let flags = hierarchy.selected(&roots)?;
        let anchor = hierarchy.insertion(destination)?;
        if destination.parent_group.is_some_and(|id| {
            hierarchy
                .row(id)
                .and_then(|row| flags.get(row.start))
                .copied()
                .unwrap_or(false)
        }) {
            return Err(Invalid(
                "a group cannot move inside itself or its descendants",
            ));
        }
        if destination.before.is_some_and(|id| {
            hierarchy
                .row(id)
                .and_then(|row| flags.get(row.start))
                .copied()
                .unwrap_or(false)
        }) {
            return Ok(Created::None);
        }
        let insertion = flags
            .get(..anchor)
            .ok_or(Invalid("track insertion exceeds inventory"))?
            .iter()
            .filter(|selected| !**selected)
            .count();
        let mut kept = Vec::new();
        let mut moved = Vec::new();
        kept.try_reserve_exact(self.tracks.len())
            .map_err(|_| Invalid("could not reserve reordered tracks"))?;
        moved
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| Invalid("could not reserve reordered tracks"))?;
        for (index, mut track) in std::mem::take(&mut self.tracks).into_iter().enumerate() {
            if flags
                .get(index)
                .copied()
                .ok_or(Invalid("track selection exceeds inventory"))?
            {
                if roots.iter().any(|root| {
                    hierarchy
                        .rows
                        .get(*root)
                        .is_some_and(|row| row.id == track.id())
                }) {
                    track.parent_group = destination.parent_group;
                }
                moved.push(track);
            } else {
                kept.push(track);
            }
        }
        for (offset, track) in moved.into_iter().enumerate() {
            let index = insertion
                .checked_add(offset)
                .filter(|i| *i <= kept.len())
                .ok_or(Invalid("track insertion exceeds inventory"))?;
            kept.insert(index, track);
        }
        self.tracks = kept;
        Ok(Created::None)
    }

    pub(crate) fn ungroup_track(&mut self, group: TrackId) -> Result<Created, Invalid> {
        let track = self
            .track(group)
            .filter(|track| track.is_group())
            .ok_or(Invalid("selected track is not a group"))?;
        let parent = track.parent_group;
        for child in &mut self.tracks {
            if child.parent_group == Some(group) {
                child.parent_group = parent;
            }
        }
        self.remove_track_rows(&[group])?;
        Ok(Created::None)
    }

    /// Deleting a selected group deletes its complete subtree. Ungroup is the
    /// separate retaining operation. Source files and the asset table stay owned.
    pub(crate) fn remove_track_subtrees(&mut self, ids: &[TrackId]) -> Result<Created, Invalid> {
        let selection = TrackSelectionIds::new(ids)?;
        let expanded = self.expand_track_selection(&selection)?;
        self.remove_track_rows(expanded.ids())?;
        Ok(Created::None)
    }
    fn remove_track_rows(&mut self, ids: &[TrackId]) -> Result<(), Invalid> {
        for receiver in self
            .tracks()
            .iter()
            .filter(|track| ids.contains(&track.id()) && track.retained_source_path())
        {
            if let Some(source) = receiver.source_id()
                && self
                    .tracks()
                    .iter()
                    .any(|track| track.source_id() == Some(source) && !ids.contains(&track.id()))
            {
                return Err(Invalid(
                    "cannot delete the retained source path while linked tracks remain",
                ));
            }
        }
        let receivers = self
            .native_sources()
            .iter()
            .filter_map(|source| {
                source
                    .receiver()
                    .filter(|receiver| ids.contains(receiver))
                    .map(|_| source.id())
            })
            .collect::<Vec<_>>();
        self.tracks.retain(|track| !ids.contains(&track.id()));
        for source in receivers {
            self.set_native_source_receiver(source, None)?;
        }
        self.audio_feeds.retain(|route| {
            !matches!(route.source(), crate::RoutingOwner::Track(id) if ids.contains(&id))
                && !ids.contains(&route.destination())
        });
        self.midi_routes
            .retain(|route| !ids.contains(&route.source()) && !ids.contains(&route.destination()));
        self.session
            .cells
            .retain(|cell| !ids.contains(&cell.track()));
        Ok(())
    }
}
