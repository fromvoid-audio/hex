//! Control-only transformed Arrangement origins. The semantic compiler never
//! grants native ownership: its finite PCM is copied into a fresh constructor
//! receipt, retained in the slot, then aliased to the exact graph destination.
use super::*;
use crate::{
    audio_transform::{GroupSource, sparse_group_plan},
    compile::PreparedAudioSourceRef,
};
use hex_project::{AudioDomainClock, AudioViewProjection, MAX_AUDIO_DOMAIN_FRAMES};

pub(crate) fn transformed_count(project: &hex_project::Project) -> usize {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter(|clip| clip.audio_edit().is_some())
        .count()
}

fn invalid<E>(_: E) -> AudioStorageError {
    AudioStorageError::UnsupportedGraph
}

/// A finite upper bound for the semantic compiler's processed/cached roots.
/// It traverses canonical projection metadata only, before any PCM allocation.
fn root_work(
    clip: &hex_project::Clip,
    tempo: &hex_project::TempoMap,
) -> Result<u64, AudioStorageError> {
    let Some(edit) = clip.audio_edit() else {
        return Ok(0);
    };
    if edit.placement.is_none() && edit.content_view.is_none() {
        return edit
            .projected_domain(tempo)
            .map(|(_, length, _, _)| length)
            .map_err(invalid);
    }
    let projection = AudioViewProjection::new(clip, tempo).map_err(invalid)?;
    if projection.occurrence_count() > 8_192 || clip.duration() > MAX_AUDIO_DOMAIN_FRAMES {
        return Err(AudioStorageError::Capacity);
    }
    let mut frames = 0u64;
    for index in 0..projection.occurrence_count() {
        if index == 0 || matches!(edit.domain.clock, AudioDomainClock::Beats(_)) {
            frames = frames
                .checked_add(
                    projection
                        .occurrence(index)
                        .map_err(invalid)?
                        .root_frame_length,
                )
                .filter(|frames| *frames <= MAX_AUDIO_DOMAIN_FRAMES)
                .ok_or(AudioStorageError::Capacity)?;
        }
    }
    Ok(frames)
}

fn clip_quote(
    clip: &hex_project::Clip,
    tempo: &hex_project::TempoMap,
) -> Result<(u64, u64, usize), AudioStorageError> {
    let Some(edit) = clip.audio_edit() else {
        return Ok((0, 0, 0));
    };
    let grouped = edit
        .domain
        .events
        .as_ref()
        .filter(|events| !events.sources.is_empty());
    let sparse = sparse_group_plan(clip).map_err(invalid)?;
    let frames = if let Some((_, frames, _)) = sparse {
        u64::try_from(frames).map_err(|_| AudioStorageError::Overflow)?
    } else if grouped.is_some() || edit.placement.is_some() || edit.content_view.is_some() {
        clip.duration()
    } else {
        edit.projected_domain(tempo).map_err(invalid)?.1
    };
    if frames == 0 || frames > MAX_AUDIO_DOMAIN_FRAMES {
        return Err(AudioStorageError::Capacity);
    }
    let work = if grouped.is_some() {
        let mut work = if sparse.is_some() {
            frames
        } else {
            edit.domain.clock.length()
        };
        for root in clip.retained_audio_roots() {
            work = work
                .checked_add(root_work(&root.clip, &root.tempo)?)
                .and_then(|n| n.checked_add(root.clip.duration()))
                .filter(|frames| *frames <= MAX_AUDIO_DOMAIN_FRAMES)
                .ok_or(AudioStorageError::Capacity)?;
        }
        work.checked_add(if sparse.is_some() { 0 } else { clip.duration() })
            .ok_or(AudioStorageError::Overflow)?
    } else {
        root_work(clip, tempo)?
            .checked_add(if edit.placement.is_some() || edit.content_view.is_some() {
                clip.duration()
            } else {
                0
            })
            .ok_or(AudioStorageError::Overflow)?
    };
    if work > MAX_AUDIO_DOMAIN_FRAMES * 2 {
        return Err(AudioStorageError::Capacity);
    }
    let layout_bytes = if let Some((_, _, spans)) = sparse {
        crate::SparseFrameMap::allocation_bytes(spans).ok_or(AudioStorageError::UnknownStorage)?
    } else {
        0
    };
    Ok((frames, work, layout_bytes))
}

pub(crate) fn prepared_pcm_frames(
    project: &hex_project::Project,
) -> Result<u64, AudioStorageError> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .try_fold(0u64, |n, clip| {
            n.checked_add(clip_quote(clip, project.tempo_map())?.0)
                .filter(|frames| {
                    *frames
                        <= (crate::audio_transform::MAX_RETAINED_DERIVED_BYTES
                            / mem::size_of::<crate::Frame>()) as u64
                })
                .ok_or(AudioStorageError::Capacity)
        })
}

/// Retained fresh PCM/header bytes and maximum sequential compiler workspace.
/// Uncached compilation keeps only the current clip's finite PCM. Eight times
/// processed PCM covers root/outer/child output, event/mapping copies and the
/// fresh constructor copy; warp owns a separate fixed scratch ceiling. Metadata
/// uses explicit bounded map/event/point/tempo families, with room for wrappers.
pub(crate) fn transformed_audio_quote(
    project: &hex_project::Project,
) -> Result<(usize, usize), AudioStorageError> {
    prepared_pcm_frames(project)?;
    let mut retained = 0usize;
    let mut workspace = 0usize;
    for clip in project.tracks().iter().flat_map(|track| track.clips()) {
        let (frames, work, layout_bytes) = clip_quote(clip, project.tempo_map())?;
        if frames == 0 {
            continue;
        }
        let frames = usize::try_from(frames).map_err(|_| AudioStorageError::Overflow)?;
        let receipt = crate::pcm_arc::PcmArcReceipt::current(frames, frames)
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        retained = retained
            .checked_add(
                receipt
                    .declared_bytes()
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .and_then(|n| n.checked_add(layout_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        workspace = workspace.max(
            workspace_for_frames(work)?
                .checked_add(nested_metadata_workspace(clip)?)
                .ok_or(AudioStorageError::Overflow)?,
        );
    }
    Ok((retained, workspace))
}

fn workspace_for_frames(work: u64) -> Result<usize, AudioStorageError> {
    usize::try_from(work)
        .ok()
        .and_then(|n| n.checked_mul(mem::size_of::<crate::Frame>()))
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_add(hex_warp::MAX_SCRATCH_BYTES))
        .and_then(|n| n.checked_add(crate::audio_transform::native_metadata_workspace_bytes()?))
        .ok_or(AudioStorageError::Overflow)
}

pub(crate) fn workspace_ceiling() -> Result<usize, AudioStorageError> {
    workspace_for_frames(MAX_AUDIO_DOMAIN_FRAMES * 2)?
        .checked_add(nested_metadata_bound(
            hex_project::MAX_CLIPS,
            hex_project::MAX_RETAINED_AUDIO_DEPTH,
        )?)
        .ok_or(AudioStorageError::Overflow)
}

fn nested_metadata_bound(roots: usize, depth: usize) -> Result<usize, AudioStorageError> {
    if depth <= 1 {
        return Ok(0);
    }
    let per_level = crate::audio_transform::native_metadata_workspace_bytes()
        .ok_or(AudioStorageError::Overflow)?;
    let identities = mem::size_of::<GroupSource<'static>>()
        .checked_add(mem::size_of::<(crate::Audio, u32, u32)>())
        .ok_or(AudioStorageError::Overflow)?;
    per_level
        .checked_mul(depth - 1)
        .and_then(|n| {
            roots
                .checked_mul(depth)?
                .checked_mul(identities)?
                .checked_add(n)
        })
        .ok_or(AudioStorageError::Overflow)
}

fn nested_metadata_workspace(clip: &hex_project::Clip) -> Result<usize, AudioStorageError> {
    let mut pending = vec![(clip, 0usize)];
    let mut roots = 0usize;
    let mut depth = 0usize;
    while let Some((clip, level)) = pending.pop() {
        if let Some(events) = clip
            .audio_edit()
            .and_then(|edit| edit.domain.events.as_ref())
        {
            for root in &events.sources {
                roots = roots.checked_add(1).ok_or(AudioStorageError::Overflow)?;
                depth = depth.max(level + 1);
                pending.push((&root.clip, level + 1));
            }
        }
    }
    nested_metadata_bound(roots, depth)
}

pub(super) fn validate_sources(
    project: &hex_project::Project,
    bank: &OwnedAudioInputs,
    clip: &hex_project::Clip,
) -> Result<(), AudioStorageError> {
    for asset_id in clip.referenced_assets() {
        let asset = project
            .asset(asset_id)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let (_, source) = bank
            .sources
            .iter()
            .find(|(id, _)| *id == asset_id)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if source.sample_rate() != project.sample_rate()
            || source.frames() != asset.playback_frames()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if clip.audio_edit().is_some() && !matches!(source.kind, SourceKind::Resident(_)) {
            return Err(AudioStorageError::UnsupportedGraph);
        }
    }
    Ok(())
}

/// The existing sampler request lends its cancellation flag. Semantic
/// compilation remains one bounded control call; a flag raised during that call
/// refuses before alias construction/publication, retaining roots for cleanup.
pub(super) fn check_cancel(
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), AudioStorageError> {
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return Err(AudioStorageError::UnsupportedGraph);
    }
    Ok(())
}

pub(super) fn construct_origin(
    project: &hex_project::Project,
    bank: &OwnedAudioInputs,
    slot: &mut SourceSlot,
) -> Result<(), AudioStorageError> {
    if slot.prepared_origin.is_some() {
        return Err(AudioStorageError::SourceMismatch);
    }
    let clip = project
        .track(slot.track)
        .and_then(|track| track.clips().iter().find(|clip| clip.id() == slot.clip))
        .ok_or(AudioStorageError::SourceMismatch)?;
    let transforms = crate::audio_transform::AudioTransformPreparation::new_uncached_on_control();
    let mut compiled = if let Some(events) = clip
        .audio_edit()
        .and_then(|edit| edit.domain.events.as_ref())
        .filter(|events| !events.sources.is_empty())
    {
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(events.sources.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for root in clip.retained_audio_roots() {
            sources
                .try_reserve(1)
                .map_err(|_| AudioStorageError::Allocation)?;
            let asset = project
                .asset(root.clip.asset())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let (_, source) = bank
                .sources
                .iter()
                .find(|(id, _)| *id == asset.id())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let SourceKind::Resident(audio) = &source.kind else {
                return Err(AudioStorageError::UnsupportedGraph);
            };
            sources.push(GroupSource { root, asset, audio });
        }
        Some(
            transforms
                .compile_arrangement_group(clip, &sources)
                .map_err(invalid)?,
        )
    } else {
        let asset = project
            .asset(clip.asset())
            .ok_or(AudioStorageError::SourceMismatch)?;
        let SourceKind::Resident(audio) = &bank.sources[slot.source].1.kind else {
            return Err(AudioStorageError::UnsupportedGraph);
        };
        transforms
            .compile_clip(
                project,
                clip,
                asset,
                PreparedAudioSourceRef::Resident(audio),
            )
            .map_err(invalid)?
    };
    if let Some(compiled) = compiled.as_ref() {
        let metadata = compiled.metadata.as_ref();
        if compiled.start != slot.start
            || compiled.end != slot.end
            || metadata.is_some_and(|metadata| {
                metadata.source_phase != 0 || metadata.fade.is_some() || metadata.streamed.is_some()
            })
            || metadata.map_or(1.0, |metadata| metadata.gain) != slot.gain
        {
            return Err(AudioStorageError::SourceMismatch);
        }
    }
    let layout = compiled
        .as_mut()
        .and_then(|clip| clip.metadata.as_mut())
        .and_then(|metadata| metadata.sparse.take());
    if layout.as_ref().map(|map| map.packed_len()) != slot.sparse_frames {
        return Err(AudioStorageError::SourceMismatch);
    }
    if let Some(layout) = &layout {
        let (_, _, reserved) = clip_quote(clip, project.tempo_map())?;
        if layout.retained_bytes().is_none_or(|bytes| bytes > reserved) {
            return Err(AudioStorageError::ReservationExceeded);
        }
    }
    slot.sparse = layout;
    let mut frames = Vec::new();
    let length = compiled
        .as_ref()
        .and_then(|clip| clip.audio.as_ref())
        .map(|audio| audio.frames().len())
        .unwrap_or(usize::try_from(clip.duration()).map_err(|_| AudioStorageError::Overflow)?);
    frames
        .try_reserve_exact(length)
        .map_err(|_| AudioStorageError::Allocation)?;
    if frames.capacity() != length {
        return Err(AudioStorageError::ReservationExceeded);
    }
    if let Some(audio) = compiled.as_ref().and_then(|clip| clip.audio.as_ref()) {
        frames.extend_from_slice(audio.frames());
    } else {
        frames.resize(length, [0.0; 2]);
    }
    // Install the only fresh native root immediately in the persistent slot.
    slot.prepared_origin = Some(
        OwnedAudioSource::try_resident(project.sample_rate(), frames)
            .map_err(|refusal| refusal.reason)?,
    );
    if let Some(compiled) = compiled.take() {
        slot.start = compiled.start;
        slot.end = compiled.end;
        slot.source_offset = compiled.source_offset;
        slot.source_length = compiled.source_length;
        slot.looping = compiled.looping;
        slot.gain = compiled
            .metadata
            .as_ref()
            .map_or(1.0, |metadata| metadata.gain);
        slot.phase = compiled
            .metadata
            .as_ref()
            .map_or(0, |metadata| metadata.source_phase);
        slot.fade = compiled
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.fade)
            .unwrap_or(hex_project::ClipFade::neutral(clip.duration()).map_err(invalid)?);
    }
    Ok(())
}
