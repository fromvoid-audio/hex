//! Revision-cached flat clip index and 1D multi-resolution LOD pyramid per track.
//!
//! Built once per `(project_generation, revision)` so pan, zoom, and pointer
//! hit-testing locate visible and hovered Arrangement clips in `O(log N)` without
//! scanning entire tracks every frame. When a track has dense sub-pixel clips,
//! the LOD pyramid allows `O(viewport_width_px)` aggregation independent of
//! total clip count.
use hex_project::{ClipId, Track, TrackId};
use std::collections::HashMap;

/// Minimum clip count on a track before building hierarchical LOD bins.
const LOD_MIN_CLIPS: usize = 256;
/// Fanout factor between consecutive LOD levels.
const LOD_FANOUT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexedClipKind {
    Audio(usize),
    Midi(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexedClip {
    pub id: ClipId,
    pub start: u64,
    pub end: u64,
    /// Running maximum of `end` over `0..=index`, enabling exact `O(log N)`
    /// visibility pruning even when audio and MIDI clips overlap on a hybrid track.
    pub prefix_max_end: u64,
    pub kind: IndexedClipKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodBin {
    pub start: u64,
    pub end: u64,
    pub prefix_max_end: u64,
    /// Half-open index range into `TrackClipIndex::clips`.
    pub clip_start: u32,
    pub clip_end: u32,
    /// Index into `TrackClipIndex::clips` for representative color/activity.
    pub representative: u32,
}

#[derive(Default)]
pub struct TrackClipIndex {
    pub clips: Vec<IndexedClip>,
    pub lod_levels: Vec<Vec<LodBin>>,
}

impl TrackClipIndex {
    /// Return the subslice of clips that can overlap `[viewport_start, viewport_end)`.
    /// Every clip overlapping the window is guaranteed to lie inside this slice;
    /// callers should still check `clip.end > viewport_start` for individual items
    /// when long earlier clips overlap shorter later clips.
    pub fn candidate_slice(&self, viewport_start: u64, viewport_end: u64) -> &[IndexedClip] {
        if self.clips.is_empty() || viewport_start >= viewport_end {
            return &[];
        }
        let first = self
            .clips
            .partition_point(|clip| clip.prefix_max_end <= viewport_start);
        if first >= self.clips.len() {
            return &[];
        }
        let end = first + self.clips[first..].partition_point(|clip| clip.start < viewport_end);
        &self.clips[first..end]
    }

    /// Choose the coarsest LOD level whose bins span at most `max_bin_frames`
    /// on average across the track, or `None` if individual clips should be visited.
    pub fn lod_level_for_pixel(&self, frames_per_device_pixel: f64) -> Option<&[LodBin]> {
        if self.lod_levels.is_empty()
            || !frames_per_device_pixel.is_finite()
            || frames_per_device_pixel <= 0.0
        {
            return None;
        }
        // Only engage LOD when average clip width is well below half a device pixel.
        let total_span = self
            .clips
            .last()
            .and_then(|last| {
                self.clips
                    .first()
                    .map(|first| last.prefix_max_end.saturating_sub(first.start))
            })
            .unwrap_or(0);
        if total_span == 0 || self.clips.len() < LOD_MIN_CLIPS {
            return None;
        }
        let avg_clip_frames = total_span as f64 / self.clips.len() as f64;
        if avg_clip_frames > frames_per_device_pixel * 0.5 {
            return None;
        }
        let mut chosen: Option<&[LodBin]> = None;
        let mut group_size = LOD_FANOUT;
        for level in &self.lod_levels {
            if avg_clip_frames * group_size as f64 <= frames_per_device_pixel * 0.5 {
                chosen = Some(level.as_slice());
            } else {
                break;
            }
            group_size = group_size.saturating_mul(LOD_FANOUT);
        }
        chosen
    }

    /// Return candidate LOD bins overlapping `[viewport_start, viewport_end)`.
    pub fn candidate_bins(bins: &[LodBin], viewport_start: u64, viewport_end: u64) -> &[LodBin] {
        if bins.is_empty() || viewport_start >= viewport_end {
            return &[];
        }
        let first = bins.partition_point(|bin| bin.prefix_max_end <= viewport_start);
        if first >= bins.len() {
            return &[];
        }
        let end = first + bins[first..].partition_point(|bin| bin.start < viewport_end);
        &bins[first..end]
    }
}

#[derive(Default)]
pub struct ArrangementClipIndex {
    generation: u64,
    revision: u64,
    state_ptr: usize,
    tracks: HashMap<TrackId, TrackClipIndex>,
}

impl ArrangementClipIndex {
    pub fn ensure_current_tracks(
        &mut self,
        generation: u64,
        revision: u64,
        state_ptr: usize,
        tracks: &[Track],
        midi_bounds: impl Fn(ClipId) -> Option<(u64, u64)>,
    ) {
        if self.generation == generation
            && self.revision == revision
            && self.state_ptr == state_ptr
            && self.tracks.len() == tracks.len()
        {
            return;
        }
        self.generation = generation;
        self.revision = revision;
        self.state_ptr = state_ptr;
        self.tracks.clear();
        self.tracks.reserve(tracks.len());
        for track in tracks {
            let mut clips = Vec::with_capacity(track.clips().len() + track.midi_clips().len());
            for (index, clip) in track.clips().iter().enumerate() {
                let Some(end) = clip.start().checked_add(clip.duration()) else {
                    continue;
                };
                if end <= clip.start() {
                    continue;
                }
                clips.push(IndexedClip {
                    id: clip.id(),
                    start: clip.start(),
                    end,
                    prefix_max_end: end,
                    kind: IndexedClipKind::Audio(index),
                });
            }
            for (index, clip) in track.midi_clips().iter().enumerate() {
                let Some((frame_start, frame_end)) = midi_bounds(clip.id()) else {
                    continue;
                };
                if frame_end <= frame_start {
                    continue;
                }
                clips.push(IndexedClip {
                    id: clip.id(),
                    start: frame_start,
                    end: frame_end,
                    prefix_max_end: frame_end,
                    kind: IndexedClipKind::Midi(index),
                });
            }
            // Sort by start time; ties keep audio before MIDI in canonical order
            // so painting order matches the legacy audio-then-MIDI traversal.
            clips.sort_unstable_by_key(|clip| {
                let kind_rank = match clip.kind {
                    IndexedClipKind::Audio(i) => (0u8, i),
                    IndexedClipKind::Midi(i) => (1u8, i),
                };
                (clip.start, kind_rank)
            });
            let mut max_end = 0u64;
            for clip in &mut clips {
                max_end = max_end.max(clip.end);
                clip.prefix_max_end = max_end;
            }
            let lod_levels = build_lod_pyramid(&clips);
            self.tracks
                .insert(track.id(), TrackClipIndex { clips, lod_levels });
        }
    }

    pub fn track(&self, track_id: TrackId) -> Option<&TrackClipIndex> {
        self.tracks.get(&track_id)
    }
}

fn build_lod_pyramid(clips: &[IndexedClip]) -> Vec<Vec<LodBin>> {
    if clips.len() < LOD_MIN_CLIPS {
        return Vec::new();
    }
    let mut levels = Vec::new();
    let mut first_level = Vec::with_capacity(clips.len().div_ceil(LOD_FANOUT));
    let mut running_max = 0u64;
    for (chunk_idx, chunk) in clips.chunks(LOD_FANOUT).enumerate() {
        let clip_start = (chunk_idx * LOD_FANOUT) as u32;
        let clip_end = clip_start + chunk.len() as u32;
        let start = chunk[0].start;
        let mut end = chunk[0].end;
        for c in &chunk[1..] {
            end = end.max(c.end);
        }
        running_max = running_max.max(end);
        // Last clip in the bin matches the painter-order winner when multiple
        // unselected clips collide in one device-pixel column.
        let representative = clip_end.saturating_sub(1);
        first_level.push(LodBin {
            start,
            end,
            prefix_max_end: running_max,
            clip_start,
            clip_end,
            representative,
        });
    }
    levels.push(first_level);

    while let Some(prev) = levels.last() {
        if prev.len() < LOD_FANOUT * 2 {
            break;
        }
        let mut next = Vec::with_capacity(prev.len().div_ceil(LOD_FANOUT));
        let mut running = 0u64;
        for chunk in prev.chunks(LOD_FANOUT) {
            let start = chunk[0].start;
            let mut end = chunk[0].end;
            for b in &chunk[1..] {
                end = end.max(b.end);
            }
            running = running.max(end);
            let last = &chunk[chunk.len() - 1];
            next.push(LodBin {
                start,
                end,
                prefix_max_end: running,
                clip_start: chunk[0].clip_start,
                clip_end: last.clip_end,
                representative: last.representative,
            });
        }
        levels.push(next);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{BeatTime, Command, Created, Editor, Project};

    fn make_clip_ids(count: usize) -> Vec<ClipId> {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "t".into() })
            .unwrap()
        else {
            panic!("track");
        };
        let mut ids = Vec::with_capacity(count);
        for i in 0..count {
            let Created::Clip(id) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_quarters(i as u64).unwrap(),
                    length: BeatTime::from_quarters(1).unwrap(),
                })
                .unwrap()
            else {
                panic!("clip");
            };
            ids.push(id);
        }
        ids
    }

    fn synthetic_clips(count: usize, step: u64, len: u64) -> TrackClipIndex {
        let ids = make_clip_ids(count);
        let mut clips = Vec::with_capacity(count);
        let mut max_end = 0u64;
        for (i, id) in ids.into_iter().enumerate() {
            let start = i as u64 * step;
            let end = start + len;
            max_end = max_end.max(end);
            clips.push(IndexedClip {
                id,
                start,
                end,
                prefix_max_end: max_end,
                kind: IndexedClipKind::Audio(i),
            });
        }
        let lod_levels = build_lod_pyramid(&clips);
        TrackClipIndex { clips, lod_levels }
    }

    #[test]
    fn candidate_slice_finds_overlapping_long_clips_and_excludes_disjoint() {
        let ids = make_clip_ids(4);
        let mut clips = vec![
            IndexedClip {
                id: ids[0],
                start: 0,
                end: 1000,
                prefix_max_end: 1000,
                kind: IndexedClipKind::Audio(0),
            },
            IndexedClip {
                id: ids[1],
                start: 100,
                end: 200,
                prefix_max_end: 1000,
                kind: IndexedClipKind::Midi(0),
            },
            IndexedClip {
                id: ids[2],
                start: 500,
                end: 600,
                prefix_max_end: 1000,
                kind: IndexedClipKind::Audio(1),
            },
            IndexedClip {
                id: ids[3],
                start: 1200,
                end: 1400,
                prefix_max_end: 1400,
                kind: IndexedClipKind::Audio(2),
            },
        ];
        let mut max_end = 0;
        for c in &mut clips {
            max_end = max_end.max(c.end);
            c.prefix_max_end = max_end;
        }
        let index = TrackClipIndex {
            clips,
            lod_levels: Vec::new(),
        };
        let slice = index.candidate_slice(400, 700);
        let matched: Vec<ClipId> = slice
            .iter()
            .filter(|c| c.end > 400 && c.start < 700)
            .map(|c| c.id)
            .collect();
        assert_eq!(matched, vec![ids[0], ids[2]]);
        assert!(index.candidate_slice(1400, 2000).is_empty());
    }

    #[test]
    fn lod_pyramid_engages_for_dense_zoomed_out_tracks() {
        let index = synthetic_clips(512, 100, 80);
        assert!(!index.lod_levels.is_empty());
        // Zoomed in: 10 frames/px -> average clip is 10px wide -> no LOD.
        assert!(index.lod_level_for_pixel(10.0).is_none());
        // Zoomed out: 5000 frames/px -> 16 clips fit inside ~0.32px -> LOD engages.
        let bins = index.lod_level_for_pixel(5000.0).expect("LOD level");
        assert!(bins.len() <= 32);
        let visible = TrackClipIndex::candidate_bins(bins, 10_000, 25_000);
        assert!(!visible.is_empty());
        assert!(visible.len() < bins.len());
    }
}
