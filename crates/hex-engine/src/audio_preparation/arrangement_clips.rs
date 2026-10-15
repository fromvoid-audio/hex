//! Private final Arrangement clip owners; not a Track/Project or publication.
//! Every Box and paired destination exists before any source alias moves.
pub(super) mod tracks;
use super::*;
use crate::{Clip, ClipMetadata, owned_shared::SharedDeclaration};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementClipCapacity {
    pub tracks: tracks::ArrangementTrackCapacity,
    pub pair_backing_bytes: usize,
    pub metadata_pointee_bytes: usize,
    pub previous_growth_backing_bytes: usize,
}
impl ArrangementClipCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.pair_backing_bytes
            .checked_add(self.metadata_pointee_bytes)
            .and_then(|n| n.checked_add(self.tracks.bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding {
    slot: usize,
    track: hex_project::TrackId,
    clip: hex_project::ClipId,
    source: usize,
    handle: u64,
}
struct Pair {
    transferred: bool,
    binding: Binding,
    clip: Option<Clip>,
    metadata: Option<Box<ClipMetadata>>,
    declaration: Option<SharedDeclaration>,
    installed_handle: Option<u64>,
    installed_slot: Option<usize>,
    stream: Option<Arc<crate::StreamedSourceMetadata>>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Vacant,
    Reserved,
    Installed,
    Cleaning,
    Complete,
}
#[derive(Default)]
pub(super) struct ArrangementClips {
    rows: Vec<Pair>,
    tracks: tracks::TrackAssembly,
    phase: Phase,
    growth: usize,
    cursor: usize,
    pending_audio: Option<crate::Audio>,
    pending_stream: Option<Arc<crate::StreamedSourceMetadata>>,
    pending_metadata: Option<Box<ClipMetadata>>,
    source_disposal: Disposal,
    metadata_disposal: Disposal,
    staged: bool,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) mismatch: bool,
    #[cfg(test)]
    pub(super) probe: Option<DropProbe>,
}
impl ArrangementClips {
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementClipCapacity, AudioStorageError> {
        let metadata =
            self.rows
                .iter()
                .try_fold(usize::from(self.pending_metadata.is_some()), |n, r| {
                    n.checked_add(usize::from(r.metadata.is_some()))
                        .and_then(|n| {
                            n.checked_add(usize::from(
                                r.clip.as_ref().is_some_and(|c| c.metadata.is_some()),
                            ))
                        })
                        .ok_or(AudioStorageError::Overflow)
                })?;
        Ok(ArrangementClipCapacity {
            tracks: self.tracks.capacity(growth)?,
            pair_backing_bytes: self
                .rows
                .capacity()
                .checked_mul(mem::size_of::<Pair>())
                .ok_or(AudioStorageError::Overflow)?,
            metadata_pointee_bytes: metadata
                .checked_add(self.tracks.metadata_boxes())
                .ok_or(AudioStorageError::Overflow)?
                .checked_mul(mem::size_of::<ClipMetadata>())
                .ok_or(AudioStorageError::Overflow)?,
            previous_growth_backing_bytes: if growth {
                self.growth
                    .checked_add(self.tracks.capacity(true)?.previous_growth_backing_bytes)
                    .ok_or(AudioStorageError::Overflow)?
            } else {
                0
            },
        })
    }
    pub(super) fn reserve(
        &mut self,
        slots: &[SourceSlot],
        inputs: &OwnedAudioInputs,
        project: &hex_project::Project,
        tracks: bool,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Vacant
            || !self.rows.is_empty()
            || self.staged
            || self.pending_audio.is_some()
            || self.pending_stream.is_some()
            || self.pending_metadata.is_some()
        {
            return Err(AudioStorageError::Capacity);
        }
        // All identity/size/geometry checks precede construction of metadata.
        for s in slots {
            let source = inputs
                .sources
                .get(s.source)
                .ok_or(AudioStorageError::SourceMismatch)?;
            if s.owner.is_some()
                || s.disposal != Disposal::Unstarted
                || s.handle == 0
                || s.end <= s.start
                || s.source_length == 0
                || s.source_offset
                    .checked_add(s.source_length)
                    .is_none_or(|n| u64::try_from(n).ok().is_none_or(|n| n > source.1.frames()))
                || (!s.looping && s.phase != 0)
                || (s.looping && s.phase >= s.source_length as u64)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            s.fade
                .validate_segment(s.end - s.start)
                .map_err(|_| AudioStorageError::SourceMismatch)?;
        }
        slots
            .len()
            .checked_mul(mem::size_of::<Pair>())
            .ok_or(AudioStorageError::Overflow)?;
        slots
            .len()
            .checked_mul(mem::size_of::<ClipMetadata>())
            .ok_or(AudioStorageError::Overflow)?;
        self.phase = Phase::Reserved;
        self.growth = if slots.len() > self.rows.capacity() {
            self.capacity(false)?.pair_backing_bytes
        } else {
            0
        };
        self.rows
            .try_reserve_exact(slots.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for (slot, s) in slots.iter().enumerate() {
            self.rows.push(Pair {
                transferred: false,
                binding: Binding {
                    slot,
                    track: s.track,
                    clip: s.clip,
                    source: s.source,
                    handle: s.handle,
                },
                clip: None,
                metadata: None,
                declaration: None,
                installed_handle: None,
                installed_slot: None,
                stream: None,
            });
            let index = self.rows.len() - 1;
            let target = &mut self.rows[index];
            let fade = (s.fade.fade_in() != 0 || s.fade.fade_out() != 0).then_some(s.fade);
            if fade.is_some()
                || s.phase != 0
                || matches!(&inputs.sources[s.source].1.kind, SourceKind::Streamed(_))
            {
                // Box allocation is process-fatal-OOM excluded, like existing
                // constructors. Install directly, with no later owning local.
                target.metadata = Some(Box::new(ClipMetadata {
                    source_phase: s.phase,
                    fade,
                    streamed: None,
                }));
            }
        }
        if tracks {
            self.tracks.reserve(project, &self.rows)?;
        }
        #[cfg(test)]
        if self.mismatch {
            if let Some(r) = self.rows.first_mut() {
                r.binding.handle = r
                    .binding
                    .handle
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        Ok(())
    }
    pub(super) fn construct(
        &mut self,
        slots: &mut [SourceSlot],
        inputs: &OwnedAudioInputs,
        tracks: bool,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Reserved || self.rows.len() != slots.len() {
            return Err(AudioStorageError::Capacity);
        }
        for (slot, (r, s)) in self.rows.iter().zip(slots.iter()).enumerate() {
            let source = inputs
                .sources
                .get(s.source)
                .ok_or(AudioStorageError::SourceMismatch)?;
            let owner = s.owner.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
            let required = matches!(&owner.kind, SourceKind::Streamed(_))
                || s.phase != 0
                || s.fade.fade_in() != 0
                || s.fade.fade_out() != 0;
            if r.binding
                != (Binding {
                    slot,
                    track: s.track,
                    clip: s.clip,
                    source: s.source,
                    handle: s.handle,
                })
                || r.transferred
                || r.clip.is_some()
                || r.installed_handle.is_some()
                || r.installed_slot.is_some()
                || r.declaration.is_some()
                || r.stream.is_some()
                || r.metadata.is_some() != required
                || r.metadata.as_ref().is_some_and(|m| {
                    m.streamed.is_some()
                        || m.source_phase != s.phase
                        || m.fade
                            != (s.fade.fade_in() != 0 || s.fade.fade_out() != 0).then_some(s.fade)
                })
                || s.disposal != Disposal::Unstarted
                || owner.handle != s.handle
                || !crate::audio_input_storage::same(owner, &source.1)
                || crate::audio_input_storage::parts(owner)?
                    != crate::audio_input_storage::parts(&source.1)?
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        // Batch preflight plus exclusive borrows make all following owner moves
        // infallible. No legacy consuming builder, allocation, clone or helper.
        for (index, (r, s)) in self.rows.iter_mut().zip(slots.iter_mut()).enumerate() {
            if let Some(OwnedAudioSource { kind, handle }) = s.owner.take() {
                match kind {
                    SourceKind::Resident(audio) => {
                        r.clip = Some(Clip {
                            audio: Some(audio),
                            start: s.start,
                            end: s.end,
                            looping: s.looping,
                            source_offset: s.source_offset,
                            source_length: s.source_length,
                            metadata: r.metadata.take(),
                        });
                    }
                    SourceKind::Streamed(metadata) => {
                        let (arc, receipt) = metadata.into_paired_parts_on_control();
                        r.declaration = Some(receipt);
                        r.stream = Some(arc);
                        // Destination was checked above and cannot disappear.
                        if let Some(target) = r.metadata.as_mut() {
                            target.streamed = r.stream.take();
                        }
                        r.clip = Some(Clip {
                            audio: None,
                            start: s.start,
                            end: s.end,
                            looping: s.looping,
                            source_offset: s.source_offset,
                            source_length: s.source_length,
                            metadata: r.metadata.take(),
                        });
                    }
                }
                r.installed_handle = Some(handle);
                r.installed_slot = Some(index);
                s.disposal = Disposal::Transferred;
            }
            #[cfg(test)]
            {
                assert_ne!(
                    self.panic_after,
                    Some(index),
                    "persistent Arrangement clip construction panic"
                );
                if self.fail_after == Some(index) {
                    return Err(AudioStorageError::Allocation);
                }
            }
        }
        self.phase = Phase::Installed;
        if tracks {
            self.tracks.construct(&mut self.rows)?;
        }
        Ok(())
    }
    pub(super) fn has_next(&self) -> bool {
        !matches!(self.phase, Phase::Vacant | Phase::Complete)
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        if self.tracks.has_next() {
            self.tracks.dispose_next(&mut self.rows)?;
            return Ok(());
        }
        self.phase = Phase::Cleaning;
        if self.cursor == self.rows.len() {
            self.phase = Phase::Complete;
            return Ok(());
        }
        if !self.staged {
            if self.pending_audio.is_some()
                || self.pending_stream.is_some()
                || self.pending_metadata.is_some()
            {
                return Err(AudioStorageError::Capacity);
            }
            // Cursor/steps persist BEFORE taking actual final owner. Empty
            // source-metadata/row destinations were validated above.
            self.staged = true;
            let r = &mut self.rows[self.cursor];
            if let Some(Clip {
                audio,
                metadata,
                start: _,
                end: _,
                looping: _,
                source_offset: _,
                source_length: _,
            }) = r.clip.take()
            {
                self.pending_audio = audio;
                self.pending_metadata = metadata;
            } else {
                self.pending_metadata = r.metadata.take();
            }
            if let Some(m) = self.pending_metadata.as_mut() {
                self.pending_stream = m.streamed.take();
            }
            if r.stream.is_some() {
                if self.pending_stream.is_some() {
                    return Err(AudioStorageError::UnknownStorage);
                }
                self.pending_stream = r.stream.take();
            }
            return Ok(());
        }
        if self.source_disposal == Disposal::Unstarted {
            self.source_disposal = Disposal::Pending;
            drop(self.pending_audio.take());
            drop(self.pending_stream.take());
            #[cfg(test)]
            drop(self.probe.take());
            self.source_disposal = Disposal::Complete;
            return Ok(());
        }
        if self.source_disposal != Disposal::Complete {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.metadata_disposal != Disposal::Unstarted {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.metadata_disposal = Disposal::Pending;
        drop(self.pending_metadata.take());
        self.metadata_disposal = Disposal::Complete;
        self.cursor += 1;
        self.staged = false;
        self.source_disposal = Disposal::Unstarted;
        self.metadata_disposal = Disposal::Unstarted;
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.staged
            || self.pending_audio.is_some()
            || self.pending_stream.is_some()
            || self.pending_metadata.is_some()
            || self.rows.iter().any(|r| {
                r.transferred || r.clip.is_some() || r.metadata.is_some() || r.stream.is_some()
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        self.tracks.clear_disposed()?;
        self.rows.clear();
        self.cursor = 0;
        self.phase = Phase::Vacant;
        Ok(())
    }
    pub(super) fn installed(&self) -> usize {
        self.rows.iter().filter(|r| r.clip.is_some()).count() + self.tracks.installed_clips()
    }
    pub(super) fn track_metadata(
        &self,
        index: usize,
    ) -> Option<tracks::ArrangementTrackMetadata<'_>> {
        self.tracks.metadata(index)
    }
    #[cfg(test)]
    pub(super) fn handles(&self) -> Vec<Option<u64>> {
        self.rows.iter().map(|r| r.installed_handle).collect()
    }
    #[cfg(test)]
    pub(super) fn pending_metadata_present(&self) -> bool {
        self.pending_metadata.is_some()
    }
    #[cfg(test)]
    pub(super) fn pending_stream_empty(&self) -> bool {
        self.pending_stream.is_none()
    }
    #[cfg(test)]
    pub(super) fn clips(&self) -> impl Iterator<Item = &Clip> {
        self.rows.iter().filter_map(|r| r.clip.as_ref())
    }
    #[cfg(test)]
    pub(super) fn validate(&self, bank: &OwnedAudioInputs) -> bool {
        self.rows.iter().all(|r| {
            let Some(c) = &r.clip else {
                return false;
            };
            let Some((_, b)) = bank.sources.get(r.binding.source) else {
                return false;
            };
            r.installed_slot == Some(r.binding.slot)
                && r.installed_handle == Some(r.binding.handle)
                && match (&b.kind, &c.audio, &c.metadata) {
                    (SourceKind::Resident(a), Some(b), _) => {
                        a.shares_pcm_with(b) && r.declaration.is_none()
                    }
                    (SourceKind::Streamed(a), None, Some(m)) => {
                        m.streamed.as_ref().is_some_and(|b| {
                            Arc::ptr_eq(a.arc(), b)
                                && r.declaration.as_ref().and_then(|d| d.parts(b)) == a.parts()
                        })
                    }
                    _ => false,
                }
        })
    }
}
