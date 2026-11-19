//! Control-owned multi-source event composition. Each retained source is prepared
//! with its captured tempo and rendered through the existing finite clip law.
//! All metadata/work admission precedes PCM allocation; no callback work or files.
use super::*;
use hex_project::{AudioDomainClock, AudioViewProjection, MAX_AUDIO_DOMAIN_FRAMES};

pub(crate) struct GroupSource<'a> {
    pub root: &'a hex_project::RetainedAudioRoot,
    pub asset: &'a Asset,
    pub audio: &'a Audio,
}

/// Shared selection law for semantic compilation and native storage quoting.
pub(crate) fn sparse_group_plan(
    clip: &hex_project::Clip,
) -> Result<Option<(u64, usize, usize)>, Invalid> {
    let Some(edit) = clip.audio_edit() else {
        return Ok(None);
    };
    if edit.placement.is_some() || edit.content_view.is_some() {
        return Ok(None);
    }
    let Some(events) = &edit.domain.events else {
        return Ok(None);
    };
    let Some(anchor) = events.sources.first() else {
        return Ok(None);
    };
    let extent = edit.domain.clock.length();
    events.validate(extent)?;
    let Some((frames, spans)) = crate::SparseFrameMap::packing_size(extent, &events.events)? else {
        return Ok(None);
    };
    let projection = AudioViewProjection::new(clip, &anchor.tempo)?;
    if projection.occurrence_count() != 1 {
        return Ok(None);
    }
    let occurrence = projection.occurrence(0)?;
    Ok((occurrence.destination_start == clip.start()
        && occurrence.root_end.checked_sub(occurrence.root_start) == Some(clip.duration()))
    .then_some((occurrence.root_start, frames, spans)))
}

impl AudioTransformPreparation {
    /// Arrangement playback and worker peaks share the sparse frame law.
    pub(crate) fn compile_arrangement_group(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
    ) -> Result<Clip, Invalid> {
        self.compile_group_playback(clip, sources, true)
    }

    pub(crate) fn compile_group(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
    ) -> Result<Clip, Invalid> {
        self.compile_group_playback(clip, sources, false)
    }

    fn compile_group_playback(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
        sparse: bool,
    ) -> Result<Clip, Invalid> {
        let edit = clip
            .audio_edit()
            .ok_or(Invalid("grouped audio edit missing"))?;
        let fade = clip
            .projected_outer_fade(group_tempo(edit)?)?
            .map(|projection| projection.frame_playback_envelope())
            .transpose()?;
        self.compile_group_with_outer_fade(clip, sources, edit, fade, sparse)
    }

    /// The group instance fade belongs to destination playback. Even sparse
    /// output keeps its occupied-source packing and the original child fades.
    pub(super) fn compile_group_with_outer_fade(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
        edit: &AudioClipEdit,
        fade: Option<hex_project::ClipFade>,
        sparse: bool,
    ) -> Result<Clip, Invalid> {
        if fade.is_some() != (edit.domain.fade_clock == hex_project::AudioClipFadeClock::Outer) {
            return Err(Invalid("grouped fade differs from its processing clock"));
        }
        let tempo = group_tempo(edit)?;
        let processing = super::processing_root::unfaded_outer_edit(edit, tempo)?;
        let edit = processing.as_ref();
        let finish = |compiled: Clip| match fade {
            Some(fade) => compiled.with_fade(fade),
            None => Ok(compiled),
        };
        if sparse
            && let Some((offset, _, _)) = sparse_group_plan(clip)?
            && let Some(events) = &edit.domain.events
            && let Some(layout) =
                crate::SparseFrameMap::for_events(edit.domain.clock.length(), &events.events)?
        {
            let audio = self.prepare_group_audio(clip, sources, edit, true, Some(&layout))?;
            return Clip::sparse_region(
                audio,
                Arc::new(layout),
                clip.start(),
                clip.duration(),
                offset,
            )
            .and_then(|compiled| finish(compiled.with_gain(clip.gain())));
        }
        // A loop-shaped frame placement plays the shared composite root, so
        // split or duplicated copies of the group add no outer PCM.
        if edit.content_view.is_none()
            && let Some(shape) =
                super::view::root_loop(clip, edit, &AudioViewProjection::new(clip, tempo)?)?
        {
            let root = self.prepare_group_audio(clip, sources, edit, true, None)?;
            return super::view::looped_clip(root, clip, shape).and_then(finish);
        }
        let audio = self.prepare_group_audio(clip, sources, edit, false, None)?;
        super::view::placed_clip(audio, clip).and_then(finish)
    }

    /// The full composite processing domain, before finite crop/repeat and
    /// outer clip gain. Preview and playback share this exact composition law.
    pub(super) fn prepare_group_root(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
    ) -> Result<Audio, Invalid> {
        let edit = clip
            .audio_edit()
            .ok_or(Invalid("grouped audio edit missing"))?;
        let processing = super::processing_root::unfaded_outer_edit(edit, group_tempo(edit)?)?;
        self.prepare_group_audio(clip, sources, processing.as_ref(), true, None)
    }

    fn prepare_group_audio(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
        edit: &AudioClipEdit,
        root_only: bool,
        packing: Option<&crate::SparseFrameMap>,
    ) -> Result<Audio, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let events = edit
            .domain
            .events
            .as_ref()
            .ok_or(Invalid("grouped audio events missing"))?;
        events.validate(edit.domain.clock.length())?;
        if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
            || edit.domain.warp_enabled
            || edit.domain.pitch_cents != 0
            || !edit.domain.source_gain.is_empty()
            || sources.is_empty()
            || events.events.iter().any(|event| event.source == 0)
        {
            return Err(Invalid(
                "grouped audio supports frame edits, fades, gain and finite placement; outer source processing is unsupported",
            ));
        }
        // A grouped root is frozen in frames, so tempo only serves the finite
        // placement projection. Its frame clock is independent of this map.
        let first_root = events
            .sources
            .first()
            .ok_or(Invalid("grouped audio source missing"))?;
        let anchor = sources
            .iter()
            .find(|source| std::ptr::eq(source.root, first_root))
            .ok_or(Invalid("grouped audio dependency missing"))?;
        let tempo = &anchor.root.tempo;
        let projection = AudioViewProjection::new(clip, tempo)?;
        if (packing.is_none() && clip.duration() > MAX_AUDIO_DOMAIN_FRAMES)
            || projection.occurrence_count() > 8_192
        {
            return Err(Invalid("grouped audio exceeds bounded placement work"));
        }
        // A single forward occurrence can be copied from its exact root
        // interval. Root previews, directed views and multiple occurrences
        // still use the complete root.
        let root_window =
            if !root_only && edit.content_view.is_none() && projection.occurrence_count() == 1 {
                let occurrence = projection.occurrence(0)?;
                occurrence.root_start..occurrence.root_end
            } else {
                0..edit.domain.clock.length()
            };
        let root_length = packing.map_or(root_window.end - root_window.start, |map| {
            map.packed_len() as u64
        });
        let mut work = root_length;
        for source in sources {
            let root = &source.root.clip;
            let root_work = if let Some(inner) = root.audio_edit() {
                inner.validate(root, &source.root.tempo, source.asset.playback_frames())?;
                if inner.placement.is_some() || inner.content_view.is_some() {
                    let view = AudioViewProjection::new(root, &source.root.tempo)?;
                    if view.occurrence_count() > 8_192 {
                        return Err(Invalid("grouped source exceeds bounded placement work"));
                    }
                    let mut frames = root.duration();
                    for index in 0..view.occurrence_count() {
                        if index == 0 || matches!(inner.domain.clock, AudioDomainClock::Beats(_)) {
                            frames = frames
                                .checked_add(view.occurrence(index)?.root_frame_length)
                                .ok_or(Invalid("grouped source preparation work overflow"))?;
                        }
                    }
                    frames
                } else {
                    root.duration()
                        .checked_add(inner.projected_domain(&source.root.tempo)?.1)
                        .ok_or(Invalid("grouped source preparation work overflow"))?
                }
            } else {
                root.duration()
            };
            work = work
                .checked_add(root_work)
                .filter(|work| *work <= MAX_AUDIO_DOMAIN_FRAMES)
                .ok_or(Invalid(
                    "grouped audio exceeds bounded aggregate source preparation work",
                ))?;
        }
        for index in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            super::view::validate_copy(clip, projection.occurrence(index)?)?;
        }
        // The existing key matcher checks the anchor source. Every remaining
        // source's immutable PCM and conversion identity participates explicitly.
        // Group output is frozen in frames; translating its destination does
        // not change the captured source program or require another PCM copy.
        if let Some(audio) = self.lookup_key(anchor.audio, anchor.asset, |key| {
            matches!(key, EntryKey::Group { root_only: old_root_only, sparse, edit: old, duration, sources: old_sources }
                if *old_root_only == root_only && *sparse == packing.is_some() && same_group_program(old, edit, root_only)
                && (root_only || *duration == clip.duration())
                && old_sources.len() == sources.len()
                && old_sources.iter().zip(sources).all(|((pcm, conversion, rate), source)|
                    pcm.shares_pcm_with(source.audio) && pcm.sample_rate() == source.audio.sample_rate()
                    && *conversion == source.asset.conversion_version()
                    && *rate == source.asset.source_sample_rate()))
        })? { return Ok(audio); }
        let bytes_for = |length: u64| {
            usize::try_from(length)
                .ok()
                .and_then(|length| length.checked_mul(std::mem::size_of::<crate::Frame>()))
                .ok_or(Invalid("grouped audio byte count overflow"))
        };
        let root_bytes = bytes_for(root_length)?;
        let prepared_bytes = if root_only {
            root_bytes
        } else {
            bytes_for(clip.duration())?
        };
        // Child display reads share the preparation work ceiling and the
        // separate peak budget. A refusal abandons this optional table.
        let peak_budget = preview_peaks::event_source_budget();
        let mut event_sources = if self.entry_limit > 0
            && prepared_bytes <= self.cache_limit
            && events
                .sources
                .iter()
                .try_fold(work, |used, child| {
                    used.checked_add(child.clip.duration())
                        .filter(|total| *total <= MAX_AUDIO_DOMAIN_FRAMES)
                })
                .is_some()
        {
            PreparedGroupEventSources::admit(events.sources.len(), peak_budget)
        } else {
            None
        };
        let root_charge = self.reserve_with_eviction(root_bytes)?;
        let output_reservation = if root_only {
            None
        } else {
            Some(self.reserve_with_eviction(prepared_bytes)?)
        };
        let mut root_pcm = Vec::new();
        root_pcm
            .try_reserve_exact(root_bytes / std::mem::size_of::<crate::Frame>())
            .map_err(|_| Invalid("grouped audio allocation failed"))?;
        root_pcm.resize(root_bytes / std::mem::size_of::<crate::Frame>(), [0.0; 2]);
        // Dependencies are collected in depth-first order. Advance once across
        // sibling subtrees instead of repeatedly scanning from the beginning.
        let mut source_index = 0usize;
        for root in &events.sources {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let offset = sources[source_index..]
                .iter()
                .position(|source| std::ptr::eq(source.root, root))
                .ok_or(Invalid("grouped audio dependency missing"))?;
            source_index += offset;
            let source = &sources[source_index];
            let child_start = source_index + 1;
            let child_end = child_start
                .checked_add(root.clip.retained_audio_roots().count())
                .filter(|end| *end <= sources.len())
                .ok_or(Invalid("grouped audio dependency tree is incomplete"))?;
            source_index = child_end;
            let compiled = if root.clip.has_retained_audio_roots() {
                Some(self.compile_group(&root.clip, &sources[child_start..child_end])?)
            } else {
                self.compile_clip_at_tempo(
                    &root.tempo,
                    &root.clip,
                    source.asset,
                    PreparedAudioSourceRef::Resident(source.audio),
                )?
            };
            if let Some(table) = &mut event_sources {
                let peaks = usize::try_from(root.clip.duration())
                    .ok()
                    .and_then(|frames| {
                        PreparedEventSourcePeaks::capture_with(
                            frames,
                            None,
                            peak_budget,
                            |index| {
                                compiled.as_ref().map_or(Some([0.0; 2]), |clip| {
                                    child_sample(clip, index as u64).ok()
                                })
                            },
                            || self.cancelled(),
                        )
                    });
                if let Some(peaks) = peaks {
                    table.push(root.id, peaks);
                } else {
                    event_sources = None;
                }
            }
            for event in events
                .events
                .iter()
                .filter(|event| event.active && event.source == source.root.id)
            {
                let start = event.start.max(root_window.start);
                let end = event.end.min(root_window.end);
                if start >= end {
                    continue;
                }
                let source_start = event.source_start + (start - event.start);
                let packed_start = if let Some(map) = packing {
                    map.packed_index(start as usize)
                        .ok_or(Invalid("grouped audio sparse event is missing"))?
                } else {
                    (start - root_window.start) as usize
                };
                let destination = root_pcm
                    .get_mut(packed_start..packed_start + (end - start) as usize)
                    .ok_or(Invalid("grouped audio event exceeds root"))?;
                let event_gain = event.gain.linear();
                for (offset, frame) in destination.iter_mut().enumerate() {
                    if offset % 4096 == 0 && self.cancelled() {
                        return Err(Invalid("audio preparation cancelled"));
                    }
                    if let Some(compiled) = compiled.as_ref() {
                        let child_frame = source_start
                            .checked_add(offset as u64)
                            .ok_or(Invalid("grouped audio source position overflow"))?;
                        let mut sample = child_sample(compiled, child_frame)?;
                        if let Some(fade) = event.fade {
                            let at = child_frame
                                .checked_sub(fade.source_start())
                                .filter(|at| *at < fade.shape().span())
                                .ok_or(Invalid("grouped event sample exceeds its fade domain"))?;
                            let gain = fade.shape().gain_at(at);
                            sample[0] *= gain;
                            sample[1] *= gain;
                        }
                        if event_gain != 1.0 {
                            sample[0] *= event_gain;
                            sample[1] *= event_gain;
                        }
                        *frame = sample;
                    }
                }
            }
        }
        if let Some(map) = packing {
            let mut result = Ok(());
            map.visit(
                root_window.start as usize..root_window.end as usize,
                |root, packed| {
                    if result.is_ok() {
                        result = mapping::apply_fade(
                            &mut root_pcm[packed],
                            root.start as u64,
                            edit,
                            tempo,
                            &self.shutdown,
                        );
                    }
                },
            );
            result?;
        } else if root_only || edit.content_view.is_none() {
            mapping::apply_fade(
                &mut root_pcm,
                root_window.start,
                edit,
                tempo,
                &self.shutdown,
            )?;
        }
        let audio = if let Some(reservation) = output_reservation {
            let mut output = Vec::new();
            output
                .try_reserve_exact(clip.duration() as usize)
                .map_err(|_| Invalid("grouped audio placement allocation failed"))?;
            output.resize(clip.duration() as usize, [0.0; 2]);
            if edit.content_view.is_some() {
                self.copy_frame_content(clip, &projection, &root_pcm, &mut output)?;
                self.apply_content_fade(tempo, clip, edit, &projection, &mut output)?;
            } else {
                for index in 0..projection.occurrence_count() {
                    if self.cancelled() {
                        return Err(Invalid("audio preparation cancelled"));
                    }
                    let occurrence = projection.occurrence(index)?;
                    let from = root_pcm
                        .get(
                            (occurrence.root_start - root_window.start) as usize
                                ..(occurrence.root_end - root_window.start) as usize,
                        )
                        .ok_or(Invalid("grouped audio crop exceeds root"))?;
                    let start = (occurrence.destination_start - clip.start()) as usize;
                    let to = output
                        .get_mut(start..start + from.len())
                        .ok_or(Invalid("grouped audio placement exceeds output"))?;
                    to.copy_from_slice(from);
                }
            }
            drop(root_pcm);
            drop(root_charge);
            Audio::from_derived(anchor.audio.sample_rate(), output, reservation)?
        } else {
            Audio::from_derived(anchor.audio.sample_rate(), root_pcm, root_charge)?
        };
        let mut identities = Vec::new();
        identities
            .try_reserve_exact(sources.len())
            .map_err(|_| Invalid("grouped audio source identity allocation failed"))?;
        for source in sources {
            identities.push((
                source.audio.clone(),
                source.asset.conversion_version(),
                source.asset.source_sample_rate(),
            ));
        }
        self.retain(Entry {
            source: anchor.audio.clone(),
            key: EntryKey::Group {
                root_only,
                sparse: packing.is_some(),
                edit: edit.clone(),
                duration: clip.duration(),
                sources: identities,
            },
            algorithm_version: hex_warp::ALGORITHM_VERSION,
            conversion: anchor.asset.conversion_version(),
            source_rate: anchor.asset.source_sample_rate(),
            prepared: audio.clone(),
            event_source_peaks: None,
            event_source_occurrences: None,
            group_event_sources: event_sources.map(PreparedGroupEventSources::finish),
            bytes: std::mem::size_of_val(audio.frames()),
        })?;
        Ok(audio)
    }
}

fn child_sample(compiled: &Clip, child_frame: u64) -> Result<crate::Frame, Invalid> {
    let position = compiled
        .start
        .checked_add(child_frame)
        .ok_or(Invalid("grouped audio source position overflow"))?;
    let metadata = compiled.metadata.as_deref();
    let mut sample = compiled.at(position, metadata.map_or(0, |m| m.source_phase));
    if let Some(fade) = metadata.and_then(|m| m.fade.as_ref()) {
        let gain = fade.gain_at(child_frame);
        sample[0] *= gain;
        sample[1] *= gain;
    }
    let gain = metadata.map_or(1.0, |m| m.gain);
    if gain != 1.0 {
        sample[0] *= gain;
        sample[1] *= gain;
    }
    Ok(sample)
}

fn group_tempo(edit: &AudioClipEdit) -> Result<&hex_project::TempoMap, Invalid> {
    edit.domain
        .events
        .as_ref()
        .and_then(|events| events.sources.first())
        .map(|root| root.tempo.as_ref())
        .ok_or(Invalid("grouped audio source missing"))
}

/// The composite root depends on the frame domain (events, sources, fade) and its
/// processing clock, never on the finite crop or placement read from it. Outer
/// output additionally depends on that crop/placement and the clip duration.
fn same_group_program(old: &AudioClipEdit, edit: &AudioClipEdit, root_only: bool) -> bool {
    if !root_only {
        return old == edit;
    }
    (Arc::ptr_eq(&old.domain, &edit.domain) || old.domain == edit.domain)
        && old.timing == edit.timing
        && old.processing_projection == edit.processing_projection
}
