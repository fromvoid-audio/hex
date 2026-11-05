//! Bounded control-only directed content preparation. Immutable base placement
//! is projected canonically; only admitted output PCM reaches the renderer and
//! waveform owner. Processing precedes reflection; destination fades follow it.
use super::*;
use hex_project::{AudioDomainClock, AudioViewProjection, MAX_AUDIO_DOMAIN_FRAMES};
#[path = "content_map.rs"]
mod read_map;
const MAX_CONTENT_OCCURRENCES: u64 = 8_192;
impl AudioTransformPreparation {
    pub(super) fn compile_content(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
    ) -> Result<Clip, Invalid> {
        edit.content_view
            .ok_or(Invalid("directed audio view missing"))?;
        let projection = AudioViewProjection::new(clip, tempo_map)?;
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        if clip.duration() > MAX_AUDIO_DOMAIN_FRAMES
            || projection.occurrence_count() > MAX_CONTENT_OCCURRENCES
        {
            return Err(Invalid(
                "directed audio exceeds bounded preparation frames or occurrences",
            ));
        }
        let PreparedAudioSourceRef::Resident(source) = source else {
            return Err(Invalid(
                "directed audio preparation currently requires resident media; source changes were not committed",
            ));
        };
        let tempo = self.shared_tempo(tempo_map)?;
        if let Some(audio) = self.lookup_key(source, asset, |key| {
            key.matches_view(edit, tempo_map, clip.start(), clip.duration())
        })? {
            return super::view::placed_clip(audio, clip);
        }
        let musical = matches!(edit.domain.clock, AudioDomainClock::Beats(_));
        let map_bytes = (2 * hex_project::MAX_TEMPO_POINTS + 2)
            .checked_mul(std::mem::size_of::<u64>() + std::mem::size_of::<hex_warp::MapSegment>())
            .ok_or(Invalid("directed map metadata byte overflow"))?;
        let _map_admission = if musical {
            Some(self.reserve_with_eviction(map_bytes)?)
        } else {
            None
        };
        if musical {
            read_map::base_bounds(&projection, edit)?;
        }
        let admitted_projection = if musical {
            projection.full_content_projection()?.0
        } else {
            projection
        };
        let mut segments = 0usize;
        let mut filter_work = 0u64;
        let mut work = 0_u64;
        for i in 0..admitted_projection.occurrence_count() {
            let occurrence = admitted_projection.occurrence(i)?;
            if musical
                && let Some(span) = read_map::prepare(&admitted_projection, edit, occurrence)?
            {
                segments = segments
                    .checked_add(span.map.segments().len())
                    .filter(|n| *n <= hex_warp::MAX_MAP_SEGMENTS)
                    .ok_or(Invalid(
                        "directed audio exceeds 8192 aggregate tempo segments",
                    ))?;
                let render = (span.last - span.first)
                    .div_ceil(1024)
                    .checked_mul(1024 * 64)
                    .and_then(|n| {
                        n.checked_add((span.map.segments().len() as u64).checked_mul(64 * 256)?)
                    })
                    .ok_or(Invalid("directed filter work overflow"))?;
                filter_work = filter_work
                    .checked_add(render)
                    .filter(|n| *n <= hex_warp::MAX_WORK_UNITS)
                    .ok_or(Invalid("directed audio exceeds bounded filter work"))?;
            }
            if i == 0 || matches!(edit.domain.clock, AudioDomainClock::Beats(_)) {
                work = work
                    .checked_add(occurrence.root_frame_length)
                    .filter(|n| *n <= MAX_AUDIO_DOMAIN_FRAMES)
                    .ok_or(Invalid(
                        "directed audio exceeds bounded full-root preparation work",
                    ))?;
            }
        }
        let length = usize::try_from(clip.duration())
            .map_err(|_| Invalid("directed audio size overflow"))?;
        let bytes = length
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("directed audio byte count overflow"))?;
        let reservation = self.reserve_with_eviction(bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(length)
            .map_err(|_| Invalid("directed audio allocation failed"))?;
        frames.resize(length, [0.0; 2]);
        if musical {
            self.render_musical_content(tempo_map, clip, asset, source, &mut frames)?;
        } else {
            // Only the transient pre-fade processing program is neutralized. Saved
            // fade/source settings and source PCM are never rewritten.
            let mut unfaded = edit.clone();
            Arc::make_mut(&mut unfaded.domain).fade =
                hex_project::ClipFade::neutral(edit.domain.fade.span())
                    .map_err(|e| Invalid(e.0))?;
            if projection.occurrence_count() != 0 {
                let occurrence = projection.occurrence(0)?;
                let root_edit = super::view::occurrence_edit(
                    &unfaded,
                    occurrence.root_origin,
                    self,
                    tempo_map,
                )?;
                let root = self.prepare_audio_at_tempo(tempo_map, &root_edit, asset, source)?;
                self.copy_frame_content(clip, &projection, root.frames(), &mut frames)?;
            }
        }
        self.apply_content_fade(tempo_map, clip, edit, &projection, &mut frames)?;
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let audio = Audio::from_derived(source.sample_rate(), frames, reservation)?;
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
            source_bytes: std::mem::size_of_val(source.frames()),
        })?;
        super::view::placed_clip(audio, clip)
    }
    /// Reflect admitted frame-root PCM through the canonical directed view.
    /// Shared by ordinary and composite roots; no intermediate PCM is allocated.
    pub(super) fn copy_frame_content(
        &self,
        clip: &hex_project::Clip,
        projection: &AudioViewProjection<'_>,
        root: &[crate::Frame],
        frames: &mut [crate::Frame],
    ) -> Result<(), Invalid> {
        let view = clip
            .audio_edit()
            .and_then(|edit| edit.content_view)
            .ok_or(Invalid("directed audio view missing"))?;
        for i in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(i)?;
            let begin = occurrence.elapsed_start.max(view.window.start);
            let end = occurrence.elapsed_end.min(view.window.end);
            if begin >= end {
                continue;
            }
            let (a, b) = if view.window.reversed {
                (view.window.end - end, view.window.end - begin)
            } else {
                (begin - view.window.start, end - view.window.start)
            };
            let first = projection.frame_at_elapsed(a)?.max(clip.start());
            let last = projection.frame_at_elapsed(b)?.min(
                clip.start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("audio end overflow"))?,
            );
            if first >= last {
                continue;
            }
            if root.len() as u64 != occurrence.root_frame_length {
                return Err(Invalid("directed root differs from admitted pcm"));
            }

            for frame in first..last {
                if (frame - first) % 1_024 == 0 && self.cancelled() {
                    return Err(Invalid("audio preparation cancelled"));
                }
                let elapsed = projection.elapsed_at_frame(frame)?;
                let read = view.window.read_position(elapsed)?;
                let base = projection.base_frame_at_elapsed(read)?;
                let sample = occurrence
                    .root_start
                    .checked_add(base.saturating_sub(occurrence.destination_start))
                    .ok_or(Invalid("directed root sample overflow"))?;
                if sample >= occurrence.root_end {
                    return Err(Invalid("directed sample exceeds admitted crop"));
                }
                let value = *root
                    .get(sample as usize)
                    .ok_or(Invalid("directed sample exceeds admitted root"))?;
                *frames
                    .get_mut((frame - clip.start()) as usize)
                    .ok_or(Invalid("directed destination exceeds admitted pcm"))? = value;
            }
        }
        Ok(())
    }

    /// Destination fades retain their forward clock after content reflection.
    pub(super) fn apply_content_fade(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        edit: &AudioClipEdit,
        projection: &AudioViewProjection<'_>,
        frames: &mut [crate::Frame],
    ) -> Result<(), Invalid> {
        let view = edit
            .content_view
            .ok_or(Invalid("directed audio view missing"))?;
        if !edit.domain.fade.is_neutral() {
            // A fade keeps its original forward container clock. Iterate base
            // occurrences once; no fade buffer or callback work is introduced.
            let consumed = view
                .base_start_delta
                .checked_neg()
                .ok_or(Invalid("audio content fade displacement overflow"))?;
            let mut covered = 0usize;
            for i in 0..projection.occurrence_count() {
                let occurrence = projection.occurrence(i)?;
                let a = (i128::from(occurrence.elapsed_start) - consumed).max(0);
                let b = (i128::from(occurrence.elapsed_end) - consumed)
                    .min(i128::from(view.window.length()?));
                if a >= b {
                    continue;
                }
                let first = projection.frame_at_elapsed(a as u64)?.max(clip.start());
                let last = projection
                    .frame_at_elapsed(b as u64)?
                    .min(clip.start() + clip.duration());
                let start = (first - clip.start()) as usize;
                let end = (last - clip.start()) as usize;
                for f in frames
                    .get_mut(covered..start)
                    .ok_or(Invalid("directed fade coverage overflow"))?
                {
                    *f = [0.0; 2];
                }
                let root_edit =
                    super::view::occurrence_edit(edit, occurrence.root_origin, self, tempo_map)?;
                let fade = root_edit
                    .projected_fade(tempo_map)
                    .map_err(|e| Invalid(e.0))?;
                for frame in first..last {
                    let elapsed =
                        u64::try_from(consumed + i128::from(projection.elapsed_at_frame(frame)?))
                            .map_err(|_| Invalid("directed fade clock overflow"))?;
                    let base = projection.base_frame_at_elapsed(elapsed)?;
                    let sample = occurrence
                        .root_start
                        .checked_add(base.saturating_sub(occurrence.destination_start))
                        .ok_or(Invalid("directed fade sample overflow"))?
                        .clamp(occurrence.root_start, occurrence.root_end - 1);
                    let gain = fade.gain_at(sample).map_err(|e| Invalid(e.0))?;
                    let f = frames
                        .get_mut((frame - clip.start()) as usize)
                        .ok_or(Invalid("directed fade destination overflow"))?;
                    f[0] *= gain;
                    f[1] *= gain;
                }
                covered = end;
            }
            for f in frames
                .get_mut(covered..)
                .ok_or(Invalid("directed fade coverage overflow"))?
            {
                *f = [0.0; 2];
            }
        }
        Ok(())
    }

    /// Assemble the actual finite outer content once. Adjacent occurrence PCM
    /// supplies sinc context across internal Repeat/crop/tempo seams; only the
    /// true finite base edges use the kernel's zero-padding policy. Always render
    /// the complete directed map before cropping its integer output. Rebuilding
    /// interpolation/filter rates at a crop boundary would alter retained PCM.
    /// Whole windows render directly; cropped windows charge one bounded full
    /// output scratch buffer to the existing control-side preparation budget.
    fn render_musical_content(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        asset: &Asset,
        source: &Audio,
        frames: &mut [crate::Frame],
    ) -> Result<(), Invalid> {
        let edit = clip
            .audio_edit()
            .ok_or(Invalid("directed audio edit missing"))?;
        let (projection, crop) =
            AudioViewProjection::new(clip, tempo_map)?.full_content_projection()?;
        let (output_start, output_end) = projection.outer_frame_bounds();
        let output_len = usize::try_from(output_end - output_start)
            .map_err(|_| Invalid("directed full output size overflow"))?;
        let cropped = crop.start != 0 || crop.end != output_len as u64;
        let output_bytes = output_len
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("directed full output bytes overflow"))?;
        let _output_reservation = if cropped {
            Some(self.reserve_with_eviction(output_bytes)?)
        } else {
            None
        };
        let mut scratch = Vec::new();
        if cropped {
            scratch
                .try_reserve_exact(output_len)
                .map_err(|_| Invalid("directed full output allocation failed"))?;
            scratch.resize(output_len, [0.0; 2]);
        }
        let output = if cropped {
            scratch.as_mut_slice()
        } else {
            &mut *frames
        };

        let (base_start, base_end) = read_map::base_bounds(&projection, edit)?;
        let length = usize::try_from(base_end - base_start)
            .map_err(|_| Invalid("directed filter context size overflow"))?;
        let bytes = length
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("directed filter context byte overflow"))?;
        let reservation = self.reserve_with_eviction(bytes)?;
        let mut base = Vec::new();
        base.try_reserve_exact(length)
            .map_err(|_| Invalid("directed filter context allocation failed"))?;
        base.resize(length, [0.0; 2]);
        let mut unfaded = edit.clone();
        Arc::make_mut(&mut unfaded.domain).fade =
            hex_project::ClipFade::neutral(edit.domain.fade.span()).map_err(|e| Invalid(e.0))?;
        for i in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let occurrence = projection.occurrence(i)?;
            let root_edit =
                super::view::occurrence_edit(&unfaded, occurrence.root_origin, self, tempo_map)?;
            let root = self.prepare_audio_at_tempo(tempo_map, &root_edit, asset, source)?;
            if root.frames().len() as u64 != occurrence.root_frame_length {
                return Err(Invalid("directed root differs from admitted pcm"));
            }
            let input = root
                .frames()
                .get(occurrence.root_start as usize..occurrence.root_end as usize)
                .ok_or(Invalid("directed base crop exceeds prepared root"))?;
            let start = occurrence
                .destination_start
                .checked_sub(base_start)
                .ok_or(Invalid("directed base precedes admitted context"))?
                as usize;
            let end = occurrence
                .destination_end
                .checked_sub(base_start)
                .ok_or(Invalid("directed base end precedes context"))?
                as usize;
            let output = base
                .get_mut(start..end)
                .ok_or(Invalid("directed base exceeds admitted context"))?;
            if input.len() != output.len() {
                return Err(Invalid(
                    "directed base requires unsupported processing rebase",
                ));
            }
            output.copy_from_slice(input);
        }
        if projection
            .content_window()
            .ok_or(Invalid("directed audio view missing"))?
            .reversed
        {
            for i in 0..base.len() / 2 {
                if i % 1024 == 0 && self.cancelled() {
                    return Err(Invalid("audio preparation cancelled"));
                }
                let last = base.len() - 1 - i;
                base.swap(i, last);
            }
        }
        let base = Audio::from_derived(source.sample_rate(), base, reservation)?;
        let law = super::warp_law(
            edit.content_view
                .ok_or(Invalid("directed audio view missing"))?
                .read_resampler,
        );
        for i in 0..projection.occurrence_count() {
            if self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            if let Some(span) = read_map::prepare(&projection, edit, projection.occurrence(i)?)? {
                let destination = output
                    .get_mut(
                        (span.first - output_start) as usize..(span.last - output_start) as usize,
                    )
                    .ok_or(Invalid(
                        "directed filtered destination exceeds admitted pcm",
                    ))?;
                self.render_content_span(&base, &span.map, law, destination)?;
            }
        }
        if cropped {
            let retained = scratch
                .get(crop.start as usize..crop.end as usize)
                .ok_or(Invalid("directed crop exceeds prepared output"))?;
            if retained.len() != frames.len() {
                return Err(Invalid("directed crop differs from destination"));
            }
            frames.copy_from_slice(retained);
        }
        Ok(())
    }
    fn render_content_span(
        &self,
        root: &Audio,
        map: &hex_warp::TimeMap,
        law: hex_warp::Resampler,
        destination: &mut [crate::Frame],
    ) -> Result<(), Invalid> {
        const Q32: i128 = 1_i128 << 32;
        let integral = map.segments().iter().all(|s| {
            s.source_start_q32 % Q32 == 0
                && s.source_end_q32 - s.source_start_q32
                    == i128::from(s.output_end - s.output_start) * Q32
        });
        if integral {
            let first = map
                .segments()
                .first()
                .ok_or(Invalid("directed map is empty"))?
                .source_start_q32
                / Q32;
            let first = usize::try_from(first)
                .map_err(|_| Invalid("directed integer source precedes root"))?;
            for (i, frame) in destination.iter_mut().enumerate() {
                let index = first
                    .checked_add(i)
                    .ok_or(Invalid("directed integer sample overflow"))?;
                *frame = *root
                    .frames()
                    .get(index)
                    .ok_or(Invalid("directed integer sample exceeds root"))?;
            }
            return Ok(());
        }
        // RePitch's owning kernel admits at most MAX_SCRATCH_BYTES; retain
        // this conservative allowance together with its temporary output.
        let temporary = std::mem::size_of_val(destination)
            .checked_add(hex_warp::MAX_SCRATCH_BYTES)
            .ok_or(Invalid("directed filter scratch byte overflow"))?;
        let _render = self.reserve_with_eviction(temporary)?;
        let input = root.frames();
        let request = hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: input,
                sample_rate: root.sample_rate(),
                region_start: 0,
                region_length: input.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map,
            output_frames: destination.len(),
            pitch_cents: 0,
            mode: hex_warp::Mode::RePitch,
        };
        // The view's own read law, independent of the root content's law: saved
        // musical views migrate to the frozen v1 reconstruction, new ones use v2.
        let rendered = hex_warp::render_with_resampler(&request, law, &mut || self.cancelled())
            .map_err(warp_error)?;
        if rendered.frames.len() != destination.len() {
            return Err(Invalid(
                "directed filtered output differs from admitted span",
            ));
        }
        destination.copy_from_slice(&rendered.frames);
        Ok(())
    }
}
