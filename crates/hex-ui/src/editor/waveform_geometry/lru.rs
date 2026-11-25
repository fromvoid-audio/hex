//! O(1) indexed LRU storage and hashing for cached waveform geometry.
use super::*;

pub(in crate::editor) enum SlotBucket {
    One(usize),
    Many(Vec<usize>),
}

pub(in crate::editor) struct GeometryLruNode {
    pub(super) entry: WaveformGeometryEntry,
    pub(super) hash: u64,
    pub(super) prev: Option<usize>,
    pub(super) next: Option<usize>,
}

/// Arena-backed doubly-linked LRU with a 64-bit hash bucket index for O(1)
/// lookup, promotion, invalidation, and front eviction without unsafe code.
#[derive(Default)]
pub(in crate::editor) struct GeometryLru {
    pub(super) nodes: Vec<Option<GeometryLruNode>>,
    pub(super) free: Vec<usize>,
    pub(super) head: Option<usize>,
    pub(super) tail: Option<usize>,
    pub(super) len: usize,
    pub(super) index: HashMap<u64, SlotBucket>,
    pub(super) stretch_slot: Option<usize>,
}

#[cfg(test)]
pub(super) struct GeometryLruIter<'a> {
    pub(super) nodes: &'a [Option<GeometryLruNode>],
    pub(super) current: Option<usize>,
    pub(super) remaining: usize,
}

#[cfg(test)]
impl<'a> Iterator for GeometryLruIter<'a> {
    type Item = &'a WaveformGeometryEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let slot = self.current?;
        let node = self.nodes.get(slot)?.as_ref()?;
        self.current = node.next;
        self.remaining -= 1;
        Some(&node.entry)
    }
}

impl GeometryLru {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> GeometryLruIter<'_> {
        GeometryLruIter {
            nodes: &self.nodes,
            current: self.head,
            remaining: self.len,
        }
    }

    pub(super) fn find_slot_with_hash(
        &self,
        hash: u64,
        key: &WaveformGeometryKey,
        source: &Arc<CachedWaveform>,
    ) -> Option<usize> {
        let matches = |slot: usize| {
            self.nodes
                .get(slot)
                .and_then(Option::as_ref)
                .is_some_and(|node| {
                    Arc::ptr_eq(&node.entry.source, source) && &node.entry.key == key
                })
        };
        match self.index.get(&hash)? {
            SlotBucket::One(slot) => matches(*slot).then_some(*slot),
            SlotBucket::Many(slots) => slots.iter().copied().find(|&slot| matches(slot)),
        }
    }

    pub(super) fn find_slot(
        &self,
        key: &WaveformGeometryKey,
        source: &Arc<CachedWaveform>,
    ) -> Option<usize> {
        self.find_slot_with_hash(geometry_entry_hash(key, source), key, source)
    }

    pub(super) fn geometry_at(&self, slot: usize) -> Option<Arc<WaveformGeometry>> {
        self.nodes
            .get(slot)
            .and_then(Option::as_ref)
            .map(|node| Arc::clone(&node.entry.geometry))
    }

    pub(super) fn move_to_back(&mut self, slot: usize) {
        if self.tail == Some(slot) {
            return;
        }
        let Some((prev, next)) = self
            .nodes
            .get(slot)
            .and_then(Option::as_ref)
            .map(|node| (node.prev, node.next))
        else {
            return;
        };
        if let Some(prev_slot) = prev {
            if let Some(prev_node) = self.nodes.get_mut(prev_slot).and_then(Option::as_mut) {
                prev_node.next = next;
            }
        } else {
            self.head = next;
        }
        if let Some(next_slot) = next {
            if let Some(next_node) = self.nodes.get_mut(next_slot).and_then(Option::as_mut) {
                next_node.prev = prev;
            }
        } else {
            self.tail = prev;
        }
        let old_tail = self.tail;
        if let Some(node) = self.nodes.get_mut(slot).and_then(Option::as_mut) {
            node.prev = old_tail;
            node.next = None;
        }
        if let Some(tail_slot) = old_tail {
            if let Some(tail_node) = self.nodes.get_mut(tail_slot).and_then(Option::as_mut) {
                tail_node.next = Some(slot);
            }
        } else {
            self.head = Some(slot);
        }
        self.tail = Some(slot);
    }

    pub(super) fn insert_index(&mut self, hash: u64, slot: usize) {
        match self.index.get_mut(&hash) {
            None => {
                self.index.insert(hash, SlotBucket::One(slot));
            }
            Some(bucket) => match bucket {
                SlotBucket::One(existing) => {
                    let existing = *existing;
                    *bucket = SlotBucket::Many(vec![existing, slot]);
                }
                SlotBucket::Many(slots) => slots.push(slot),
            },
        }
    }

    pub(super) fn remove_index(&mut self, hash: u64, slot: usize) {
        let remove_key = match self.index.get_mut(&hash) {
            None => false,
            Some(SlotBucket::One(existing)) => *existing == slot,
            Some(bucket @ SlotBucket::Many(_)) => {
                let SlotBucket::Many(slots) = bucket else {
                    return;
                };
                slots.retain(|&candidate| candidate != slot);
                if slots.is_empty() {
                    true
                } else if slots.len() == 1 {
                    let only = slots[0];
                    *bucket = SlotBucket::One(only);
                    false
                } else {
                    false
                }
            }
        };
        if remove_key {
            self.index.remove(&hash);
        }
    }

    pub(super) fn push_back_with_hash(&mut self, entry: WaveformGeometryEntry, hash: u64) {
        let is_stretch = matches!(entry.key.mapping, WaveformMappingKey::Stretch(_));
        let slot = if let Some(slot) = self.free.pop() {
            slot
        } else {
            let slot = self.nodes.len();
            self.nodes.push(None);
            slot
        };
        let old_tail = self.tail;
        if let Some(slot_ref) = self.nodes.get_mut(slot) {
            *slot_ref = Some(GeometryLruNode {
                entry,
                hash,
                prev: old_tail,
                next: None,
            });
        }
        if let Some(tail_slot) = old_tail {
            if let Some(tail_node) = self.nodes.get_mut(tail_slot).and_then(Option::as_mut) {
                tail_node.next = Some(slot);
            }
        } else {
            self.head = Some(slot);
        }
        self.tail = Some(slot);
        self.len = self.len.saturating_add(1);
        self.insert_index(hash, slot);
        if is_stretch {
            self.stretch_slot = Some(slot);
        }
    }

    pub(super) fn push_back(&mut self, entry: WaveformGeometryEntry) {
        let hash = geometry_entry_hash(&entry.key, &entry.source);
        self.push_back_with_hash(entry, hash);
    }

    pub(super) fn remove_slot(&mut self, slot: usize) -> Option<WaveformGeometryEntry> {
        let node = self.nodes.get_mut(slot)?.take()?;
        if let Some(prev_slot) = node.prev {
            if let Some(prev_node) = self.nodes.get_mut(prev_slot).and_then(Option::as_mut) {
                prev_node.next = node.next;
            }
        } else {
            self.head = node.next;
        }
        if let Some(next_slot) = node.next {
            if let Some(next_node) = self.nodes.get_mut(next_slot).and_then(Option::as_mut) {
                next_node.prev = node.prev;
            }
        } else {
            self.tail = node.prev;
        }
        self.len = self.len.saturating_sub(1);
        self.remove_index(node.hash, slot);
        if self.stretch_slot == Some(slot) {
            self.stretch_slot = None;
        }
        self.free.push(slot);
        Some(node.entry)
    }

    pub(super) fn pop_front(&mut self) -> Option<WaveformGeometryEntry> {
        let slot = self.head?;
        self.remove_slot(slot)
    }

    pub(super) fn remove_stretch(&mut self) -> Option<WaveformGeometryEntry> {
        let slot = self.stretch_slot.take()?;
        self.remove_slot(slot)
    }

    pub(super) fn clear(&mut self) {
        self.nodes.clear();
        self.free.clear();
        self.head = None;
        self.tail = None;
        self.len = 0;
        self.index.clear();
        self.stretch_slot = None;
    }
}

#[inline]
pub(in crate::editor) fn mix_u64(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x1000_0000_01b3);
}

/// Deterministic 64-bit bucket hash strictly consistent with
/// `Arc::ptr_eq(source) && key == other_key` (any collision is resolved exactly
/// inside `SlotBucket::Many`).
pub(in crate::editor) fn geometry_entry_hash(
    key: &WaveformGeometryKey,
    source: &Arc<CachedWaveform>,
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    mix_u64(&mut hash, Arc::as_ptr(source) as usize as u64);
    #[cfg(test)]
    mix_u64(&mut hash, u64::from(key.reference_stereo));
    mix_u64(
        &mut hash,
        match key.amplitude_scale {
            WaveformAmplitudeScale::Linear => 0,
            WaveformAmplitudeScale::Decibel => 1,
        },
    );
    mix_u64(&mut hash, u64::from(key.height));
    mix_u64(&mut hash, u64::from(key.scale));
    match &key.placement {
        WaveformPlacementKey::Window {
            visible_start,
            visible_end,
            origin_x,
            origin_y,
            width,
            grid,
        } => {
            mix_u64(&mut hash, 1);
            mix_u64(&mut hash, *visible_start);
            mix_u64(&mut hash, *visible_end);
            mix_u64(&mut hash, u64::from(*origin_x));
            mix_u64(&mut hash, u64::from(*origin_y));
            mix_u64(&mut hash, u64::from(*width));
            if let Some(grid) = grid {
                mix_u64(&mut hash, 1);
                for value in grid {
                    mix_u64(&mut hash, *value);
                }
            } else {
                mix_u64(&mut hash, 0);
            }
        }
        WaveformPlacementKey::Coarse {
            first_frame,
            frames_per_bin,
            bins,
            pixel_pitch,
            detail_budget,
        } => {
            mix_u64(&mut hash, 2);
            mix_u64(&mut hash, *first_frame);
            mix_u64(&mut hash, *frames_per_bin);
            mix_u64(&mut hash, *bins as u64);
            mix_u64(&mut hash, u64::from(*pixel_pitch));
            mix_u64(&mut hash, *detail_budget as u64);
        }
    }
    match &key.mapping {
        WaveformMappingKey::PreparedFade(fade) => {
            mix_u64(&mut hash, 1);
            mix_u64(&mut hash, fade.timeline_origin);
            mix_u64(&mut hash, fade.root_offset);
        }
        WaveformMappingKey::PreparedMusicalFade(fade) => {
            mix_u64(&mut hash, 2);
            mix_u64(&mut hash, fade.start());
        }
        WaveformMappingKey::Frame(frame_key, clock) => {
            mix_u64(&mut hash, 3);
            mix_u64(&mut hash, frame_key.start);
            match clock {
                waveform_frame::Clock::Arrangement => mix_u64(&mut hash, 0),
                waveform_frame::Clock::Event(id) => {
                    mix_u64(&mut hash, 1);
                    mix_u64(&mut hash, *id);
                }
            }
        }
        WaveformMappingKey::MusicalEventFade(_, event) => {
            mix_u64(&mut hash, 4);
            mix_u64(&mut hash, u64::from(event.is_some()));
        }
        WaveformMappingKey::MusicalEvent(_) => {
            mix_u64(&mut hash, 5);
        }
        WaveformMappingKey::Stretch(_) => {
            mix_u64(&mut hash, 6);
        }
        WaveformMappingKey::Loop(_) => {
            mix_u64(&mut hash, 7);
        }
        WaveformMappingKey::Prepared {
            timeline_origin,
            root_offset,
            gain,
        } => {
            mix_u64(&mut hash, 8);
            mix_u64(&mut hash, *timeline_origin);
            mix_u64(&mut hash, *root_offset);
            mix_u64(&mut hash, *gain as i64 as u64);
        }
        WaveformMappingKey::GainedSource {
            timeline_origin,
            gain,
            detail,
        } => {
            mix_u64(&mut hash, 9);
            mix_u64(&mut hash, *timeline_origin);
            mix_u64(&mut hash, u64::from(*gain));
            mix_u64(&mut hash, u64::from(*detail));
        }
        WaveformMappingKey::Clip {
            id,
            transformed,
            start,
            duration,
            source_offset,
            source_length,
            source_phase,
            looping,
            timeline_origin,
            gain,
            fade,
        } => {
            mix_u64(&mut hash, 10);
            mix_u64(&mut hash, id.unwrap_or(u64::MAX));
            mix_u64(&mut hash, u64::from(*transformed));
            mix_u64(&mut hash, *start);
            mix_u64(&mut hash, *duration);
            mix_u64(&mut hash, *source_offset);
            mix_u64(&mut hash, *source_length);
            mix_u64(&mut hash, *source_phase);
            mix_u64(&mut hash, u64::from(*looping));
            mix_u64(&mut hash, *timeline_origin);
            mix_u64(&mut hash, u64::from(*gain));
            mix_u64(&mut hash, u64::from(fade.is_some()));
        }
        WaveformMappingKey::Source { timeline_origin } => {
            mix_u64(&mut hash, 11);
            mix_u64(&mut hash, *timeline_origin);
        }
    }
    hash
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor) fn painted_clip_waveform_outline(
    editor: &gpui::Entity<super::super::EditorView>,
    visual: &mut gpui::VisualTestContext,
    clip: hex_project::ClipId,
) -> [u64; 2] {
    let selector: &'static str =
        Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let clip_bounds = visual.debug_bounds(selector).expect("painted clip");
    editor.read_with(visual, |view, _| {
        view.waveform_geometry_cache
            .borrow()
            .painted_waveforms()
            .iter()
            .rev()
            .find(|paint| clip_bounds.contains(&paint.bounds.origin))
            .expect("completed clip waveform repaint")
            .outline_fingerprint
    })
}
