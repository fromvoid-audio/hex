//! Two virtual split overlays over borrowed canonical events. Hidden intervals
//! remain in the stream; only actual interior hits consume local identities.
use super::*;
use crate::audio_events::{EventSplit, split_event, validate_event_geometry};
use crate::{AudioEvent, AudioEvents, MAX_AUDIO_EVENTS};

#[derive(Clone, Copy)]
enum Source<'a> {
    Stored(&'a AudioEvents),
    Implicit(AudioEvent),
}
impl Source<'_> {
    fn len(self) -> usize {
        match self {
            Self::Stored(e) => e.events.len(),
            Self::Implicit(_) => 1,
        }
    }
    fn next_id(self) -> u64 {
        match self {
            Self::Stored(e) => e.next_id,
            Self::Implicit(_) => 2,
        }
    }
    fn get(self, index: usize) -> Option<AudioEvent> {
        match self {
            Self::Stored(e) => e.events.get(index).copied(),
            Self::Implicit(e) => (index == 0).then_some(e),
        }
    }
}
#[derive(Clone, Copy)]
struct Overlay {
    source_index: usize,
    split: EventSplit,
}

/// Exact virtual editable sequence. The source is borrowed, the two overlays
/// and emitted events are Copy, and no AudioEvents/list owner is constructed.
pub struct EditableAudioEventsRead<'a> {
    source: Source<'a>,
    overlays: [Option<Overlay>; 2],
    next_id: u64,
    len: usize,
}
impl<'a> EditableAudioEventsRead<'a> {
    pub(super) fn new(clip: &'a Clip, facts: AudioClipRootFacts) -> Result<Self, Invalid> {
        let length = facts.domain.clock.length();
        let source = clip
            .audio_edit()
            .and_then(|e| e.domain.events.as_deref())
            .map_or(
                Source::Implicit(AudioEvent {
                    id: 1,
                    start: 0,
                    end: length,
                    source_start: 0,
                }),
                Source::Stored,
            );
        let mut read = Self {
            source,
            overlays: [None; 2],
            next_id: source.next_id(),
            len: source.len(),
        };
        // Preserve the owning helper's early return: no geometry/cap validation
        // is added when the view already covers the complete domain.
        if facts.view_start == 0 && facts.view_end == length {
            return Ok(read);
        }
        if source.len() > MAX_AUDIO_EVENTS {
            return Err(Invalid("audio event read exceeds canonical event limit"));
        }
        // Same geometry-before-range ordering as edited_inner(SplitRange).
        // Fixed existing scratch: 1024 (u64,usize) pairs, 16 KiB on 64-bit hosts.
        if read.next_id == 0 {
            return Err(Invalid("audio event identity is invalid"));
        }
        let mut ids = [(0_u64, 0_usize); MAX_AUDIO_EVENTS];
        for (index, entry) in ids.iter_mut().take(source.len()).enumerate() {
            let event = source
                .get(index)
                .ok_or(Invalid("audio event source index is invalid"))?;
            *entry = (event.id, index);
        }
        let ids = &mut ids[..source.len()];
        ids.sort_unstable();
        let duplicate = ids
            .windows(2)
            .filter(|p| p[0].0 == p[1].0)
            .map(|p| p[1].1)
            .min();
        validate_event_geometry(
            read.next_id,
            length,
            read.indexed().map(|(_, e)| Ok(e)),
            |index, _| Some(index) != duplicate,
        )?;
        if facts.view_start >= facts.view_end || facts.view_end > length {
            return Err(Invalid("invalid audio event split range"));
        }
        read.split_at(0, facts.view_start)?;
        // Initial identities were unique; the shared splitter only introduces
        // the checked next identity. Recheck intermediate geometry in the same
        // position as the owning second Split, without a second ID scratch.
        validate_event_geometry(
            read.next_id,
            length,
            read.indexed().map(|(_, e)| Ok(e)),
            |_, _| true,
        )?;
        read.split_at(1, facts.view_end)?;
        Ok(read)
    }
    fn split_at(&mut self, slot: usize, at: u64) -> Result<(), Invalid> {
        let hit = self.indexed().find(|(_, e)| e.start < at && at < e.end);
        if let Some((source_index, event)) = hit {
            if let Some(split) = split_event(event, at, self.next_id)? {
                self.next_id = split.next_id;
                self.overlays[slot] = Some(Overlay {
                    source_index,
                    split,
                });
                self.len += 1;
            }
        }
        Ok(())
    }
    pub fn next_id(&self) -> u64 {
        self.next_id
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl ExactSizeIterator<Item = AudioEvent> + '_ {
        self.indexed().map(|(_, event)| event)
    }
    fn indexed(&self) -> Events<'_> {
        Events {
            source: self.source,
            overlays: self.overlays,
            source_index: 0,
            pending: [None; 2],
            remaining: self.len,
        }
    }
}
struct Events<'a> {
    source: Source<'a>,
    overlays: [Option<Overlay>; 2],
    source_index: usize,
    pending: [Option<(usize, AudioEvent)>; 2],
    remaining: usize,
}
impl Iterator for Events<'_> {
    type Item = (usize, AudioEvent);
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(event) = self.pending[0].take() {
            self.pending[0] = self.pending[1].take();
            self.remaining -= 1;
            return Some(event);
        }
        let index = self.source_index;
        let event = self.source.get(index)?;
        self.source_index += 1;
        let mut pieces = [Some(event), None, None];
        let mut count = 1;
        for overlay in self
            .overlays
            .into_iter()
            .flatten()
            .filter(|o| o.source_index == index)
        {
            pieces[count - 1] = Some(overlay.split.left);
            pieces[count] = Some(overlay.split.right);
            count += 1;
        }
        self.pending = [pieces[1].map(|e| (index, e)), pieces[2].map(|e| (index, e))];
        self.remaining -= 1;
        pieces[0].map(|e| (index, e))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for Events<'_> {}
