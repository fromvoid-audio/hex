//! Bounded retained audio dependencies. Child clip identities are provenance,
//! not new Arrangement objects. Children retain their complete source, finite
//! placement and immutable processing tempo; no nested root tables are admitted.
use crate::{AssetId, Clip, ClipSnapshot, Invalid, Project, RetainedAudioRoot, TempoMap};
use std::sync::Arc;

impl Clip {
    /// All media needed by this clip, including non-destructive grouped roots.
    /// Duplicates are allowed; consumers that own inventories deduplicate them.
    pub fn referenced_assets(&self) -> impl Iterator<Item = AssetId> + '_ {
        std::iter::once(self.asset()).chain(
            self.audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
                .into_iter()
                .flat_map(|events| events.sources.iter().map(|source| source.clip.asset())),
        )
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
            name: self.name.to_string(),
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
    /// Restore one non-nested retained processing root against this media catalog.
    /// This does not register its provenance ID or change project/history state.
    pub fn restore_retained_audio_root(
        &self,
        id: u64,
        snapshot: ClipSnapshot,
        tempo: Arc<TempoMap>,
    ) -> Result<RetainedAudioRoot, Invalid> {
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
        self.validate_retained_audio_root(&root)?;
        Ok(root)
    }
    fn validate_retained_audio_root(&self, root: &RetainedAudioRoot) -> Result<(), Invalid> {
        let clip = &root.clip;
        if root.id == 0
            || clip.id.0 == 0
            || clip.id.0 >= self.next_identity()
            || root.tempo.sample_rate() != self.sample_rate()
            || clip.has_retained_audio_roots()
        {
            return Err(Invalid(
                "retained audio root identity, clock or nesting is invalid",
            ));
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
            edit.validate(clip, &root.tempo, asset.playback_frames())?;
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
                count = count
                    .checked_add(events.sources.len())
                    .filter(|n| *n <= crate::MAX_CLIPS)
                    .ok_or(Invalid("project retained audio root limit reached"))?;
                for root in &events.sources {
                    self.validate_retained_audio_root(root)?;
                }
            }
        }
        Ok(())
    }
}
