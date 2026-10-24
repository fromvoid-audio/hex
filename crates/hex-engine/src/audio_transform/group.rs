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

impl AudioTransformPreparation {
    pub(crate) fn compile_group(
        &self,
        clip: &hex_project::Clip,
        sources: &[GroupSource<'_>],
    ) -> Result<Clip, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let edit = clip
            .audio_edit()
            .ok_or(Invalid("grouped audio edit missing"))?;
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
            || sources.len() != events.sources.len()
            || events.events.iter().any(|event| event.source == 0)
        {
            return Err(Invalid(
                "grouped audio supports frame edits, fades, gain and finite placement; outer source processing is unsupported",
            ));
        }
        // A grouped root is frozen in frames, so tempo only serves the finite
        // placement projection. Its frame clock is independent of this map.
        let tempo = &sources[0].root.tempo;
        let projection = AudioViewProjection::new(clip, tempo)?;
        if clip.duration() > MAX_AUDIO_DOMAIN_FRAMES || projection.occurrence_count() > 8_192 {
            return Err(Invalid("grouped audio exceeds bounded placement work"));
        }
        let mut work = edit.domain.clock.length();
        for source in sources {
            if source
                .root
                .clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
                .is_some_and(|events| !events.sources.is_empty())
            {
                return Err(Invalid("nested grouped audio preparation is unsupported"));
            }
            let root = &source.root.clip;
            let root_work = if let Some(inner) = root.audio_edit() {
                inner.validate(root, &source.root.tempo, source.asset.playback_frames())?;
                if inner.placement.is_some() {
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
        let anchor = &sources[0];
        if let Some(audio) = self.lookup_key(anchor.audio, anchor.asset, |key| {
            matches!(key, EntryKey::Group { edit: old, start, duration, sources: old_sources }
                if old == edit && *start == clip.start() && *duration == clip.duration()
                && old_sources.len() == sources.len()
                && old_sources.iter().zip(sources).all(|((pcm, conversion, rate), source)|
                    pcm.shares_pcm_with(source.audio) && pcm.sample_rate() == source.audio.sample_rate()
                    && *conversion == source.asset.conversion_version()
                    && *rate == source.asset.source_sample_rate()))
        })? { return super::view::placed_clip(audio, clip); }
        let bytes_for = |length: u64| {
            usize::try_from(length)
                .ok()
                .and_then(|length| length.checked_mul(std::mem::size_of::<crate::Frame>()))
                .ok_or(Invalid("grouped audio byte count overflow"))
        };
        let root_bytes = bytes_for(edit.domain.clock.length())?;
        let root_charge = self.reserve_with_eviction(root_bytes)?;
        let reservation = self.reserve_with_eviction(bytes_for(clip.duration())?)?;
        let mut root_pcm = Vec::new();
        root_pcm
            .try_reserve_exact(root_bytes / std::mem::size_of::<crate::Frame>())
            .map_err(|_| Invalid("grouped audio allocation failed"))?;
        root_pcm.resize(root_bytes / std::mem::size_of::<crate::Frame>(), [0.0; 2]);
        for source in sources {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let compiled = self.compile_clip_at_tempo(
                &source.root.tempo,
                &source.root.clip,
                source.asset,
                PreparedAudioSourceRef::Resident(source.audio),
            )?;
            for event in events
                .events
                .iter()
                .filter(|event| event.source == source.root.id)
            {
                let destination = root_pcm
                    .get_mut(event.start as usize..event.end as usize)
                    .ok_or(Invalid("grouped audio event exceeds root"))?;
                for (offset, frame) in destination.iter_mut().enumerate() {
                    if offset % 4096 == 0 && self.cancelled() {
                        return Err(Invalid("audio preparation cancelled"));
                    }
                    if let Some(compiled) = compiled.as_ref() {
                        let position = compiled
                            .start
                            .checked_add(event.source_start)
                            .and_then(|start| start.checked_add(offset as u64))
                            .ok_or(Invalid("grouped audio source position overflow"))?;
                        let metadata = compiled.metadata.as_deref();
                        let mut sample =
                            compiled.at(position, metadata.map_or(0, |m| m.source_phase));
                        if let Some(fade) = metadata.and_then(|m| m.fade.as_ref()) {
                            let gain = fade.gain_at(position - compiled.start);
                            sample[0] *= gain;
                            sample[1] *= gain;
                        }
                        let gain = metadata.map_or(1.0, |m| m.gain);
                        if gain != 1.0 {
                            sample[0] *= gain;
                            sample[1] *= gain;
                        }
                        *frame = sample;
                    }
                }
            }
        }
        mapping::apply_fade(&mut root_pcm, edit, tempo, &self.shutdown)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(clip.duration() as usize)
            .map_err(|_| Invalid("grouped audio placement allocation failed"))?;
        output.resize(clip.duration() as usize, [0.0; 2]);
        for index in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(index)?;
            let from = root_pcm
                .get(occurrence.root_start as usize..occurrence.root_end as usize)
                .ok_or(Invalid("grouped audio crop exceeds root"))?;
            let start = (occurrence.destination_start - clip.start()) as usize;
            let to = output
                .get_mut(start..start + from.len())
                .ok_or(Invalid("grouped audio placement exceeds output"))?;
            to.copy_from_slice(from);
        }
        drop(root_pcm);
        drop(root_charge);
        let audio = Audio::from_derived(anchor.audio.sample_rate(), output, reservation)?;
        let mut identities = Vec::new();
        identities
            .try_reserve_exact(sources.len())
            .map_err(|_| Invalid("grouped audio source identity allocation failed"))?;
        let mut source_bytes = 0usize;
        for source in sources {
            source_bytes = source_bytes
                .checked_add(std::mem::size_of_val(source.audio.frames()))
                .ok_or(Invalid("grouped audio source byte count overflow"))?;
            identities.push((
                source.audio.clone(),
                source.asset.conversion_version(),
                source.asset.source_sample_rate(),
            ));
        }
        self.retain(Entry {
            source: anchor.audio.clone(),
            key: EntryKey::Group {
                edit: edit.clone(),
                start: clip.start(),
                duration: clip.duration(),
                sources: identities,
            },
            algorithm_version: hex_warp::ALGORITHM_VERSION,
            conversion: anchor.asset.conversion_version(),
            source_rate: anchor.asset.source_sample_rate(),
            prepared: audio.clone(),
            bytes: bytes_for(clip.duration())?,
            source_bytes,
        })?;
        super::view::placed_clip(audio, clip)
    }
}
