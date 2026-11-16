//! Control-side realization of the canonical finite placement projection.
//! A placement whose occurrences all read one shared root (frame clocks, or
//! musical clocks at translation-free origins) and tile its window plays that
//! root as a loop (or crop); split and copied pieces add no PCM. Other placements
//! materialize one contiguous outer buffer shared with waveform preparation.
//! Full processing roots keep their own cache/reservation; their crops never
//! restart warp, source envelopes, events or fades. No callback scheduler changes.
use super::*;
use hex_project::{
    AudioClipTiming, AudioDomainClock, AudioProcessingProjection, AudioViewOccurrence,
    AudioViewProjection, MAX_AUDIO_DOMAIN_FRAMES,
};

/// Bound metadata traversal independently from PCM size. Total processed root
/// frames and outer PCM each share the existing 16777216-frame preparation cap.
const MAX_VIEW_OCCURRENCES: u64 = 8_192;

/// Control-only processing root for a finite placement's visual preview.
/// The same transform cache/budget owns PCM; this shared query transfers no
/// callback or canonical-state authority. Summarize on the worker, never render.
pub struct AudioPreviewRoot {
    pub audio: Audio,
    /// Technical domain-clock origin (raw beats for a musical domain).
    pub origin: i128,
}

impl AudioTransformPreparation {
    /// Prepare a resident, uncommitted frame-clock repeat-period preview.
    ///
    /// Call on a control worker with the clip's already accepted decoded source;
    /// rate and extent checks do not establish its identity. The
    /// returned clip owns its prepared audio but does not publish a graph,
    /// mutate the project or confer a saved edit. Unsupported source shapes,
    /// incompatible rate or extent and capacity/cancellation failures refuse.
    pub fn preview_repeat_period(
        &self,
        project: &CanonicalProject,
        clip_id: hex_project::ClipId,
        source: &Audio,
        period: u64,
    ) -> Result<Clip, Invalid> {
        let clip = project
            .clip(clip_id)
            .ok_or(Invalid("audio preview clip is missing"))?;
        let edit = clip
            .audio_edit()
            .ok_or(Invalid("audio repeat preview requires a captured repeat"))?;
        let asset = project
            .asset(clip.asset())
            .ok_or(Invalid("audio preview asset is missing"))?;
        edit.validate(clip, project.tempo_map(), asset.playback_frames())?;
        let projection = AudioViewProjection::new(clip, project.tempo_map())?
            .preview_frame_repeat_period(period)?;
        self.compile_view_projection(
            project.tempo_map(),
            clip,
            edit,
            asset,
            PreparedAudioSourceRef::Resident(source),
            projection,
        )
    }

    /// Resolve the retained first forward processing root for an accepted finite
    /// placement or directed content view. Use inside the caller's existing
    /// preparation transaction;
    /// failure must refuse that candidate alongside its playback preparation.
    /// Original assets must have already passed the resident source boundary.
    pub fn preview_root(
        &self,
        project: &CanonicalProject,
        clip: hex_project::ClipId,
        source: &Audio,
    ) -> Result<Option<AudioPreviewRoot>, Invalid> {
        let clip = project
            .clip(clip)
            .ok_or(Invalid("audio preview clip is missing"))?;
        let Some(edit) = clip
            .audio_edit()
            .filter(|edit| edit.placement.is_some() || edit.content_view.is_some())
        else {
            return Ok(None);
        };
        if clip.has_retained_audio_roots() {
            return Err(Invalid(
                "grouped audio preview requires every retained source",
            ));
        }
        let asset = project
            .asset(clip.asset())
            .ok_or(Invalid("audio preview asset is missing"))?;
        if source.sample_rate() != project.sample_rate()
            || source.frames().len() as u64 != asset.playback_frames()
        {
            return Err(Invalid("audio preview source differs from project media"));
        }
        edit.validate(clip, project.tempo_map(), asset.playback_frames())?;
        let projection = AudioViewProjection::new(clip, project.tempo_map())?;
        let origin = projection.root_origin();
        let root = occurrence_edit(edit, origin, self, project.tempo_map())?;
        let audio = self.prepare_audio_at_tempo(project.tempo_map(), &root, asset, source)?;
        if audio.frames().len() as u64 != root.projected_domain(project.tempo_map())?.1 {
            return Err(Invalid(
                "audio preview root differs from canonical projection",
            ));
        }
        Ok(Some(AudioPreviewRoot { audio, origin }))
    }

    /// Resolve every original resident dependency for a finite-placement root
    /// preview. Grouped clips use the same bounded composite renderer as their
    /// playback; holes, retained processing and fades survive outside the visible
    /// crop. The returned root omits outer clip gain, like `preview_root`.
    pub fn preview_root_with_sources<'a>(
        &self,
        project: &CanonicalProject,
        clip: hex_project::ClipId,
        mut source: impl FnMut(hex_project::AssetId) -> Option<&'a Audio>,
    ) -> Result<Option<AudioPreviewRoot>, Invalid> {
        let canonical = project
            .clip(clip)
            .ok_or(Invalid("audio preview clip is missing"))?;
        let Some(edit) = canonical
            .audio_edit()
            .filter(|edit| edit.placement.is_some() || edit.content_view.is_some())
        else {
            return Ok(None);
        };
        let Some(events) = edit
            .domain
            .events
            .as_ref()
            .filter(|events| !events.sources.is_empty())
        else {
            let audio = source(canonical.asset())
                .ok_or(Invalid("audio preview requires resident source"))?;
            return self.preview_root(project, clip, audio);
        };
        let asset = project
            .asset(canonical.asset())
            .ok_or(Invalid("audio preview asset is missing"))?;
        edit.validate(canonical, project.tempo_map(), asset.playback_frames())?;
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(events.sources.len())
            .map_err(|_| Invalid("grouped audio preview dependency allocation failed"))?;
        for root in canonical.retained_audio_roots() {
            sources
                .try_reserve(1)
                .map_err(|_| Invalid("grouped audio preview dependency allocation failed"))?;
            let asset = project
                .asset(root.clip.asset())
                .ok_or(Invalid("audio preview asset is missing"))?;
            let audio = source(asset.id()).ok_or(Invalid(
                "grouped audio preview requires every resident source",
            ))?;
            if audio.sample_rate() != project.sample_rate()
                || audio.frames().len() as u64 != asset.playback_frames()
            {
                return Err(Invalid("audio preview source differs from project media"));
            }
            sources.push(GroupSource { root, asset, audio });
        }
        let audio = self.prepare_group_root(canonical, &sources)?;
        let origin = AudioViewProjection::new(canonical, project.tempo_map())?.root_origin();
        if audio.frames().len() as u64 != edit.domain.clock.length() {
            return Err(Invalid(
                "grouped audio preview root differs from canonical domain",
            ));
        }
        Ok(Some(AudioPreviewRoot { audio, origin }))
    }

    pub(super) fn compile_view(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
    ) -> Result<Clip, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let projection = AudioViewProjection::new(clip, tempo_map)?;
        self.compile_view_projection(tempo_map, clip, edit, asset, source, projection)
    }

    // A preview can differ from the canonical edit, so it must never use the
    // canonical view cache key. Root processing remains shared as before.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_view_projection(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
        projection: AudioViewProjection<'_>,
    ) -> Result<Clip, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        if !std::ptr::eq(projection.clip(), clip)
            || clip
                .audio_edit()
                .is_none_or(|value| !std::ptr::eq(value, edit))
            || clip.asset() != asset.id()
            || projection.outer_frame_bounds()
                != (
                    clip.start(),
                    clip.start()
                        .checked_add(clip.duration())
                        .ok_or(Invalid("audio placement end overflow"))?,
                )
        {
            return Err(Invalid("audio projection differs from its clip"));
        }
        let canonical_cache = !projection.has_silent_repeat_gap();
        if clip.duration() > MAX_AUDIO_DOMAIN_FRAMES
            || projection.occurrence_count() > MAX_VIEW_OCCURRENCES
        {
            return Err(Invalid(
                "audio placement exceeds bounded preparation frames or occurrences",
            ));
        }
        let PreparedAudioSourceRef::Resident(source) = source else {
            return Err(Invalid(
                "finite audio placement preparation currently requires resident audio; source changes were not committed",
            ));
        };
        if source.sample_rate() != tempo_map.sample_rate()
            || source.frames().len() as u64 != asset.playback_frames()
        {
            return Err(Invalid("audio preview source differs from project media"));
        }
        let tempo = self.shared_tempo(tempo_map)?;
        // Admit all projections before PCM allocation or root preparation. Each
        // distinct musical occurrence has its own processing origin. Frame
        // roots are identical across occurrences and need processing only once.
        let mut root_frames = 0_u64;
        for index in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(index)?;
            validate_copy(clip, occurrence)?;
            if index == 0 || matches!(edit.domain.clock, AudioDomainClock::Beats(_)) {
                root_frames = root_frames
                    .checked_add(occurrence.root_frame_length)
                    .filter(|frames| *frames <= MAX_AUDIO_DOMAIN_FRAMES)
                    .ok_or(Invalid(
                        "audio placement exceeds bounded full-root preparation work",
                    ))?;
            }
        }
        if let Some(shape) = root_loop(clip, edit, &projection)? {
            let first = projection.occurrence(0)?;
            let root_edit = occurrence_edit(edit, first.root_origin, self, tempo_map)?;
            let root = self.prepare_audio_at_tempo(tempo_map, &root_edit, asset, source)?;
            return looped_clip(root, clip, shape);
        }
        if matches!(edit.domain.clock, AudioDomainClock::Beats(_))
            && let Some((root, shape)) =
                self.shared_musical_root(tempo_map, clip, edit, asset, source, &projection)?
        {
            return looped_clip(root, clip, shape);
        }
        if canonical_cache
            && let Some(audio) = self.lookup_key(source, asset, |key| {
                key.matches_view(edit, tempo_map, clip.start(), clip.duration())
            })?
        {
            return placed_clip(audio, clip);
        }
        let length = usize::try_from(clip.duration())
            .map_err(|_| Invalid("audio placement exceeds addressable frames"))?;
        let bytes = length
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("audio placement byte count overflow"))?;
        let reservation = self.reserve_with_eviction(bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(length)
            .map_err(|_| Invalid("audio placement allocation failed"))?;
        frames.resize(length, [0.0; 2]);
        let mut frame_root: Option<Audio> = None;
        for index in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(index)?;
            let root = if let Some(root) = frame_root.as_ref() {
                root
            } else {
                let root_edit = occurrence_edit(edit, occurrence.root_origin, self, tempo_map)?;
                let prepared = self.prepare_audio_at_tempo(tempo_map, &root_edit, asset, source)?;
                frame_root = Some(prepared);
                frame_root
                    .as_ref()
                    .ok_or(Invalid("audio placement root is missing"))?
            };
            if root.frames().len() as u64 != occurrence.root_frame_length {
                return Err(Invalid(
                    "audio placement root projection differs from prepared pcm",
                ));
            }
            let source_range = occurrence.root_start as usize..occurrence.root_end as usize;
            let elapsed = occurrence
                .destination_start
                .checked_sub(clip.start())
                .ok_or(Invalid("audio placement precedes its destination"))?;
            let start = usize::try_from(elapsed)
                .map_err(|_| Invalid("audio placement destination overflow"))?;
            let end = start
                .checked_add(source_range.len())
                .ok_or(Invalid("audio placement destination end overflow"))?;
            let destination = frames
                .get_mut(start..end)
                .ok_or(Invalid("audio placement copy exceeds admitted bounds"))?;
            let source = root
                .frames()
                .get(source_range)
                .ok_or(Invalid("audio placement root crop exceeds pcm"))?;
            destination.copy_from_slice(source);
            if matches!(edit.domain.clock, AudioDomainClock::Beats(_)) {
                frame_root = None;
            }
        }
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let audio = Audio::from_derived(source.sample_rate(), frames, reservation)?;
        if canonical_cache {
            self.retain(Entry {
                source: source.clone(),
                key: EntryKey::View {
                    edit: edit.clone(),
                    tempo,
                    start: clip.start(),
                    duration: clip.duration(),
                },
                algorithm_version: hex_warp::ALGORITHM_VERSION,
                conversion: asset.conversion_version(),
                source_rate: asset.source_sample_rate(),
                prepared: audio.clone(),
                bytes,
            })?;
        }
        placed_clip(audio, clip)
    }
}

pub(super) fn validate_copy(
    clip: &hex_project::Clip,
    occurrence: AudioViewOccurrence,
) -> Result<(), Invalid> {
    let source = occurrence
        .root_end
        .checked_sub(occurrence.root_start)
        .filter(|length| *length > 0)
        .ok_or(Invalid("audio placement has an empty processing crop"))?;
    let destination = occurrence
        .destination_end
        .checked_sub(occurrence.destination_start)
        .ok_or(Invalid("audio placement destination is reversed"))?;
    if source != destination
        || occurrence.destination_start < clip.start()
        || occurrence.destination_end
            > clip
                .start()
                .checked_add(clip.duration())
                .ok_or(Invalid("audio placement end overflow"))?
    {
        return Err(Invalid(
            "audio placement requires an unsupported processing rebase",
        ));
    }
    Ok(())
}

pub(super) fn occurrence_edit(
    edit: &AudioClipEdit,
    origin: i128,
    preparation: &AudioTransformPreparation,
    current_tempo: &hex_project::TempoMap,
) -> Result<AudioClipEdit, Invalid> {
    let mut root = edit.clone();
    root.content_view = None;
    root.view_start = 0;
    root.view_end = root.domain.clock.length();
    root.placement = None;
    if let AudioDomainClock::Beats(_) = edit.domain.clock {
        match &mut root.timing {
            AudioClipTiming::Musical(_) => {
                root.processing_projection = Some(Arc::new(AudioProcessingProjection {
                    tempo: preparation.shared_tempo(edit.processing_tempo(current_tempo))?,
                    origin_beat_raw: origin,
                }));
            }
            AudioClipTiming::Frozen {
                reference_start_raw,
                ..
            } => {
                *reference_start_raw = origin;
            }
            AudioClipTiming::Frames => return Err(Invalid("musical audio placement has no clock")),
        }
    }
    Ok(root)
}

impl AudioTransformPreparation {
    /// Prepare each musical occurrence root (translation-free origins share one
    /// cached render). Only when all of them are that one PCM can the placement
    /// play it as a loop; otherwise the caller materializes as before, reusing
    /// the roots prepared here through the cache.
    fn shared_musical_root(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: &Audio,
        projection: &AudioViewProjection,
    ) -> Result<Option<(Audio, RootLoop)>, Invalid> {
        let mut shared: Option<Audio> = None;
        for index in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(index)?;
            let root_edit = occurrence_edit(edit, occurrence.root_origin, self, tempo_map)?;
            let root = self.prepare_audio_at_tempo(tempo_map, &root_edit, asset, source)?;
            match &shared {
                Some(first) if !first.shares_pcm_with(&root) => return Ok(None),
                Some(_) => {}
                None => shared = Some(root),
            }
        }
        let Some(root) = shared else {
            return Ok(None);
        };
        Ok(occurrence_loop(clip, projection)?.map(|shape| (root, shape)))
    }
}

/// Loop geometry of a placement whose output equals its processing root's crop
/// window `[offset, offset + period)` looped from `phase`.
#[derive(Clone, Copy)]
pub(super) struct RootLoop {
    offset: u64,
    period: u64,
    phase: u64,
    root_frames: u64,
}

/// Frame-clock occurrences read one identical processing root through the
/// edit's crop window. When they tile the whole clip contiguously, each
/// starting where the previous one wrapped inside that window, the outer output
/// equals the window looped. Any other layout (gaps, rebased crops, differing
/// roots) returns `None` and keeps the materialized path.
pub(super) fn root_loop(
    clip: &hex_project::Clip,
    edit: &AudioClipEdit,
    projection: &AudioViewProjection,
) -> Result<Option<RootLoop>, Invalid> {
    if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
        || projection.occurrence_count() == 0
    {
        return Ok(None);
    }
    tiled_window(clip, projection, edit.view_start, edit.view_end)
}

/// Musical occurrences carry their own projected window. When every occurrence
/// already resolved to one shared root, the window is read from the first two
/// occurrences (a lone occurrence is a crop) and verified like a frame window.
pub(super) fn occurrence_loop(
    clip: &hex_project::Clip,
    projection: &AudioViewProjection,
) -> Result<Option<RootLoop>, Invalid> {
    let (start, end) = match projection.occurrence_count() {
        0 => return Ok(None),
        1 => {
            let only = projection.occurrence(0)?;
            (only.root_start, only.root_end)
        }
        _ => (
            projection.occurrence(1)?.root_start,
            projection.occurrence(0)?.root_end,
        ),
    };
    tiled_window(clip, projection, start, end)
}

/// Verify that occurrences tile the clip by looping root window `[offset, end)`.
fn tiled_window(
    clip: &hex_project::Clip,
    projection: &AudioViewProjection,
    offset: u64,
    end: u64,
) -> Result<Option<RootLoop>, Invalid> {
    let first = projection.occurrence(0)?;
    let Some(period) = end.checked_sub(offset).filter(|n| *n > 0) else {
        return Ok(None);
    };
    let Some(phase) = first
        .root_start
        .checked_sub(offset)
        .filter(|phase| *phase < period)
    else {
        return Ok(None);
    };
    if offset
        .checked_add(period)
        .is_none_or(|end| end > first.root_frame_length)
    {
        return Ok(None);
    }
    let mut elapsed = 0_u64;
    for index in 0..projection.occurrence_count() {
        let occurrence = projection.occurrence(index)?;
        let Some(remaining) = clip.duration().checked_sub(elapsed).filter(|n| *n > 0) else {
            return Ok(None);
        };
        // Both terms are below `period`, so the sum cannot overflow a u128.
        let at = ((u128::from(phase) + u128::from(elapsed % period)) % u128::from(period)) as u64;
        let length = (period - at).min(remaining);
        if occurrence.root_frame_length != first.root_frame_length
            || clip.start().checked_add(elapsed) != Some(occurrence.destination_start)
            || occurrence.root_start != offset + at
            || occurrence.root_end.checked_sub(occurrence.root_start) != Some(length)
        {
            return Ok(None);
        }
        elapsed += length;
    }
    Ok((elapsed == clip.duration()).then_some(RootLoop {
        offset,
        period,
        phase,
        root_frames: first.root_frame_length,
    }))
}

/// Play a shared processing root's window as the clip's loop, without outer
/// PCM. Split or duplicated placements of the same root share one buffer. A
/// window that never wraps is an ordinary crop of that root.
pub(super) fn looped_clip(
    root: Audio,
    clip: &hex_project::Clip,
    shape: RootLoop,
) -> Result<Clip, Invalid> {
    if root.frames().len() as u64 != shape.root_frames {
        return Err(Invalid(
            "audio placement root projection differs from prepared pcm",
        ));
    }
    let compiled = if shape
        .phase
        .checked_add(clip.duration())
        .is_some_and(|end| end <= shape.period)
    {
        Clip::region(
            root,
            clip.start(),
            clip.duration(),
            shape.offset + shape.phase,
            clip.duration(),
            false,
        )?
    } else {
        Clip::region(
            root,
            clip.start(),
            clip.duration(),
            shape.offset,
            shape.period,
            true,
        )?
        .with_source_phase(shape.phase)?
    };
    Ok(compiled.with_gain(clip.gain()))
}

pub(super) fn placed_clip(audio: Audio, clip: &hex_project::Clip) -> Result<Clip, Invalid> {
    Clip::region(
        audio,
        clip.start(),
        clip.duration(),
        0,
        clip.duration(),
        false,
    )
    .map(|compiled| compiled.with_gain(clip.gain()))
}

#[cfg(test)]
mod directed_preview_root_tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project, TrimEdge};
    #[test]
    fn directed_ordinary_clip_prepares_forward_root_before_first_repeat() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 2,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        assert!(
            editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .placement
                .is_none()
        );
        let before = editor.project().snapshot();
        let source = Audio::new(48_000, (0..8).map(|i| [i as f32 / 10.0; 2]).collect()).unwrap();
        let transforms = AudioTransformPreparation::default();
        let first = transforms
            .preview_root(editor.project(), clip, &source)
            .unwrap()
            .unwrap();
        assert_eq!(
            first.audio.frames(),
            source.frames(),
            "full forward root retains hidden prefix and original order"
        );
        let again = transforms
            .preview_root(editor.project(), clip, &source)
            .unwrap()
            .unwrap();
        assert!(first.audio.shares_pcm_with(&again.audio));
        assert_eq!(editor.project().snapshot(), before);
    }
}
