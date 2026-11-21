//! Non-destructive intervals within an ordinary processing root or a frame-fixed
//! container of independently retained complete roots.
//! IDs are local to this sequence; callers scope references to clip/revision.
use crate::{AudioClipEdit, AudioEventFade, Clip, ClipId, Invalid, Project, WarpSettings};
use std::sync::Arc;

pub const MAX_AUDIO_EVENTS: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioEvent {
    /// Inactive events retain their source and geometry but contribute silence.
    pub active: bool,
    /// Independent saved amplitude after the source read/event fade and before
    /// the container fade/outer clip gain. Unity preserves the original samples.
    pub gain: crate::ClipGain,
    /// Zero reads the original root. A marker fork stays in its live beat
    /// clock. A grouped source reads its independently retained finite frames.
    pub source: u64,
    pub id: u64,
    /// Half-open positions in the retained root's clock.
    pub start: u64,
    pub end: u64,
    /// Original root coordinate played at `start`; for a retained source this
    /// is its finite prepared output frame, never a mutable/media-file offset.
    pub source_start: u64,
    /// Independent source-window envelope; absent is exact unity.
    pub fade: Option<AudioEventFade>,
}
impl AudioEvent {
    pub fn length(self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// Immutable full processing/source custody for one grouped event source.
/// The captured tempo preserves the original warp/fade projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedAudioRoot {
    pub id: u64,
    pub clip: Clip,
    pub tempo: Arc<crate::TempoMap>,
}

/// A marker fork of the same live musical source, sharing its media, mode,
/// source origin and tempo projection with the original processing root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioEventWarpSource {
    pub id: u64,
    pub markers: Arc<[crate::WarpMarker]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioEvents {
    pub sources: Vec<RetainedAudioRoot>,
    pub warp_sources: Vec<AudioEventWarpSource>,
    pub next_id: u64,
    pub events: Vec<AudioEvent>,
}

/// Inner edits of an Arrangement clip's retained processing root. Console
/// source-window one-shots have no such root and refuse these edits explicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEventEdit {
    SetGain {
        event: u64,
        gain: crate::ClipGain,
    },
    SetRetainedGain {
        sources: Vec<u64>,
        event: u64,
        gain: crate::ClipGain,
    },
    /// Set the activation of existing event identities atomically.
    SetActive {
        events: Vec<u64>,
        active: bool,
    },
    /// Set activation within one retained source without flattening its processing.
    SetRetainedActive {
        source: u64,
        events: Vec<u64>,
        active: bool,
    },
    /// Set activation inside a bounded path of retained source identities.
    SetRetainedPathActive {
        sources: Vec<u64>,
        events: Vec<u64>,
        active: bool,
    },
    /// Edit markers in one event without changing its clock or sibling reads.
    SetRetainedWarp {
        event: u64,
        settings: WarpSettings,
    },
    SetFade {
        event: u64,
        fade: Option<AudioEventFade>,
    },
    Split {
        at: u64,
    },
    SplitRange {
        start: u64,
        end: u64,
    },
    Move {
        event: u64,
        start: u64,
    },
    /// Copy one event to a signed root-clock position with a fresh identity.
    /// The destination may overwrite other events but must leave the source intact.
    Duplicate {
        event: u64,
        start: i64,
    },
    MoveRange {
        start: u64,
        end: u64,
        destination: u64,
    },
    Trim {
        event: u64,
        start: u64,
        end: u64,
    },
    Delete {
        start: u64,
        end: u64,
    },
    /// Clipboard coordinates are relative to its selected range; sources refer
    /// to this same retained root. Cross-root paste needs explicit source custody.
    Insert {
        at: u64,
        length: u64,
        events: Vec<AudioEvent>,
    },
    Paste {
        at: u64,
        length: u64,
        events: Vec<AudioEvent>,
    },
    /// Place one event anywhere on its clip's free event timeline. `start` is a
    /// signed root-clock position relative to the current root origin; a frame
    /// one-shot root first becomes a grouped timeline. A raw source loop keeps
    /// its source phase. The destination grows within the processing budget.
    Place {
        event: u64,
        start: i64,
    },
    /// Move a half-open selection to a signed root coordinate. Complete
    /// events keep their IDs. A split gives the right fragment a fresh ID.
    PlaceRange {
        start: u64,
        end: u64,
        destination: i64,
    },
}
impl AudioEvents {
    /// Re-express frame events in Q32.32 beats using the retained root origin.
    /// Both source endpoints must retain the destination span to within one
    /// raw beat tick of independent rounding. Unequal tempo segments may need
    /// a non-affine source program which this interval format cannot represent;
    /// refuse those atomically rather than trim or reinterpret source content.
    pub(crate) fn reproject_to_beats(
        &self,
        length: u64,
        mut position: impl FnMut(u64) -> Result<u64, Invalid>,
    ) -> Result<Self, Invalid> {
        if !self.sources.is_empty() {
            return Err(Invalid(
                "grouped audio uses a fixed frame clock; musical reprojection is unsupported",
            ));
        }
        if self.events.len() > MAX_AUDIO_EVENTS {
            return Err(Invalid("audio event timing projection exceeds event limit"));
        }
        let mut events = Vec::new();
        events
            .try_reserve_exact(self.events.len())
            .map_err(|_| Invalid("audio event timing projection allocation failed"))?;
        for event in &self.events {
            let mut projected = AudioEvent {
                active: event.active,
                gain: event.gain,
                source: event.source,
                id: event.id,
                start: position(event.start)?,
                end: position(event.end)?,
                source_start: position(event.source_start)?,
                fade: event
                    .fade
                    .map(|fade| fade.reproject(&mut position))
                    .transpose()?,
            };
            let source_end = position(
                event
                    .source_start
                    .checked_add(event.length())
                    .ok_or(Invalid("audio event source interval overflow"))?,
            )?;
            let span = projected
                .end
                .checked_sub(projected.start)
                .ok_or(Invalid("audio event timing projection is reversed"))?;
            let projected_end = projected
                .source_start
                .checked_add(span)
                .ok_or(Invalid("audio event source projection overflow"))?;
            if projected_end.abs_diff(source_end) > 1 {
                return Err(Invalid(
                    "audio event timing crosses unequal tempo segments; set one global tempo before enabling warp",
                ));
            }
            // One Q32.32 beat tick is smaller than a prepared sample at every
            // supported rate/tempo. Normalize only an endpoint rounding overrun;
            // the projected source start still resolves to its original frame.
            if projected_end > length && projected_end - length == 1 {
                projected.source_start = projected
                    .source_start
                    .checked_sub(1)
                    .ok_or(Invalid("audio event source projection precedes its root"))?;
            }
            events.push(projected);
        }
        let projected = Self {
            sources: Vec::new(),
            warp_sources: Vec::new(),
            next_id: self.next_id,
            events,
        };
        projected.validate(length)?;
        Ok(projected)
    }
    pub fn identity(length: u64) -> Self {
        Self {
            sources: Vec::new(),
            warp_sources: Vec::new(),
            next_id: 2,
            events: vec![AudioEvent {
                active: true,
                gain: crate::ClipGain::UNITY,
                source: 0,
                id: 1,
                start: 0,
                end: length,
                source_start: 0,
                fade: None,
            }],
        }
    }
    pub fn validate(&self, length: u64) -> Result<(), Invalid> {
        if self.events.len() > MAX_AUDIO_EVENTS
            || self.sources.len() > MAX_AUDIO_EVENTS
            || self.warp_sources.len() > MAX_AUDIO_EVENTS
            || (!self.sources.is_empty() && !self.warp_sources.is_empty())
            || self.next_id == 0
        {
            return Err(Invalid("audio event limit or identity is invalid"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for source in &self.sources {
            if source.id == 0 || !ids.insert(source.id) {
                return Err(Invalid("invalid retained audio root identity"));
            }
        }
        for source in &self.warp_sources {
            if source.id == 0 || !ids.insert(source.id) {
                return Err(Invalid("invalid event warp source identity"));
            }
        }
        self.validate_geometry(length)
    }
    fn validate_geometry(&self, length: u64) -> Result<(), Invalid> {
        if self.next_id == 0 {
            return Err(Invalid("audio event identity is invalid"));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut previous_end = 0;
        for event in &self.events {
            if event.id == 0
                || event.id >= self.next_id
                || !ids.insert(event.id)
                || event.start < previous_end
                || event.start >= event.end
                || event.end > length
                || event
                    .source_start
                    .checked_add(event.length())
                    .is_none_or(|end| end > self.source_length(event.source, length).unwrap_or(0))
            {
                return Err(Invalid(
                    "audio events must have unique identities and bounded nonoverlapping intervals",
                ));
            }
            if let Some(fade) = event.fade {
                fade.validate(
                    self.source_length(event.source, length).unwrap_or(0),
                    event.source_start,
                    event.length(),
                )?;
            }
            previous_end = event.end;
        }
        Ok(())
    }
    pub(crate) fn source_length(&self, source: u64, own_length: u64) -> Option<u64> {
        if source == 0 {
            self.sources.is_empty().then_some(own_length)
        } else if self.warp_source(source).is_some() {
            Some(own_length)
        } else {
            // Canonical writers assign contiguous IDs. Preserve the validated
            // arbitrary-ID reader contract through a bounded fallback.
            source
                .checked_sub(1)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| self.sources.get(index))
                .filter(|root| root.id == source)
                .or_else(|| self.sources.iter().find(|root| root.id == source))
                .map(|root| root.clip.duration())
        }
    }
    pub fn warp_source(&self, id: u64) -> Option<&AudioEventWarpSource> {
        self.warp_sources.iter().find(|source| source.id == id)
    }
    fn id(&mut self) -> Result<u64, Invalid> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or(Invalid("audio event identity exhausted"))?;
        Ok(id)
    }
    fn carve(&mut self, start: u64, end: u64) -> Result<(), Invalid> {
        let mut result = Vec::new();
        for event in std::mem::take(&mut self.events) {
            if event.end <= start || event.start >= end {
                result.push(event);
                continue;
            }
            if event.start < start {
                result.push(AudioEvent {
                    end: start,
                    ..event
                });
            }
            if event.end > end {
                result.push(AudioEvent {
                    id: self.id()?,
                    start: end,
                    source_start: event.source_start + (end - event.start),
                    ..event
                });
            }
        }
        self.events = result;
        Ok(())
    }
    pub fn edited(&self, operation: AudioEventEdit, length: u64) -> Result<Self, Invalid> {
        // Cropped views may add two virtual boundary fragments. Only the final
        // canonical result consumes the event limit, so reducing edits stay usable.
        if self.events.len() > MAX_AUDIO_EVENTS + 2 {
            return Err(Invalid("audio event view limit reached"));
        }
        if matches!(
            operation,
            AudioEventEdit::Place { .. } | AudioEventEdit::PlaceRange { .. }
        ) {
            return Err(Invalid(
                "free audio event placement edits the clip's whole timeline",
            ));
        }
        let next = self.edited_inner(operation, length)?;
        next.validate(length)?;
        Ok(next)
    }

    fn edited_in_root(
        &self,
        operation: AudioEventEdit,
        root: &AudioClipEdit,
    ) -> Result<Self, Invalid> {
        let length = root.domain.clock.length();
        match operation {
            AudioEventEdit::SetRetainedWarp { event, settings } if self.sources.is_empty() => {
                let mut next = self.clone();
                next.set_split_warp(event, settings, root)?;
                next.validate(length)?;
                Ok(next)
            }
            operation => self.edited(operation, length),
        }
    }
    // Retained leaf metadata keeps its original processing/source custody. The
    // command owner validates the restored leaf against project media before commit.
    fn edit_retained(&mut self, source: u64, operation: AudioEventEdit) -> Result<(), Invalid> {
        let root = self
            .sources
            .iter_mut()
            .find(|root| root.id == source)
            .ok_or(Invalid("retained audio source no longer exists"))?;
        let mut edit = root.clip.audio_event_root();
        let original = root.clip.editable_audio_events()?;
        let changed = original.edited(operation, edit.domain.clock.length())?;
        if changed != original {
            Arc::make_mut(&mut edit.domain).events = Some(Arc::new(changed));
            root.clip.edit = Some(Arc::new(edit));
        }
        Ok(())
    }
    fn set_retained_warp(&mut self, event: u64, settings: WarpSettings) -> Result<(), Invalid> {
        let index = self
            .events
            .iter()
            .position(|candidate| candidate.id == event && candidate.active)
            .ok_or(Invalid("active audio event no longer exists"))?;
        let source_id = self.events[index].source;
        let source_index = self
            .sources
            .iter()
            .position(|root| root.id == source_id)
            .ok_or(Invalid("audio event has no retained warp source"))?;
        let original = &self.sources[source_index];
        let mut edit = original.clip.audio_event_root();
        let current = edit
            .domain
            .warp
            .as_ref()
            .filter(|_| edit.domain.warp_enabled)
            .ok_or(Invalid(
                "enable warp on the retained source before editing markers",
            ))?;
        if original.clip.has_retained_audio_roots()
            || current.mode != settings.mode
            || current.markers.first() != settings.markers.first()
            || current.markers.last() != settings.markers.last()
        {
            return Err(Invalid(
                "grouped marker edits must keep the retained source bounds and mode",
            ));
        }
        if current == &settings {
            return Ok(());
        }
        Arc::make_mut(&mut edit.domain).warp = Some(settings);
        let edit = Some(Arc::new(edit));
        if self
            .events
            .iter()
            .enumerate()
            .any(|(other, event)| other != index && event.source == source_id)
        {
            if self.sources.len() >= MAX_AUDIO_EVENTS {
                return Err(Invalid("grouped audio source limit reached"));
            }
            let mut fork = original.clone();
            fork.id = self
                .sources
                .iter()
                .map(|root| root.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(Invalid("retained audio source identity exhausted"))?;
            fork.clip.edit = edit;
            self.events[index].source = fork.id;
            self.sources.push(fork);
        } else {
            self.sources[source_index].clip.edit = edit;
        }
        Ok(())
    }

    fn set_split_warp(
        &mut self,
        event: u64,
        settings: WarpSettings,
        root: &AudioClipEdit,
    ) -> Result<(), Invalid> {
        let index = self
            .events
            .iter()
            .position(|candidate| candidate.id == event && candidate.active)
            .ok_or(Invalid("active audio event no longer exists"))?;
        let base = root
            .domain
            .warp
            .as_ref()
            .filter(|_| root.domain.warp_enabled)
            .ok_or(Invalid("enable warp before editing split event markers"))?;
        let source_id = self.events[index].source;
        let source_index = self
            .warp_sources
            .iter()
            .position(|source| source.id == source_id);
        let current = source_index
            .map(|index| self.warp_sources[index].markers.as_ref())
            .unwrap_or(&base.markers);
        if base.mode != settings.mode
            || current.first() != settings.markers.first()
            || current.last() != settings.markers.last()
        {
            return Err(Invalid(
                "event marker edits must keep the source bounds and mode",
            ));
        }
        if current == settings.markers.as_slice() {
            return Ok(());
        }
        let source_end = root
            .domain
            .source_offset
            .checked_add(root.domain.source_length)
            .ok_or(Invalid("audio domain source end overflow"))?;
        crate::audio_edit::validate_warp_markers(
            &settings.markers,
            root.domain.source_offset,
            source_end,
        )?;
        if let Some(source_index) = source_index
            && !self
                .events
                .iter()
                .enumerate()
                .any(|(other, event)| other != index && event.source == source_id)
        {
            self.warp_sources[source_index].markers = settings.markers.into();
        } else {
            if self.warp_sources.len() >= MAX_AUDIO_EVENTS {
                return Err(Invalid("event warp source limit reached"));
            }
            let id = self
                .warp_sources
                .iter()
                .map(|source| source.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(Invalid("event warp source identity exhausted"))?;
            self.warp_sources.push(AudioEventWarpSource {
                id,
                markers: settings.markers.into(),
            });
            self.events[index].source = id;
        }
        Ok(())
    }

    fn edited_inner(&self, operation: AudioEventEdit, length: u64) -> Result<Self, Invalid> {
        self.validate_geometry(length)?;
        let mut next = self.clone();
        match operation {
            AudioEventEdit::Place { .. } | AudioEventEdit::PlaceRange { .. } => {
                return Err(Invalid(
                    "free audio event placement edits the clip's whole timeline",
                ));
            }
            AudioEventEdit::SetRetainedPathActive {
                sources,
                events,
                active,
            } => {
                if sources.is_empty()
                    || sources.len() > crate::retained_audio::MAX_RETAINED_AUDIO_DEPTH
                {
                    return Err(Invalid("invalid retained audio source path"));
                }
                let root = next
                    .sources
                    .iter_mut()
                    .find(|root| root.id == sources[0])
                    .ok_or(Invalid("retained audio source no longer exists"))?;
                let mut edit = root.clip.audio_event_root();
                let original = root.clip.editable_audio_events()?;
                let operation = if sources.len() == 1 {
                    AudioEventEdit::SetActive { events, active }
                } else {
                    AudioEventEdit::SetRetainedPathActive {
                        sources: sources[1..].to_vec(),
                        events,
                        active,
                    }
                };
                let changed = original.edited(operation, edit.domain.clock.length())?;
                if changed != original {
                    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(changed));
                    root.clip.edit = Some(Arc::new(edit));
                }
            }
            AudioEventEdit::SetRetainedActive {
                source,
                events,
                active,
            } => {
                next.edit_retained(source, AudioEventEdit::SetActive { events, active })?;
            }
            AudioEventEdit::SetRetainedGain {
                sources,
                event,
                gain,
            } => {
                if sources.len() > crate::MAX_RETAINED_AUDIO_DEPTH {
                    return Err(Invalid("invalid retained audio source path"));
                }
                let Some((&source, rest)) = sources.split_first() else {
                    return Err(Invalid("invalid retained audio source path"));
                };
                let operation = if rest.is_empty() {
                    AudioEventEdit::SetGain { event, gain }
                } else {
                    AudioEventEdit::SetRetainedGain {
                        sources: rest.to_vec(),
                        event,
                        gain,
                    }
                };
                next.edit_retained(source, operation)?;
            }
            AudioEventEdit::SetRetainedWarp { event, settings } => {
                next.set_retained_warp(event, settings)?;
            }
            AudioEventEdit::SetGain { event, gain } => {
                next.events
                    .iter_mut()
                    .find(|candidate| candidate.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?
                    .gain = gain;
            }
            AudioEventEdit::SetActive { mut events, active } => {
                if events.is_empty() || events.len() > MAX_AUDIO_EVENTS {
                    return Err(Invalid("invalid audio event activation batch"));
                }
                events.sort_unstable();
                if events.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err(Invalid("duplicate audio event identity"));
                }
                let mut matched = 0;
                for event in &mut next.events {
                    if events.binary_search(&event.id).is_ok() {
                        event.active = active;
                        matched += 1;
                    }
                }
                if matched != events.len() {
                    return Err(Invalid("audio event no longer exists"));
                }
            }
            AudioEventEdit::SetFade { event, fade } => {
                let index = next
                    .events
                    .iter()
                    .position(|e| e.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let target = next
                    .events
                    .get(index)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let source_length = next
                    .source_length(target.source, length)
                    .ok_or(Invalid("audio event source is missing"))?;
                if let Some(fade) = fade {
                    fade.validate(source_length, target.source_start, target.length())?;
                }
                next.events
                    .get_mut(index)
                    .ok_or(Invalid("audio event no longer exists"))?
                    .fade = fade.filter(|fade| !fade.shape().is_neutral());
            }
            AudioEventEdit::Split { at } => {
                if let Some(index) = next.events.iter().position(|e| e.start < at && at < e.end) {
                    let event = next.events[index];
                    let right = AudioEvent {
                        id: next.id()?,
                        start: at,
                        source_start: event.source_start + (at - event.start),
                        ..event
                    };
                    next.events[index].end = at;
                    next.events.insert(index + 1, right);
                }
            }
            AudioEventEdit::SplitRange { start, end } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event split range"));
                }
                next = next.edited_inner(AudioEventEdit::Split { at: start }, length)?;
                next = next.edited_inner(AudioEventEdit::Split { at: end }, length)?;
            }
            AudioEventEdit::Insert {
                at,
                length: span,
                events,
            } => {
                if at > length || span == 0 {
                    return Err(Invalid("invalid audio event insert range"));
                }
                next = next.edited_inner(AudioEventEdit::Split { at }, length)?;
                for event in &mut next.events {
                    if event.start >= at {
                        event.end = event
                            .end
                            .checked_add(span)
                            .filter(|end| *end <= length)
                            .ok_or(Invalid(
                                "audio event insert needs free time inside the clip",
                            ))?;
                        event.start = event
                            .start
                            .checked_add(span)
                            .ok_or(Invalid("audio event insert position overflow"))?;
                    }
                }
                next = next.edited_inner(
                    AudioEventEdit::Paste {
                        at,
                        length: span,
                        events,
                    },
                    length,
                )?;
            }
            AudioEventEdit::Duplicate { event, start } => {
                let source = next
                    .events
                    .iter()
                    .find(|item| item.id == event)
                    .copied()
                    .ok_or(Invalid("audio event no longer exists"))?;
                let start = u64::try_from(start)
                    .map_err(|_| Invalid("audio event duplicate precedes its container"))?;
                let end = start
                    .checked_add(source.length())
                    .filter(|end| *end <= length)
                    .ok_or(Invalid("audio event duplicate exceeds its container"))?;
                if start < source.end && source.start < end {
                    return Err(Invalid("audio event duplicate overlaps its source"));
                }
                next.carve(start, end)?;
                let id = next.id()?;
                next.events.push(AudioEvent {
                    id,
                    start,
                    end,
                    ..source
                });
            }
            AudioEventEdit::Move { event, start } => {
                let index = next
                    .events
                    .iter()
                    .position(|e| e.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let mut moved = next.events.remove(index);
                let end = start
                    .checked_add(moved.length())
                    .filter(|end| *end <= length)
                    .ok_or(Invalid("audio event move exceeds its container"))?;
                if start == moved.start {
                    return Ok(self.clone());
                }
                next.carve(start, end)?;
                moved.start = start;
                moved.end = end;
                next.events.push(moved);
            }
            AudioEventEdit::Trim { event, start, end } => {
                let index = next
                    .events
                    .iter()
                    .position(|e| e.id == event)
                    .ok_or(Invalid("audio event no longer exists"))?;
                let original = next.events[index];
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event trim"));
                }
                let source_start = if start < original.start {
                    original.source_start.checked_sub(original.start - start)
                } else {
                    original.source_start.checked_add(start - original.start)
                }
                .ok_or(Invalid("audio event trim exceeds original source"))?;
                next.events.remove(index);
                next.carve(start, end)?;
                next.events.push(AudioEvent {
                    start,
                    end,
                    source_start,
                    fade: original
                        .fade
                        .map(|fade| fade.reanchor(source_start, end - start))
                        .transpose()?,
                    ..original
                });
            }
            AudioEventEdit::MoveRange {
                start,
                end,
                destination,
            } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event time range"));
                }
                let span = end - start;
                if destination == start {
                    return Ok(self.clone());
                }
                destination
                    .checked_add(span)
                    .filter(|end| *end <= length)
                    .ok_or(Invalid("audio event range move exceeds its container"))?;
                let copied = next.copy_range(start, end);
                next = next.edited_inner(AudioEventEdit::Delete { start, end }, length)?;
                next = next.edited_inner(
                    AudioEventEdit::Paste {
                        at: destination,
                        length: span,
                        events: copied,
                    },
                    length,
                )?;
            }
            AudioEventEdit::Delete { start, end } => {
                if start >= end || end > length {
                    return Err(Invalid("invalid audio event time range"));
                }
                next.carve(start, end)?;
            }
            AudioEventEdit::Paste {
                at,
                length: span,
                events,
            } => {
                let end = at
                    .checked_add(span)
                    .filter(|end| *end <= length && span > 0)
                    .ok_or(Invalid("audio event paste exceeds its container"))?;
                if events.len() > MAX_AUDIO_EVENTS {
                    return Err(Invalid("audio event clipboard limit reached"));
                }
                // Validate untrusted clipboard geometry before carving anything.
                let mut prior = 0;
                for event in &events {
                    if event.start < prior
                        || event.start >= event.end
                        || event.end > span
                        || event
                            .source_start
                            .checked_add(event.length())
                            .is_none_or(|end| {
                                end > self.source_length(event.source, length).unwrap_or(0)
                            })
                    {
                        return Err(Invalid("invalid audio event clipboard"));
                    }
                    prior = event.end;
                }
                next.carve(at, end)?;
                for event in events {
                    let id = next.id()?;
                    next.events.push(AudioEvent {
                        id,
                        start: at + event.start,
                        end: at + event.end,
                        ..event
                    });
                }
            }
        }
        next.events.sort_by_key(|e| e.start);
        next.validate_geometry(length)?;
        Ok(next)
    }
    pub(crate) fn place_range(
        self,
        start: u64,
        end: u64,
        destination: u64,
        length: u64,
    ) -> Result<Self, Invalid> {
        // Split before removing the selection so complete events and
        // untouched left fragments keep their original identities.
        let mut next = self.edited_inner(AudioEventEdit::SplitRange { start, end }, length)?;
        let first = next.events.partition_point(|event| event.start < start);
        let last = next.events.partition_point(|event| event.start < end);
        let moved: Vec<_> = next.events.drain(first..last).collect();
        next.carve(destination, destination + (end - start))?;
        next.events
            .extend(moved.into_iter().map(|event| AudioEvent {
                start: destination + (event.start - start),
                end: destination + (event.end - start),
                ..event
            }));
        next.events.sort_by_key(|event| event.start);
        next.validate(length)?;
        Ok(next)
    }
    pub fn copy_range(&self, start: u64, end: u64) -> Vec<AudioEvent> {
        self.events
            .iter()
            .filter_map(|event| {
                let a = start.max(event.start);
                let b = end.min(event.end);
                (a < b).then(|| AudioEvent {
                    start: a - start,
                    end: b - start,
                    source_start: event.source_start + (a - event.start),
                    ..*event
                })
            })
            .collect()
    }
}

impl Clip {
    pub fn audio_event_root(&self) -> AudioClipEdit {
        self.audio_edit()
            .cloned()
            .unwrap_or_else(|| AudioClipEdit::capture(self))
    }
    /// Split only the editable view boundaries, retaining hidden source intervals.
    /// The returned local IDs are materialized by the next actual inner edit.
    pub fn editable_audio_events(&self) -> Result<AudioEvents, Invalid> {
        let root = self.audio_event_root();
        if root.view_start == 0 && root.view_end == root.domain.clock.length() {
            return Ok(self.audio_events());
        }
        self.audio_events().edited_inner(
            AudioEventEdit::SplitRange {
                start: root.view_start,
                end: root.view_end,
            },
            root.domain.clock.length(),
        )
    }
    /// Compute the event sequence used by both a local preview and the
    /// canonical clip edit. Media and placement admission remain with Project.
    pub fn preview_audio_event_edit(
        &self,
        operation: AudioEventEdit,
    ) -> Result<AudioEvents, Invalid> {
        let root = self.audio_event_root();
        self.editable_audio_events()?
            .edited_in_root(operation, &root)
    }
    /// The implicit initial event keeps legacy clips allocation-free until edited.
    pub fn audio_events(&self) -> AudioEvents {
        self.audio_edit()
            .and_then(|edit| edit.domain.events.as_deref())
            .cloned()
            .unwrap_or_else(|| {
                AudioEvents::identity(
                    self.audio_edit()
                        .map_or(self.duration(), |e| e.domain.clock.length()),
                )
            })
    }
}

pub(crate) fn apply(
    project: &mut Project,
    clip: ClipId,
    operation: AudioEventEdit,
) -> Result<(), Invalid> {
    let original = project.clip(clip).ok_or_else(|| {
        if project.session_audio_clip(clip).is_some() {
            Invalid("audio event editing is unavailable for console source windows")
        } else {
            Invalid("clip no longer exists")
        }
    })?;
    if matches!(
        operation,
        AudioEventEdit::Place { .. }
            | AudioEventEdit::PlaceRange { .. }
            | AudioEventEdit::Duplicate { .. }
    ) {
        let candidate = crate::audio_event_timeline::prepare(project, clip, &operation)?;
        *project.clip_mut(clip)? = candidate;
        return Ok(());
    }
    let mut edit = original
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(original));
    let events = original.editable_audio_events()?;
    let retained_source = match &operation {
        AudioEventEdit::SetRetainedActive { source, .. } => Some(*source),
        AudioEventEdit::SetRetainedPathActive { sources, .. }
        | AudioEventEdit::SetRetainedGain { sources, .. } => sources.first().copied(),
        _ => None,
    };
    let warp_event = match &operation {
        AudioEventEdit::SetRetainedWarp { event, .. } => Some(*event),
        _ => None,
    };
    let next = events.edited_in_root(operation, &edit)?;
    if next == events {
        return Ok(());
    }
    let changed_source = retained_source.or_else(|| {
        warp_event.and_then(|id| {
            next.events
                .iter()
                .find(|event| event.id == id)
                .map(|event| event.source)
        })
    });
    // Validate retained roots against their original media and processing clock
    // before installing any changed source into the candidate clip.
    if let Some(root) = changed_source.and_then(|id| next.sources.iter().find(|root| root.id == id))
    {
        project.restore_retained_audio_root(
            root.id,
            root.clip.retained_snapshot(),
            Arc::clone(&root.tempo),
        )?;
    }
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(next));
    let mut candidate = original.clone();
    candidate.edit = Some(Arc::new(edit));
    crate::audio_edit::validate_clip(project, &candidate)?;
    *project.clip_mut(clip)? = candidate;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_is_local_atomic_and_survives_split_trim_copy_and_inactivation() {
        let initial = AudioEvents::identity(100);
        let gain = crate::ClipGain::new(-6_000).unwrap();
        assert!(
            initial
                .edited(AudioEventEdit::SetGain { event: 99, gain }, 100)
                .is_err()
        );
        assert_eq!(initial.events[0].gain, crate::ClipGain::UNITY);
        let gained = initial
            .edited(AudioEventEdit::SetGain { event: 1, gain }, 100)
            .unwrap();
        assert_eq!(gained.events[0].gain, gain);
        assert_eq!(
            gained
                .edited(AudioEventEdit::SetGain { event: 1, gain }, 100)
                .unwrap(),
            gained
        );
        let split = gained
            .edited(AudioEventEdit::Split { at: 40 }, 100)
            .unwrap();
        assert!(split.events.iter().all(|event| event.gain == gain));
        let other = crate::ClipGain::new(3_000).unwrap();
        let changed = split
            .edited(
                AudioEventEdit::SetGain {
                    event: 2,
                    gain: other,
                },
                100,
            )
            .unwrap();
        assert_eq!(changed.events[0], split.events[0]);
        let trimmed = changed
            .edited(
                AudioEventEdit::Trim {
                    event: 2,
                    start: 50,
                    end: 90,
                },
                100,
            )
            .unwrap();
        let inactive = trimmed
            .edited(
                AudioEventEdit::SetActive {
                    events: vec![2],
                    active: false,
                },
                100,
            )
            .unwrap();
        assert_eq!(inactive.events[1].gain, other);
        let copied = inactive.copy_range(20, 90);
        let pasted = initial
            .edited(
                AudioEventEdit::Paste {
                    at: 0,
                    length: 70,
                    events: copied,
                },
                100,
            )
            .unwrap();
        assert_eq!(pasted.events[0].gain, gain);
        assert!(
            pasted
                .events
                .iter()
                .any(|event| !event.active && event.gain == other)
        );
    }

    #[test]
    fn activation_is_atomic_and_survives_split_and_copy() {
        let initial = AudioEvents::identity(100);
        assert!(
            initial
                .edited(
                    AudioEventEdit::SetActive {
                        events: vec![1, 2],
                        active: false
                    },
                    100
                )
                .is_err()
        );
        assert!(
            initial
                .edited(
                    AudioEventEdit::SetActive {
                        events: vec![1, 1],
                        active: false
                    },
                    100
                )
                .is_err()
        );
        let inactive = initial
            .edited(
                AudioEventEdit::SetActive {
                    events: vec![1],
                    active: false,
                },
                100,
            )
            .unwrap();
        let split = inactive
            .edited(AudioEventEdit::Split { at: 40 }, 100)
            .unwrap();
        assert!(split.events.iter().all(|event| !event.active));
        let copied = split.copy_range(10, 50);
        assert_eq!(copied.len(), 2);
        assert!(copied.iter().all(|event| !event.active));
        let pasted = split
            .edited(
                AudioEventEdit::Paste {
                    at: 50,
                    length: 40,
                    events: copied,
                },
                100,
            )
            .unwrap();
        assert!(pasted.events.iter().all(|event| !event.active));
    }
    #[test]
    fn split_move_overwrite_trim_and_range_clipboard_preserve_source_coordinates() {
        let identity = AudioEvents::identity(100);
        let split = identity
            .edited(AudioEventEdit::Split { at: 30 }, 100)
            .unwrap();
        assert_eq!(
            split
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 30, 0), (30, 100, 30)]
        );
        let moved = split
            .edited(
                AudioEventEdit::Move {
                    event: 1,
                    start: 50,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            moved
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(30, 50, 30), (50, 80, 0), (80, 100, 80)]
        );
        let trim = moved
            .edited(
                AudioEventEdit::Trim {
                    event: 1,
                    start: 55,
                    end: 75,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            trim.events.iter().find(|e| e.id == 1).unwrap().source_start,
            5
        );
        let copied = trim.copy_range(40, 85);
        let pasted = trim
            .edited(
                AudioEventEdit::Paste {
                    at: 0,
                    length: 45,
                    events: copied,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            pasted
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![
                (0, 10, 40),
                (15, 35, 5),
                (40, 45, 80),
                (45, 50, 45),
                (55, 75, 5),
                (80, 100, 80)
            ]
        );
        let cut = pasted
            .edited(AudioEventEdit::Delete { start: 10, end: 80 }, 100)
            .unwrap();
        assert_eq!(
            cut.events
                .iter()
                .map(|e| (e.start, e.end))
                .collect::<Vec<_>>(),
            vec![(0, 10), (80, 100)]
        );
        assert!(
            cut.edited(
                AudioEventEdit::Move {
                    event: 1,
                    start: u64::MAX
                },
                100
            )
            .is_err()
        );
        assert_eq!(identity.events[0].end, 100);
    }
    #[test]
    fn console_source_windows_refuse_inner_edits_without_changing_project_or_history() {
        use crate::{Command, Created, Editor};
        let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "original.wav".into(),
                sample_rate: 48000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let before_copy = editor.project().clone();
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let before_cell = editor.project().clone();
        let Created::SessionClip(console) = editor
            .execute(Command::CopyArrangementAudioClipToSession {
                source: clip,
                scene,
            })
            .unwrap()
        else {
            panic!("cell")
        };
        let before = editor.project().clone();
        assert_eq!(
            editor.execute(Command::EditAudioEvents {
                clip: console,
                operation: AudioEventEdit::Delete { start: 10, end: 20 }
            }),
            Err(Invalid(
                "audio event editing is unavailable for console source windows"
            ))
        );
        assert_eq!(*editor.project(), before);
        assert!(editor.undo());
        let mut expected = before_cell.snapshot();
        // Global IDs remain monotonic across undo; musical state is restored.
        expected.next_id = editor.project().snapshot().next_id;
        assert_eq!(
            editor.project().snapshot(),
            expected,
            "refusal adds no history edge"
        );
        assert!(editor.undo());
        let mut expected = before_copy.snapshot();
        expected.next_id = editor.project().snapshot().next_id;
        assert_eq!(editor.project().snapshot(), expected);
    }
    #[test]
    fn grouped_warp_forks_only_the_selected_retained_audio_source() {
        use crate::{BeatTime, ClipFade, Command, Created, Editor};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { track, asset, clip } = editor
            .execute(Command::ImportAudioTrack {
                name: "warp source".into(),
                path: "warp.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("audio source");
        };
        let mut warp = editor
            .project()
            .audio_warp_settings_for_enable(clip)
            .unwrap();
        assert!(warp.insert_marker(24_000).unwrap());
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp.clone()),
            })
            .unwrap();
        let Created::Clip(other) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 97_000,
                duration: 1_000,
                source_offset: 0,
                source_length: 1_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("adjacent audio");
        };
        let Created::Clip(group) = editor
            .execute(Command::GroupClips(vec![clip, other]))
            .unwrap()
        else {
            panic!("group");
        };
        let command = |operation| Command::EditAudioEvents {
            clip: group,
            operation,
        };
        editor
            .execute(command(AudioEventEdit::Split { at: 48_000 }))
            .unwrap();
        editor
            .execute(command(AudioEventEdit::SetFade {
                event: 1,
                fade: Some(
                    AudioEventFade::new(0, ClipFade::new(48_000, 0, 64, 32).unwrap()).unwrap(),
                ),
            }))
            .unwrap();
        let before = editor.project().clone();
        let events = before.clip(group).unwrap().audio_events();
        let [selected, sibling, _] = events.events.as_slice() else {
            panic!("split retained events");
        };
        assert_eq!(selected.source, sibling.source);
        assert_ne!(selected.source, 0);

        let mut shifted = warp.clone();
        let beat = shifted.markers[1].beat.raw();
        assert!(
            shifted
                .move_marker(24_000, BeatTime::from_raw(beat + BeatTime::ONE.raw() / 8))
                .unwrap()
        );
        editor
            .execute(command(AudioEventEdit::SetRetainedWarp {
                event: selected.id,
                settings: shifted.clone(),
            }))
            .unwrap();
        let after = editor.project().clone();
        let changed = after.clip(group).unwrap().audio_events();
        assert_eq!(changed.events[1..], events.events[1..]);
        assert_eq!(changed.sources.len(), events.sources.len() + 1);
        assert_eq!(changed.sources[..events.sources.len()], events.sources);
        let edited = changed.events[0];
        assert_ne!(edited.source, selected.source);
        assert_eq!(
            AudioEvent {
                source: selected.source,
                ..edited
            },
            *selected
        );
        let fork = changed
            .sources
            .iter()
            .find(|root| root.id == edited.source)
            .unwrap();
        assert_eq!(
            fork.clip.audio_edit().unwrap().domain.warp.as_ref(),
            Some(&shifted)
        );
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);

        let mut invalid = shifted.clone();
        invalid.markers[1].beat = invalid.markers[0].beat;
        assert!(
            editor
                .execute(command(AudioEventEdit::SetRetainedWarp {
                    event: selected.id,
                    settings: invalid,
                }))
                .is_err()
        );
        assert_eq!(editor.project(), &after);
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }

    #[test]
    fn split_marker_move_keeps_both_sources_in_the_live_beat_clock() {
        use crate::{AudioDomainClock, BeatTime, ClipFade, Command, Created, Editor, Tempo};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "split warp".into(),
                path: "split-warp.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("audio source");
        };
        let mut warp = editor
            .project()
            .audio_warp_settings_for_enable(clip)
            .unwrap();
        assert!(warp.insert_marker(24_000).unwrap());
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp.clone()),
            })
            .unwrap();
        let command = |operation| Command::EditAudioEvents { clip, operation };
        let root = editor.project().clip(clip).unwrap().audio_event_root();
        let clock = root.domain.clock;
        assert!(matches!(clock, AudioDomainClock::Beats(_)));
        let split = clock.length() / 2;
        editor
            .execute(command(AudioEventEdit::Split { at: split }))
            .unwrap();
        let fade = AudioEventFade::new(0, ClipFade::new(split, 0, split / 4, 0).unwrap()).unwrap();
        editor
            .execute(command(AudioEventEdit::SetFade {
                event: 1,
                fade: Some(fade),
            }))
            .unwrap();
        let before = editor.project().clone();
        let events = before.clip(clip).unwrap().audio_events();
        let [selected, sibling] = events.events.as_slice() else {
            panic!("one split and one sibling");
        };
        assert_eq!((selected.source, sibling.source), (0, 0));

        let mut shifted = warp.clone();
        let beat = warp.markers[1].beat.raw() + BeatTime::ONE.raw() / 8;
        assert!(
            shifted
                .move_marker(24_000, BeatTime::from_raw(beat))
                .unwrap()
        );
        editor
            .execute(command(AudioEventEdit::SetRetainedWarp {
                event: selected.id,
                settings: shifted,
            }))
            .expect("a split event can edit a marker without reauthoring its sibling");
        let after = editor.project().clone();
        let clip_after = after.clip(clip).unwrap();
        let root = clip_after.audio_event_root();
        let changed = clip_after.audio_events();
        let edited = changed.events[0];
        assert_ne!(edited.source, selected.source);
        assert_eq!(
            AudioEvent {
                source: selected.source,
                ..edited
            },
            *selected
        );
        assert_eq!(changed.events[1..], [*sibling]);
        assert_eq!(root.domain.warp.as_ref(), Some(&warp));
        for source in [edited.source, sibling.source] {
            assert_eq!(root.event_source_clock(source).unwrap(), clock);
        }
        let old_end = root
            .event_source_frame_at(edited.source, clock.length(), after.tempo_map())
            .unwrap();
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ONE,
                tempo: Tempo::new(180_000_000).unwrap(),
            })
            .unwrap();
        let project = editor.project();
        let actual = project.clip(clip).unwrap().audio_event_root();
        let edited_end = actual
            .event_source_frame_at(edited.source, clock.length(), project.tempo_map())
            .unwrap();
        let sibling_end = actual
            .event_source_frame_at(sibling.source, clock.length(), project.tempo_map())
            .unwrap();
        assert_ne!(edited_end, old_end);
        assert_eq!(edited_end, sibling_end);
    }

    #[test]
    fn canonical_events_undo_and_outer_splits_keep_the_full_retained_sequence() {
        use crate::{Command, Created, Editor};
        let mut editor = Editor::new(Project::new(48000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "test".into(),
                path: "test.wav".into(),
                sample_rate: 48000,
                frames: 100,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: 30 },
            })
            .unwrap();
        let events = editor.project().clip(clip).unwrap().audio_events();
        assert_eq!(events.events.len(), 2);
        assert!(editor.undo());
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
        assert!(editor.redo());
        let Created::Clip(right) = editor
            .execute(Command::SplitClip { clip, frame: 50 })
            .unwrap()
        else {
            panic!("split")
        };
        assert_eq!(editor.project().clip(clip).unwrap().audio_events(), events);
        assert_eq!(editor.project().clip(right).unwrap().audio_events(), events);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    #[test]
    fn range_move_and_insert_preserve_holes_and_source_coordinates_with_finite_capacity() {
        let initial = AudioEvents::identity(100);
        let split = initial
            .edited(AudioEventEdit::SplitRange { start: 20, end: 40 }, 100)
            .unwrap();
        assert_eq!(
            split
                .events
                .iter()
                .map(|e| (e.start, e.end))
                .collect::<Vec<_>>(),
            vec![(0, 20), (20, 40), (40, 100)]
        );
        let cut = split
            .edited(AudioEventEdit::Delete { start: 40, end: 60 }, 100)
            .unwrap();
        let moved = cut
            .edited(
                AudioEventEdit::MoveRange {
                    start: 20,
                    end: 60,
                    destination: 60,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            moved
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 20, 0), (60, 80, 20)]
        );
        let clipboard = moved.copy_range(0, 20);
        let inserted = moved
            .edited(
                AudioEventEdit::Insert {
                    at: 0,
                    length: 20,
                    events: clipboard,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            inserted
                .events
                .iter()
                .map(|e| (e.start, e.end, e.source_start))
                .collect::<Vec<_>>(),
            vec![(0, 20, 0), (20, 40, 0), (80, 100, 20)]
        );
        assert!(
            inserted
                .edited(
                    AudioEventEdit::Insert {
                        at: 0,
                        length: 1,
                        events: vec![]
                    },
                    100
                )
                .is_err()
        );
        let moved_id = moved.events.iter().find(|e| e.start == 60).unwrap().id;
        let extended = moved
            .edited(
                AudioEventEdit::Trim {
                    event: moved_id,
                    start: 55,
                    end: 85,
                },
                100,
            )
            .unwrap();
        assert_eq!(
            extended
                .events
                .iter()
                .find(|e| e.id == moved_id)
                .unwrap()
                .source_start,
            15
        );
    }
    #[test]
    fn splitting_at_capacity_refuses_without_mutating_the_sequence() {
        let sequence = AudioEvents {
            warp_sources: Vec::new(),
            sources: Vec::new(),
            next_id: MAX_AUDIO_EVENTS as u64 + 1,
            events: (0..MAX_AUDIO_EVENTS as u64)
                .map(|i| AudioEvent {
                    active: true,
                    gain: crate::ClipGain::UNITY,
                    source: 0,
                    id: i + 1,
                    start: i * 4,
                    end: i * 4 + 2,
                    source_start: i * 4,
                    fade: None,
                })
                .collect(),
        };
        sequence.validate(MAX_AUDIO_EVENTS as u64 * 4).unwrap();
        assert!(
            sequence
                .edited(AudioEventEdit::Split { at: 1 }, MAX_AUDIO_EVENTS as u64 * 4)
                .is_err()
        );
        assert_eq!(sequence.events.len(), MAX_AUDIO_EVENTS);
    }
}

#[cfg(test)]
mod cropped_capacity_regression {
    use super::*;
    use crate::{Command, Created, Editor};
    #[test]
    fn delete_from_cropped_full_sequence_checks_final_capacity_and_undo() {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "capacity".into(),
                path: "capacity.wav".into(),
                sample_rate: 48000,
                frames: 4096,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        let mut project = editor.project().clone();
        let mut root = project.clip(clip).unwrap().audio_event_root();
        Arc::make_mut(&mut root.domain).events = Some(Arc::new(AudioEvents {
            warp_sources: Vec::new(),
            sources: Vec::new(),
            next_id: 1025,
            events: (0..1024)
                .map(|i| AudioEvent {
                    active: true,
                    gain: crate::ClipGain::UNITY,
                    source: 0,
                    id: i + 1,
                    start: i * 4,
                    end: i * 4 + 4,
                    source_start: i * 4,
                    fade: None,
                })
                .collect(),
        }));
        project.clip_mut(clip).unwrap().edit = Some(Arc::new(root));
        let mut editor = Editor::new(project, 8).unwrap();
        let Created::Clip(right) = editor
            .execute(Command::SplitClip { clip, frame: 2 })
            .unwrap()
        else {
            panic!("split")
        };
        let before = editor.project().clone();
        assert!(
            editor
                .project()
                .clip(right)
                .unwrap()
                .editable_audio_events()
                .is_ok(),
            "virtual crop boundaries must remain editable at capacity"
        );
        assert!(
            editor
                .execute(Command::EditAudioEvents {
                    clip: right,
                    operation: AudioEventEdit::Split { at: 3 },
                })
                .is_err(),
            "an increasing edit must still enforce the final limit"
        );
        assert_eq!(*editor.project(), before);
        editor
            .execute(Command::EditAudioEvents {
                clip: right,
                operation: AudioEventEdit::Delete {
                    start: 2,
                    end: 4096,
                },
            })
            .unwrap();
        let sequence = editor.project().clip(right).unwrap().audio_events();
        assert_eq!(sequence.events.len(), 1);
        assert_eq!((sequence.events[0].start, sequence.events[0].end), (0, 2));
        sequence.validate(4096).unwrap();
        assert!(editor.undo());
        assert_eq!(*editor.project(), before);
    }
}
