//! Control-owned immutable audio transformation, cache and retained PCM accounting.
mod budget;
mod events;
mod mapping;
use crate::{Audio, Clip, Invalid, compile::PreparedAudioSourceRef};
pub(crate) use budget::Reservation;
use hex_project::{
    Asset, AudioClipDomain, AudioClipEdit, BeatsTail, Project as CanonicalProject, WarpMode,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub const MAX_CACHED_DERIVED_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_CACHED_TRANSFORMS: usize = 256;
/// Conservative full-original PCM charge, including pending cache entries.
pub const MAX_CACHED_SOURCE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_RETAINED_DERIVED_BYTES: usize = budget::RETAINED_LIMIT;

struct Entry {
    source: Audio,
    domain: Arc<AudioClipDomain>,
    tempo: Option<Arc<hex_project::TempoMap>>,
    origin: i128,
    algorithm_version: u32,
    conversion: u32,
    source_rate: u32,
    prepared: Audio,
    bytes: usize,
    source_bytes: usize,
}
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
    source_bytes: usize,
}
impl Cache {
    fn evict_oldest(&mut self) -> Result<(), Invalid> {
        let old = self
            .entries
            .pop_front()
            .ok_or(Invalid("audio cache accounting is inconsistent"))?;
        self.bytes = self
            .bytes
            .checked_sub(old.bytes)
            .ok_or(Invalid("audio cache byte count underflow"))?;
        self.source_bytes = self
            .source_bytes
            .checked_sub(old.source_bytes)
            .ok_or(Invalid("audio cache source count underflow"))?;
        Ok(())
    }
}
/// One control owner's bounded cache. Never construct, query, reserve or destroy
/// this owner in the audio callback. Derived buffers retain their own budget token.
pub struct AudioTransformPreparation {
    cache: RefCell<Cache>,
    pending: RefCell<Option<Vec<Entry>>>,
    tempo: RefCell<Option<Arc<hex_project::TempoMap>>>,
    shutdown: Arc<AtomicBool>,
    budget: Arc<budget::Budget>,
    cache_limit: usize,
    source_limit: usize,
    entry_limit: usize,
    hits: Cell<u64>,
    #[cfg(test)]
    cancel_after_polls: Cell<Option<usize>>,
}
impl Default for AudioTransformPreparation {
    fn default() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }
}
impl AudioTransformPreparation {
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self {
            cache: RefCell::new(Cache {
                entries: VecDeque::new(),
                bytes: 0,
                source_bytes: 0,
            }),
            pending: RefCell::new(None),
            tempo: RefCell::new(None),
            shutdown,
            budget: budget::production_budget(),
            cache_limit: MAX_CACHED_DERIVED_BYTES,
            source_limit: MAX_CACHED_SOURCE_BYTES,
            entry_limit: MAX_CACHED_TRANSFORMS,
            hits: Cell::new(0),
            #[cfg(test)]
            cancel_after_polls: Cell::new(None),
        }
    }
    fn cancelled(&self) -> bool {
        #[cfg(test)]
        if let Some(remaining) = self.cancel_after_polls.get() {
            if remaining == 0 {
                return true;
            }
            self.cancel_after_polls.set(Some(remaining - 1));
        }
        self.shutdown.load(Ordering::Acquire)
    }
    pub fn retained_pcm_bytes(&self) -> usize {
        self.budget.used()
    }
    pub fn cached_pcm_bytes(&self) -> Result<usize, Invalid> {
        Ok(self
            .cache
            .try_borrow()
            .map_err(|_| Invalid("audio transform cache is already borrowed"))?
            .bytes)
    }
    pub fn cache_hits(&self) -> u64 {
        self.hits.get()
    }
    pub fn transaction<T>(
        &self,
        prepare: impl FnOnce() -> Result<T, Invalid>,
    ) -> Result<T, Invalid> {
        // Reserve the finite metadata capacity before any candidate PCM exists.
        // Publication can then move entries without a late allocation failure.
        {
            let mut cache = self
                .cache
                .try_borrow_mut()
                .map_err(|_| Invalid("audio transform cache is already borrowed"))?;
            let extra = self.entry_limit.saturating_sub(cache.entries.len());
            cache
                .entries
                .try_reserve(extra)
                .map_err(|_| Invalid("audio cache metadata allocation failed"))?;
        }

        {
            let mut pending = self
                .pending
                .try_borrow_mut()
                .map_err(|_| Invalid("audio preparation is already borrowed"))?;
            if pending.is_some() {
                return Err(Invalid("audio preparation transaction is already active"));
            }
            *pending = Some(Vec::new());
        }
        let result = prepare();
        let entries = self
            .pending
            .try_borrow_mut()
            .map_err(|_| Invalid("audio preparation is already borrowed"))?
            .take()
            .ok_or(Invalid("audio preparation transaction is missing"))?;
        if result.is_ok() {
            for entry in entries {
                self.insert(entry)?;
            }
        }
        result
    }
    pub(crate) fn compile_transaction<T>(
        &self,
        prepare: impl FnOnce() -> Result<T, Invalid>,
    ) -> Result<T, Invalid> {
        let active = self
            .pending
            .try_borrow()
            .map_err(|_| Invalid("audio preparation is already borrowed"))?
            .is_some();
        if active {
            prepare()
        } else {
            self.transaction(prepare)
        }
    }
    fn shared_tempo(
        &self,
        map: &hex_project::TempoMap,
    ) -> Result<Arc<hex_project::TempoMap>, Invalid> {
        let mut retained = self
            .tempo
            .try_borrow_mut()
            .map_err(|_| Invalid("audio tempo context is already borrowed"))?;
        if let Some(old) = retained.as_ref()
            && old.as_ref() == map
        {
            return Ok(Arc::clone(old));
        }
        let result = Arc::new(map.clone());
        *retained = Some(Arc::clone(&result));
        Ok(result)
    }
    fn insert(&self, entry: Entry) -> Result<(), Invalid> {
        if entry.bytes > self.cache_limit
            || entry.source_bytes > self.source_limit
            || self.entry_limit == 0
        {
            return Ok(());
        }
        let mut cache = self
            .cache
            .try_borrow_mut()
            .map_err(|_| Invalid("audio transform cache is already borrowed"))?;
        while cache.entries.len() >= self.entry_limit
            || cache
                .bytes
                .checked_add(entry.bytes)
                .is_none_or(|bytes| bytes > self.cache_limit)
            || cache
                .source_bytes
                .checked_add(entry.source_bytes)
                .is_none_or(|bytes| bytes > self.source_limit)
        {
            cache.evict_oldest()?;
        }
        cache
            .entries
            .try_reserve(1)
            .map_err(|_| Invalid("audio cache metadata allocation failed"))?;
        cache.bytes = cache
            .bytes
            .checked_add(entry.bytes)
            .ok_or(Invalid("audio cache byte count overflow"))?;
        cache.source_bytes = cache
            .source_bytes
            .checked_add(entry.source_bytes)
            .ok_or(Invalid("audio source cache byte count overflow"))?;
        cache.entries.push_back(entry);
        Ok(())
    }
    fn retain(&self, entry: Entry) -> Result<(), Invalid> {
        if entry.bytes > self.cache_limit
            || entry.source_bytes > self.source_limit
            || self.entry_limit == 0
        {
            return Ok(());
        }
        let mut pending = self
            .pending
            .try_borrow_mut()
            .map_err(|_| Invalid("audio preparation transaction is already borrowed"))?;
        if let Some(entries) = pending.as_mut() {
            let sum = |entries: &[Entry]| {
                entries
                    .iter()
                    .try_fold((0_usize, 0_usize), |(pcm, source), entry| {
                        Ok::<_, Invalid>((
                            pcm.checked_add(entry.bytes)
                                .ok_or(Invalid("audio pending byte count overflow"))?,
                            source
                                .checked_add(entry.source_bytes)
                                .ok_or(Invalid("audio pending source count overflow"))?,
                        ))
                    })
            };
            let (mut bytes, mut sources) = sum(entries)?;
            while entries.len() >= self.entry_limit
                || bytes
                    .checked_add(entry.bytes)
                    .is_none_or(|bytes| bytes > self.cache_limit)
                || sources
                    .checked_add(entry.source_bytes)
                    .is_none_or(|bytes| bytes > self.source_limit)
            {
                if entries.is_empty() {
                    return Err(Invalid("audio pending cache accounting is inconsistent"));
                }
                let old = entries.remove(0);
                bytes -= old.bytes;
                sources -= old.source_bytes;
            }
            // Pending plus committed cache owners share both finite byte caps.
            let mut cache = self
                .cache
                .try_borrow_mut()
                .map_err(|_| Invalid("audio source cache is already borrowed"))?;
            while cache
                .source_bytes
                .checked_add(sources)
                .and_then(|bytes| bytes.checked_add(entry.source_bytes))
                .is_none_or(|bytes| bytes > self.source_limit)
                || cache
                    .bytes
                    .checked_add(bytes)
                    .and_then(|bytes| bytes.checked_add(entry.bytes))
                    .is_none_or(|bytes| bytes > self.cache_limit)
            {
                cache.evict_oldest()?;
            }
            drop(cache);
            entries
                .try_reserve(1)
                .map_err(|_| Invalid("audio pending-cache allocation failed"))?;
            entries.push(entry);
            Ok(())
        } else {
            drop(pending);
            self.insert(entry)
        }
    }
    fn lookup(
        &self,
        source: &Audio,
        edit: &AudioClipEdit,
        tempo: &Option<Arc<hex_project::TempoMap>>,
        origin: i128,
        asset: &Asset,
    ) -> Result<Option<Audio>, Invalid> {
        let matching = |entry: &Entry| {
            entry.algorithm_version == hex_warp::ALGORITHM_VERSION
                && entry.source.shares_pcm_with(source)
                && entry.source.sample_rate() == source.sample_rate()
                && (Arc::ptr_eq(&entry.domain, &edit.domain) || entry.domain == edit.domain)
                && entry.origin == origin
                && entry.conversion == asset.conversion_version()
                && entry.source_rate == asset.source_sample_rate()
                && entry.tempo.as_deref() == tempo.as_deref()
        };
        if let Some(entries) = self
            .pending
            .try_borrow()
            .map_err(|_| Invalid("audio pending cache is already borrowed"))?
            .as_ref()
            && let Some(entry) = entries.iter().find(|entry| matching(entry))
        {
            self.hits.set(self.hits.get().saturating_add(1));
            return Ok(Some(entry.prepared.clone()));
        }
        let mut cache = self
            .cache
            .try_borrow_mut()
            .map_err(|_| Invalid("audio transform cache is already borrowed"))?;
        let Some(index) = cache.entries.iter().position(matching) else {
            return Ok(None);
        };
        let entry = cache
            .entries
            .remove(index)
            .ok_or(Invalid("audio cache entry disappeared"))?;
        let audio = entry.prepared.clone();
        cache.entries.push_back(entry);
        self.hits.set(self.hits.get().saturating_add(1));
        Ok(Some(audio))
    }
    fn prepare_audio(
        &self,
        project: &CanonicalProject,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: &Audio,
    ) -> Result<Audio, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let (_, length, _, _) = edit.projected_domain(project.tempo_map())?;
        let tempo = if edit.musical_start.is_some() {
            Some(self.shared_tempo(project.tempo_map())?)
        } else {
            None
        };
        let origin = if edit.musical_start.is_some() {
            edit.projection_origin()?
        } else {
            0
        };
        if let Some(audio) = self.lookup(source, edit, &tempo, origin, asset)? {
            return Ok(audio);
        }
        let bytes = usize::try_from(length)
            .ok()
            .and_then(|length| length.checked_mul(std::mem::size_of::<crate::Frame>()))
            .ok_or(Invalid("derived audio byte count overflow"))?;
        // Bound pending+committed cache ownership BEFORE reserving candidate PCM.
        // These are only cache references: audible/candidate graph owners remain charged.
        let source_bytes = std::mem::size_of_val(source.frames());
        let cacheable =
            bytes <= self.cache_limit && source_bytes <= self.source_limit && self.entry_limit > 0;
        if cacheable {
            let mut pending = self
                .pending
                .try_borrow_mut()
                .map_err(|_| Invalid("audio pending cache is already borrowed"))?;
            let (mut pending_bytes, mut pending_sources) = (0_usize, 0_usize);
            if let Some(entries) = pending.as_ref() {
                for entry in entries {
                    pending_bytes = pending_bytes
                        .checked_add(entry.bytes)
                        .ok_or(Invalid("audio pending byte count overflow"))?;
                    pending_sources = pending_sources
                        .checked_add(entry.source_bytes)
                        .ok_or(Invalid("audio pending source count overflow"))?;
                }
            }
            if let Some(entries) = pending.as_mut() {
                while pending_bytes
                    .checked_add(bytes)
                    .is_none_or(|sum| sum > self.cache_limit)
                    || pending_sources
                        .checked_add(source_bytes)
                        .is_none_or(|sum| sum > self.source_limit)
                {
                    if entries.is_empty() {
                        return Err(Invalid("audio pending accounting is inconsistent"));
                    }
                    let old = entries.remove(0);
                    pending_bytes -= old.bytes;
                    pending_sources -= old.source_bytes;
                }
            }
            let mut cache = self
                .cache
                .try_borrow_mut()
                .map_err(|_| Invalid("audio transform cache is already borrowed"))?;
            while cache
                .bytes
                .checked_add(pending_bytes)
                .and_then(|n| n.checked_add(bytes))
                .is_none_or(|sum| sum > self.cache_limit)
                || cache
                    .source_bytes
                    .checked_add(pending_sources)
                    .and_then(|n| n.checked_add(source_bytes))
                    .is_none_or(|sum| sum > self.source_limit)
            {
                cache.evict_oldest()?;
            }
        }
        let reservation = loop {
            match self.budget.reserve(bytes) {
                Ok(reservation) => break reservation,
                Err(error) => {
                    let mut cache = self
                        .cache
                        .try_borrow_mut()
                        .map_err(|_| Invalid("audio transform cache is already borrowed"))?;
                    if !cache.entries.is_empty() {
                        cache.evict_oldest()?;
                        continue;
                    }
                    drop(cache);
                    let mut pending = self
                        .pending
                        .try_borrow_mut()
                        .map_err(|_| Invalid("audio pending cache is already borrowed"))?;
                    if let Some(entries) = pending.as_mut()
                        && !entries.is_empty()
                    {
                        entries.remove(0);
                        continue;
                    }
                    return Err(error);
                }
            }
        };
        let (map, attacks) = mapping::prepare_map(edit, project.tempo_map(), &self.shutdown)?;
        let mut envelope = Vec::new();
        envelope
            .try_reserve_exact(edit.domain.source_gain.len())
            .map_err(|_| Invalid("source envelope allocation failed"))?;
        envelope.extend(edit.domain.source_gain.iter().map(|p| hex_warp::GainPoint {
            source_frame: p.frame,
            gain_q16: p.gain_q16,
        }));
        let domain = &edit.domain;
        let mode = if domain.warp_enabled {
            match domain
                .warp
                .as_ref()
                .ok_or(Invalid("warp settings missing"))?
                .mode
            {
                WarpMode::RePitch => hex_warp::Mode::RePitch,
                WarpMode::Beats { tail, decay, .. } => hex_warp::Mode::Beats {
                    division_frames: None,
                    tail: match tail {
                        BeatsTail::Off => hex_warp::Tail::Off,
                        BeatsTail::Forward => hex_warp::Tail::Forward,
                        BeatsTail::Alternating => hex_warp::Tail::Alternating,
                    },
                    decay,
                },
                WarpMode::Tones { grain_ms } => hex_warp::Mode::Tones { grain_ms },
                WarpMode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                } => hex_warp::Mode::Texture {
                    grain_ms,
                    fluctuation,
                    seed,
                },
                WarpMode::Complex => hex_warp::Mode::Complex,
                WarpMode::ComplexPro { formants, envelope } => {
                    hex_warp::Mode::ComplexPro { formants, envelope }
                }
            }
        } else {
            hex_warp::Mode::Resample
        };
        let input = hex_warp::Input {
            frames: source.frames(),
            sample_rate: source.sample_rate(),
            region_start: domain.source_offset,
            region_length: domain.source_length,
            phase: if domain.warp_enabled {
                0
            } else {
                domain.source_phase
            },
            looping: domain.looping,
            envelope: &envelope,
        };
        let request = hex_warp::Request {
            attacks: &attacks,
            input,
            map: &map,
            output_frames: length as usize,
            pitch_cents: domain.pitch_cents,
            mode,
        };
        let mut rendered =
            hex_warp::render(&request, &mut || self.cancelled()).map_err(warp_error)?;
        if edit.domain.events.is_some() {
            let _scratch_reservation = self.budget.reserve(bytes)?;
            rendered.frames =
                events::compose(&rendered.frames, edit, project.tempo_map(), &self.shutdown)?;
        }
        mapping::apply_fade(
            &mut rendered.frames,
            edit,
            project.tempo_map(),
            &self.shutdown,
        )?;
        let audio = Audio::from_derived(source.sample_rate(), rendered.frames, reservation)?;
        self.retain(Entry {
            source: source.clone(),
            domain: Arc::clone(&edit.domain),
            tempo,
            origin,
            algorithm_version: hex_warp::ALGORITHM_VERSION,
            conversion: asset.conversion_version(),
            source_rate: asset.source_sample_rate(),
            prepared: audio.clone(),
            bytes,
            source_bytes: std::mem::size_of_val(source.frames()),
        })?;
        Ok(audio)
    }
    pub(crate) fn compile_clip(
        &self,
        project: &CanonicalProject,
        clip: &hex_project::Clip,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
    ) -> Result<Option<Clip>, Invalid> {
        let Some(edit) = clip.audio_edit() else {
            return direct_clip(clip, source).map(Some);
        };
        edit.validate(clip, project.tempo_map(), asset.playback_frames())?;
        let (_, _, view_start, view_end) = edit.projected_domain(project.tempo_map())?;
        let domain = &edit.domain;
        let direct = domain.events.is_none()
            && domain.pitch_cents == 0
            && !domain.warp_enabled
            && domain.source_gain.is_empty()
            && domain.fade.fade_in() == 0
            && domain.fade.fade_out() == 0;
        if direct && matches!(source, PreparedAudioSourceRef::Streamed(_)) {
            if !domain.looping && view_start >= domain.source_length {
                return Ok(None);
            }
            let (offset, length, phase) = if domain.looping {
                (
                    domain.source_offset,
                    domain.source_length,
                    ((u128::from(domain.source_phase) + u128::from(view_start))
                        % u128::from(domain.source_length)) as u64,
                )
            } else {
                (
                    domain
                        .source_offset
                        .checked_add(view_start)
                        .ok_or(Invalid("audio crop source overflow"))?,
                    domain.source_length - view_start,
                    0,
                )
            };
            let compiled = match source {
                PreparedAudioSourceRef::Resident(audio) => Clip::region(
                    audio.clone(),
                    clip.start(),
                    clip.duration(),
                    offset,
                    length,
                    domain.looping,
                )?,
                PreparedAudioSourceRef::Streamed(source) => Clip::streamed_region(
                    Arc::clone(source),
                    clip.start(),
                    clip.duration(),
                    offset,
                    length,
                    domain.looping,
                )?,
            }
            .with_source_phase(phase)?
            .with_gain(clip.gain());
            return Ok(Some(compiled));
        }
        let PreparedAudioSourceRef::Resident(source) = source else {
            return Err(Invalid(
                "audio events, pitch, warp and source-envelope preparation currently requires resident audio; streamed source changes were not committed",
            ));
        };
        let audio = self.prepare_audio(project, edit, asset, source)?;
        Clip::region(
            audio,
            clip.start(),
            clip.duration(),
            view_start,
            view_end - view_start,
            false,
        )
        .map(|clip_engine| Some(clip_engine.with_gain(clip.gain())))
    }
}
fn direct_clip(
    clip: &hex_project::Clip,
    source: PreparedAudioSourceRef<'_>,
) -> Result<Clip, Invalid> {
    let compiled = match source {
        PreparedAudioSourceRef::Resident(audio) => Clip::region_with_fade(
            audio.clone(),
            clip.start(),
            clip.duration(),
            clip.source_offset(),
            clip.source_length(),
            clip.looping(),
            clip.fade(),
        )?,
        PreparedAudioSourceRef::Streamed(source) => Clip::streamed_region_with_fade(
            Arc::clone(source),
            clip.start(),
            clip.duration(),
            clip.source_offset(),
            clip.source_length(),
            clip.looping(),
            clip.fade(),
        )?,
    };
    compiled
        .with_gain(clip.gain())
        .with_source_phase(clip.source_phase())
}
fn warp_error(error: hex_warp::Error) -> Invalid {
    Invalid(match error {
        hex_warp::Error::Invalid(message) => message,
        hex_warp::Error::Allocation => "audio transform allocation failed",
        hex_warp::Error::Cancelled => "audio preparation cancelled",
        hex_warp::Error::WorkLimit => "audio preparation exceeds bounded analysis work",
        hex_warp::Error::ScratchLimit => "audio preparation exceeds the 32 mib scratch limit",
    })
}

#[cfg(test)]
mod tests;
