//! Bounded retained audio dependencies. Child clip identities are provenance,
//! not new Arrangement objects. Children retain their complete source, finite
//! placement and immutable processing tempo. Nesting and total roots are bounded
//! before a project is admitted or installed into history.
use crate::{AssetId, Clip, ClipSnapshot, Invalid, Project, RetainedAudioRoot, TempoMap};
use std::sync::Arc;

/// Maximum retained-source levels below an arrangement clip.
pub const MAX_RETAINED_AUDIO_DEPTH: usize = 8;

impl Clip {
    /// All media needed by this clip, including non-destructive grouped roots.
    /// Duplicates are allowed; consumers that own inventories deduplicate them.
    pub fn referenced_assets(&self) -> impl Iterator<Item = AssetId> + '_ {
        std::iter::once(self.asset())
            .chain(self.retained_audio_roots().map(|root| root.clip.asset()))
    }

    /// Retained roots in depth-first order. Project admission bounds both the
    /// depth and aggregate number before consumers traverse this inventory. The
    /// traversal owns a growable stack and is for control-side use only.
    pub fn retained_audio_roots(&self) -> impl Iterator<Item = &RetainedAudioRoot> + '_ {
        let mut pending = Vec::new();
        if let Some(events) = self
            .audio_edit()
            .and_then(|edit| edit.domain.events.as_ref())
        {
            pending.extend(events.sources.iter().rev());
        }
        std::iter::from_fn(move || {
            let root = pending.pop()?;
            if let Some(events) = root
                .clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
            {
                pending.extend(events.sources.iter().rev());
            }
            Some(root)
        })
    }
    pub fn has_retained_audio_roots(&self) -> bool {
        self.audio_edit()
            .and_then(|edit| edit.domain.events.as_ref())
            .is_some_and(|events| !events.sources.is_empty())
    }
    /// Complete metadata for the storage-owned retained-source adapter.
    pub fn retained_snapshot(&self) -> ClipSnapshot {
        ClipSnapshot {
            active: self.active,
            name: self.name().to_owned(),
            flavor: self.flavor,
            id: self.id.0,
            asset: self.asset.0,
            start: self.start,
            duration: self.duration,
            source_offset: self.source_offset,
            source_length: self.source_length,
            looping: self.looping,
            source_phase: self.source_phase,
            edit: self.edit.as_deref().cloned(),
            fade: self.fade,
            gain: self.gain,
        }
    }
}
impl Project {
    /// Restore a bounded retained processing tree against this media catalog.
    /// This does not register its provenance ID or change project/history state.
    pub fn restore_retained_audio_root(
        &self,
        id: u64,
        snapshot: ClipSnapshot,
        tempo: Arc<TempoMap>,
    ) -> Result<RetainedAudioRoot, Invalid> {
        // The snapshot is public input. Bound its label before allocating the
        // shared backing, as ordinary snapshot admission does.
        crate::edit::name_valid(&snapshot.name)?;
        let root = RetainedAudioRoot {
            id,
            tempo,
            clip: Clip {
                active: snapshot.active,
                name: snapshot.name.into(),
                flavor: snapshot.flavor,
                id: crate::ClipId(snapshot.id),
                asset: AssetId(snapshot.asset),
                start: snapshot.start,
                duration: snapshot.duration,
                source_offset: snapshot.source_offset,
                source_length: snapshot.source_length,
                looping: snapshot.looping,
                source_phase: snapshot.source_phase,
                edit: snapshot.edit.map(Arc::new),
                fade: snapshot.fade,
                gain: snapshot.gain,
            },
        };
        self.validate_retained_audio_root(&root, 1, &mut 0)?;
        Ok(root)
    }
    fn validate_retained_audio_root(
        &self,
        root: &RetainedAudioRoot,
        depth: usize,
        count: &mut usize,
    ) -> Result<(), Invalid> {
        if depth > MAX_RETAINED_AUDIO_DEPTH {
            return Err(Invalid("retained audio nesting limit reached"));
        }
        *count = count
            .checked_add(1)
            .filter(|n| *n <= crate::MAX_CLIPS)
            .ok_or(Invalid("project retained audio root limit reached"))?;
        let clip = &root.clip;
        if root.id == 0
            || clip.id.0 == 0
            || clip.id.0 >= self.next_identity()
            || root.tempo.sample_rate() != self.sample_rate()
        {
            return Err(Invalid("retained audio root identity or clock is invalid"));
        }
        crate::edit::name_valid(clip.name())?;
        let asset = self
            .asset(clip.asset())
            .ok_or(Invalid("retained audio media missing"))?;
        if asset.original_source().is_some() {
            return Err(Invalid(
                "device-original media cannot be a retained audio root",
            ));
        }
        crate::edit::interval_valid(
            clip.start,
            clip.duration,
            clip.source_offset,
            clip.source_length,
            asset,
        )?;
        if (clip.looping && clip.source_phase >= clip.source_length)
            || (!clip.looping && clip.source_phase != 0)
        {
            return Err(Invalid("retained audio source phase is invalid"));
        }
        clip.fade.validate_segment(clip.duration)?;
        if let Some(edit) = clip.audio_edit() {
            if edit.repeat_period.is_some() {
                return Err(Invalid(
                    "silent repeat gaps cannot yet be nested in grouped audio",
                ));
            }
            if edit.domain.clock.length() > crate::MAX_AUDIO_DOMAIN_FRAMES
                && (edit.plain_frame_event_root()
                    || edit.is_plain_reversed_capture(clip)
                    || edit.is_plain_pitch_capture(clip))
            {
                return Err(Invalid(
                    "long frame event timelines, plain reversals and plain pitch cannot be nested as retained audio sources",
                ));
            }
            edit.validate(clip, &root.tempo, asset.playback_frames())?;
            if let Some(events) = &edit.domain.events {
                for child in &events.sources {
                    self.validate_retained_audio_root(child, depth + 1, count)?;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn validate_retained_audio_roots(&self) -> Result<(), Invalid> {
        let mut count = 0usize;
        for clip in self.tracks().iter().flat_map(|track| track.clips()) {
            if let Some(events) = clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
            {
                for root in &events.sources {
                    self.validate_retained_audio_root(root, 1, &mut count)?;
                }
            }
        }
        Ok(())
    }
}
