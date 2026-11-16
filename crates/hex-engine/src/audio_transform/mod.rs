//! Control-owned immutable audio transformation, cache and retained PCM accounting.
//! Structural relocation may freeze original tempo/root context independently of
//! destination placement. Cache identity, mapping, fades and events all consume
//! that retained processing law; a crop never restarts full-root analysis.
//! Frame-clock placements that tile their root window loop the shared cached
//! root without outer PCM. Musical roots at translation-free origins (one
//! constant tempo, exact-frame onset) share a single render, so their copies and
//! placements reuse it the same way. Other finite Once/Repeat placements compose the
//! canonical projection into one resident outer buffer, shared with waveform
//! preparation; their distinct cache identity retains geometry and tempo. Outer frames and summed
//! musical full-root work each cap at 16777216; at most 8192 occurrences are
//! admitted before PCM allocation. Unsupported streamed placement preparation
//! refuses without changing project or media. Ordinary crops keep their path.
//! Grouped frame events resolve every retained root before PCM work. Each source
//! keeps its captured tempo, finite placement and fade/gain order. The container
//! and summed child finite/full-root work share the 16777216-frame cap; source
//! count is bounded by MAX_AUDIO_EVENTS and placement by 8192 occurrences. All
//! child PCM/conversion identities participate in the group cache key. Streamed
//! roots and outer pitch/warp/source processing refuse before history commit.
mod budget;
mod content;
mod events;
mod group;
pub(crate) use group::{GroupSource, sparse_group_plan};
mod mapping;
mod view;
use crate::{Audio, Clip, Invalid, compile::PreparedAudioSourceRef};
pub(crate) use budget::Reservation;
use hex_project::{
    Asset, AudioClipDomain, AudioClipEdit, BeatsTail, Project as CanonicalProject, WarpMode,
    WarpResampler,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub use view::AudioPreviewRoot;
fn render_mode(domain: &AudioClipDomain) -> Result<hex_warp::Mode, Invalid> {
    Ok(if domain.warp_enabled {
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
    })
}

/// Saved reconstruction law of the root pitch/warp render. The cache key holds
/// the whole domain, so a law change never reuses another law's output.
fn render_resampler(domain: &AudioClipDomain) -> hex_warp::Resampler {
    warp_law(domain.warp_resampler)
}

fn warp_law(law: WarpResampler) -> hex_warp::Resampler {
    match law {
        WarpResampler::V1 => hex_warp::Resampler::V1,
        WarpResampler::V2 => hex_warp::Resampler::V2,
    }
}

/// Check the existing renderer's projected stretch capability without PCM or
/// preparation/cache ownership. Allocates the same bounded metadata map (at most
/// 8192 segments and 1024 explicit attacks); this is control-side work, not a
/// callback API and not a proof of available render memory or work budget.
pub fn validate_clip_warp_preview(
    clip: &hex_project::Clip,
    tempo: &hex_project::TempoMap,
) -> Result<(), Invalid> {
    let Some(edit) = clip.audio_edit() else {
        return Ok(());
    };
    let (map, _) = mapping::prepare_map(edit, tempo, &AtomicBool::new(false))?;
    map.validate_stretch(render_mode(&edit.domain)?, edit.domain.pitch_cents)
        .map_err(warp_error)
}

/// Project a validated canonical Re-Pitch root for a bounded streaming worker.
/// The returned map covers the full processed root; view cropping is applied by
/// the consumer. It owns no audio and does not establish file or page custody.
/// Unsupported source processing is refused rather than silently omitted.
pub fn streamed_repitch_map(
    edit: &AudioClipEdit,
    tempo: &hex_project::TempoMap,
    source_frames: u64,
    cancel: &AtomicBool,
) -> Result<(hex_warp::TimeMap, hex_warp::Resampler), Invalid> {
    let domain = &edit.domain;
    if !edit.is_streamed_repitch_root() || domain.source_length != source_frames {
        return Err(Invalid(
            "streamed repitch requires an unlooped neutral full-source warp root",
        ));
    }
    let (map, attacks) = mapping::prepare_streamed_map(edit, tempo, cancel)?;
    if !attacks.is_empty() {
        return Err(Invalid("streamed repitch cannot use beat attacks"));
    }
    map.validate_stretch(hex_warp::Mode::RePitch, 0)
        .map_err(warp_error)?;
    Ok((map, render_resampler(domain)))
}

/// Immutable control-side file facts and canonical map for one streamed
/// Re-Pitch source. This is not a worker, page lease or publication authority.
#[derive(Clone)]
pub struct StreamedRepitchRequirement {
    inner: Arc<StreamedRepitchRequirementInner>,
}
struct StreamedRepitchRequirementInner {
    raw: crate::StreamedSourceMetadata,
    map: hex_warp::TimeMap,
    law: hex_warp::Resampler,
}
impl StreamedRepitchRequirement {
    pub fn new(
        raw: &crate::StreamedSourceMetadata,
        edit: &AudioClipEdit,
        tempo: &hex_project::TempoMap,
        cancel: &AtomicBool,
    ) -> Result<Self, Invalid> {
        if !raw.key().is_raw() || raw.project_rate() != tempo.sample_rate() {
            return Err(Invalid(
                "repitch requirement needs a raw source at the project rate",
            ));
        }
        let (map, law) = streamed_repitch_map(edit, tempo, raw.playback_frames(), cancel)?;
        Ok(Self {
            inner: Arc::new(StreamedRepitchRequirementInner {
                raw: raw.clone(),
                map,
                law,
            }),
        })
    }
    /// Derive a catalog-specific key only for the same verified file facts.
    pub fn metadata_for(
        &self,
        raw: &crate::StreamedSourceMetadata,
    ) -> Result<crate::StreamedSourceMetadata, Invalid> {
        let expected = &self.inner.raw;
        if !raw.key().is_raw()
            || raw.key().asset() != expected.key().asset()
            || raw.source_rate() != expected.source_rate()
            || raw.project_rate() != expected.project_rate()
            || raw.source_frames() != expected.source_frames()
            || raw.playback_frames() != expected.playback_frames()
            || raw.conversion_version() != expected.conversion_version()
            || raw.fingerprint() != expected.fingerprint()
        {
            return Err(Invalid("repitch requirement source facts differ"));
        }
        raw.with_repitch(&self.inner.map, self.inner.law)
    }
    pub fn asset(&self) -> hex_project::AssetId {
        self.inner.raw.key().asset()
    }
    pub fn map(&self) -> &hex_warp::TimeMap {
        &self.inner.map
    }
    /// Copy the bounded map for an independent worker without infallible allocation.
    pub fn try_owned_map(&self) -> Result<hex_warp::TimeMap, Invalid> {
        let source = self.inner.map.segments();
        let end = source
            .last()
            .ok_or(Invalid("empty repitch map"))?
            .output_end;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(source.len())
            .map_err(|_| Invalid("repitch map reservation failed"))?;
        segments.extend_from_slice(source);
        hex_warp::TimeMap::new_streamed(segments, end).map_err(warp_error)
    }
    pub fn law(&self) -> hex_warp::Resampler {
        self.inner.law
    }
    /// Bytes retained by the shared map owner, excluding the caller's handle/vector.
    pub fn retained_bytes(&self) -> Option<usize> {
        std::mem::size_of::<StreamedRepitchRequirementInner>()
            .checked_add(2 * std::mem::size_of::<usize>())?
            .checked_add(self.inner.map.backing_bytes()?)
    }
}

/// Immutable control-owned handles to Re-Pitch source requirements.
/// The set is factual metadata, not a worker, page lease or publication owner.
#[derive(Clone, Default)]
pub struct StreamedRepitchRequirements {
    requirements: Vec<StreamedRepitchRequirement>,
}
impl StreamedRepitchRequirements {
    pub fn new(requirements: Vec<StreamedRepitchRequirement>) -> Result<Self, Invalid> {
        if requirements.len() > hex_project::MAX_CLIPS
            || requirements.capacity() > hex_project::MAX_CLIPS
        {
            return Err(Invalid("repitch requirement count exceeded"));
        }
        Ok(Self { requirements })
    }
    pub fn try_push(&mut self, requirement: StreamedRepitchRequirement) -> Result<(), Invalid> {
        if self.requirements.len() >= hex_project::MAX_CLIPS {
            return Err(Invalid("repitch requirement count exceeded"));
        }
        self.requirements
            .try_reserve_exact(1)
            .map_err(|_| Invalid("repitch requirement reservation failed"))?;
        if self.requirements.capacity() > hex_project::MAX_CLIPS {
            return Err(Invalid("repitch requirement capacity exceeded"));
        }
        self.requirements.push(requirement);
        Ok(())
    }
    pub fn handle_capacity_bytes(&self) -> Option<usize> {
        self.requirements
            .capacity()
            .checked_mul(std::mem::size_of::<StreamedRepitchRequirement>())
    }
    pub fn requirements(&self) -> &[StreamedRepitchRequirement] {
        &self.requirements
    }
    pub(crate) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        ledger.bytes(
            Owner::exclusive(self),
            Location("repitch-requirement-set", 0),
            std::mem::size_of::<Self>(),
        )?;
        ledger.vec(&self.requirements)?;
        for requirement in &self.requirements {
            ledger.bytes(
                Owner::shared(&requirement.inner),
                Location("repitch-requirement-map", 0),
                requirement
                    .retained_bytes()
                    .ok_or(crate::AudioStorageError::Overflow)?,
            )?;
        }
        Ok(())
    }
}

pub const MAX_CACHED_DERIVED_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_CACHED_TRANSFORMS: usize = 256;
/// Maximum unique source PCM capacity retained by committed and pending cache entries.
pub const MAX_CACHED_SOURCE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_RETAINED_DERIVED_BYTES: usize = budget::RETAINED_LIMIT;

struct Entry {
    source: Audio,
    key: EntryKey,
    algorithm_version: u32,
    conversion: u32,
    source_rate: u32,
    prepared: Audio,
    bytes: usize,
}
// Charge each immutable PCM allocation once, including grouped source owners.
// Entries keep the Arcs alive throughout this control-side calculation.
fn source_charge<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    extra: Option<&'a Audio>,
) -> Result<usize, Invalid> {
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    let mut add = |audio: &Audio| -> Result<(), Invalid> {
        seen.try_reserve(1)
            .map_err(|_| Invalid("audio source accounting allocation failed"))?;
        if seen.insert(Arc::as_ptr(&audio.frames)) {
            bytes = bytes
                .checked_add(
                    audio
                        .pcm_capacity_frames()
                        .checked_mul(std::mem::size_of::<crate::Frame>())
                        .ok_or(Invalid("audio source count overflow"))?,
                )
                .ok_or(Invalid("audio source count overflow"))?;
        }
        Ok(())
    };
    for entry in entries {
        add(&entry.source)?;
        if let EntryKey::Group { sources, .. } = &entry.key {
            for (audio, _, _) in sources {
                add(audio)?;
            }
        }
    }
    if let Some(audio) = extra {
        add(audio)?;
    }
    Ok(bytes)
}
enum EntryKey {
    Root {
        domain: Arc<AudioClipDomain>,
        tempo: Option<Arc<hex_project::TempoMap>>,
        /// `None` when the processing origin is translation-free (see
        /// `translation_free_origin`): every such origin renders identical PCM.
        origin: Option<i128>,
    },
    Group {
        root_only: bool,
        sparse: bool,
        edit: AudioClipEdit,
        duration: u64,
        sources: Vec<(Audio, u32, u32)>,
    },
    View {
        edit: AudioClipEdit,
        tempo: Arc<hex_project::TempoMap>,
        start: u64,
        duration: u64,
    },
}
impl EntryKey {
    /// Frame-clock output is defined by its source program and finite view,
    /// independent of destination onset or the current project tempo. Musical
    /// views retain both because their sample projection can differ there.
    fn matches_view(
        &self,
        edit: &AudioClipEdit,
        tempo_map: &hex_project::TempoMap,
        start: u64,
        duration: u64,
    ) -> bool {
        matches!(self, Self::View { edit: retained, tempo, start: at, duration: length }
            if retained == edit && *length == duration
                && (matches!(edit.domain.clock, hex_project::AudioDomainClock::Frames(_))
                    || (*at == start && tempo.as_ref() == tempo_map)))
    }
}
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
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
    preview_cancel: RefCell<Option<Arc<AtomicBool>>>,
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
    /// Native constructor bridge: no cache/domain/source owners survive this
    /// one control-side clip compilation or enter the native storage ledger.
    pub(crate) fn new_uncached_on_control() -> Self {
        Self {
            cache_limit: 0,
            source_limit: 0,
            entry_limit: 0,
            ..Self::default()
        }
    }
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self {
            cache: RefCell::new(Cache {
                entries: VecDeque::new(),
                bytes: 0,
            }),
            pending: RefCell::new(None),
            tempo: RefCell::new(None),
            shutdown,
            preview_cancel: RefCell::new(None),
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
            || self
                .preview_cancel
                .borrow()
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Acquire))
    }
    /// Cooperatively abandon a superseded control-side preview without
    /// cancelling the document's canonical preparation.
    pub fn with_preview_cancellation<T>(
        &self,
        cancel: &Arc<AtomicBool>,
        prepare: impl FnOnce() -> T,
    ) -> T {
        let previous = self.preview_cancel.replace(Some(Arc::clone(cancel)));
        let result = prepare();
        self.preview_cancel.replace(previous);
        result
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
            || source_charge(std::iter::once(&entry), None)? > self.source_limit
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
            || source_charge(cache.entries.iter().chain(std::iter::once(&entry)), None)?
                > self.source_limit
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
        cache.entries.push_back(entry);
        Ok(())
    }
    fn retain(&self, entry: Entry) -> Result<(), Invalid> {
        if entry.bytes > self.cache_limit
            || source_charge(std::iter::once(&entry), None)? > self.source_limit
            || self.entry_limit == 0
        {
            return Ok(());
        }
        let mut pending = self
            .pending
            .try_borrow_mut()
            .map_err(|_| Invalid("audio preparation transaction is already borrowed"))?;
        if let Some(entries) = pending.as_mut() {
            let mut bytes = entries.iter().try_fold(0usize, |sum, e| {
                sum.checked_add(e.bytes)
                    .ok_or(Invalid("audio pending byte count overflow"))
            })?;
            while entries.len() >= self.entry_limit
                || bytes
                    .checked_add(entry.bytes)
                    .is_none_or(|sum| sum > self.cache_limit)
                || source_charge(entries.iter().chain(std::iter::once(&entry)), None)?
                    > self.source_limit
            {
                if entries.is_empty() {
                    return Err(Invalid("audio pending cache accounting is inconsistent"));
                }
                bytes -= entries.remove(0).bytes;
            }
            let mut cache = self
                .cache
                .try_borrow_mut()
                .map_err(|_| Invalid("audio source cache is already borrowed"))?;
            while cache
                .bytes
                .checked_add(bytes)
                .and_then(|n| n.checked_add(entry.bytes))
                .is_none_or(|n| n > self.cache_limit)
                || source_charge(
                    cache
                        .entries
                        .iter()
                        .chain(entries.iter())
                        .chain(std::iter::once(&entry)),
                    None,
                )? > self.source_limit
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
        origin: Option<i128>,
        asset: &Asset,
    ) -> Result<Option<Audio>, Invalid> {
        self.lookup_key(source, asset, |key| {
            matches!(key, EntryKey::Root { domain, tempo: retained, origin: at }
                if *at == origin && retained.as_deref() == tempo.as_deref()
                    && (Arc::ptr_eq(domain, &edit.domain) || domain == &edit.domain
                        || (origin.is_none() && same_translated_domain(domain, &edit.domain))))
        })
    }
    fn lookup_key(
        &self,
        source: &Audio,
        asset: &Asset,
        key_matches: impl Fn(&EntryKey) -> bool,
    ) -> Result<Option<Audio>, Invalid> {
        let matching = |entry: &Entry| {
            entry.algorithm_version == hex_warp::ALGORITHM_VERSION
                && entry.source.shares_pcm_with(source)
                && entry.source.sample_rate() == source.sample_rate()
                && entry.conversion == asset.conversion_version()
                && entry.source_rate == asset.source_sample_rate()
                && key_matches(&entry.key)
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
    #[cfg(test)]
    fn prepare_audio(
        &self,
        project: &CanonicalProject,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: &Audio,
    ) -> Result<Audio, Invalid> {
        self.prepare_audio_at_tempo(project.tempo_map(), edit, asset, source)
    }
    fn prepare_audio_at_tempo(
        &self,
        tempo_map: &hex_project::TempoMap,
        edit: &AudioClipEdit,
        asset: &Asset,
        source: &Audio,
    ) -> Result<Audio, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let (_, length, _, _) = edit.projected_domain(tempo_map)?;
        let tempo = if edit.reference_start_raw().is_some() {
            Some(self.shared_tempo(edit.processing_tempo(tempo_map))?)
        } else {
            None
        };
        let origin = if edit.reference_start_raw().is_some() {
            let origin = edit.processing_origin()?;
            (!translation_free_origin(edit.processing_tempo(tempo_map), origin)?).then_some(origin)
        } else {
            Some(0)
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
        let source_bytes = source
            .pcm_capacity_frames()
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("audio source count overflow"))?;
        let cacheable =
            bytes <= self.cache_limit && source_bytes <= self.source_limit && self.entry_limit > 0;
        if cacheable {
            let mut pending = self
                .pending
                .try_borrow_mut()
                .map_err(|_| Invalid("audio pending cache is already borrowed"))?;
            let mut pending_bytes =
                pending
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .try_fold(0usize, |sum, entry| {
                        sum.checked_add(entry.bytes)
                            .ok_or(Invalid("audio pending byte count overflow"))
                    })?;
            if let Some(entries) = pending.as_mut() {
                while pending_bytes
                    .checked_add(bytes)
                    .is_none_or(|sum| sum > self.cache_limit)
                    || source_charge(entries.iter(), Some(source))? > self.source_limit
                {
                    if entries.is_empty() {
                        return Err(Invalid("audio pending accounting is inconsistent"));
                    }
                    pending_bytes -= entries.remove(0).bytes;
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
                || source_charge(
                    cache
                        .entries
                        .iter()
                        .chain(pending.as_ref().into_iter().flatten()),
                    Some(source),
                )? > self.source_limit
            {
                cache.evict_oldest()?;
            }
        }
        let source_edit = edit
            .domain
            .musical_event_source
            .map(|source| {
                let origin = edit.processing_origin()? + i128::from(source.zero_in_destination);
                let mut root = view::occurrence_edit(edit, origin, self, tempo_map)?;
                root.view_end = source.length.raw();
                let domain = Arc::make_mut(&mut root.domain);
                domain.clock = hex_project::AudioDomainClock::Beats(source.length);
                domain.musical_event_source = None;
                domain.events = None;
                Ok::<_, Invalid>(root)
            })
            .transpose()?;
        let read_root = source_edit.as_ref().unwrap_or(edit);
        let read_length = if source_edit.is_some() {
            read_root.projected_domain(tempo_map)?.1
        } else {
            length
        };
        let reservation = self.reserve_with_eviction(bytes)?;
        let scratch_reservation = if edit.domain.events.is_some() {
            let scratch_bytes = usize::try_from(read_length)
                .ok()
                .and_then(|length| length.checked_mul(std::mem::size_of::<crate::Frame>()))
                .ok_or(Invalid("audio event source byte count overflow"))?;
            Some(self.reserve_with_eviction(scratch_bytes)?)
        } else {
            None
        };
        let (map, attacks) = mapping::prepare_map(read_root, tempo_map, &self.shutdown)?;
        let mut envelope = Vec::new();
        envelope
            .try_reserve_exact(edit.domain.source_gain.len())
            .map_err(|_| Invalid("source envelope allocation failed"))?;
        envelope.extend(edit.domain.source_gain.iter().map(|p| hex_warp::GainPoint {
            source_frame: p.frame,
            gain_q16: p.gain_q16,
        }));
        let domain = &edit.domain;
        let mode = render_mode(domain)?;
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
            output_frames: read_length as usize,
            pitch_cents: domain.pitch_cents,
            mode,
        };
        let mut rendered =
            hex_warp::render_with_resampler(&request, render_resampler(domain), &mut || {
                self.cancelled()
            })
            .map_err(warp_error)?;
        if edit.domain.events.is_some() {
            rendered.frames = events::compose(
                events::EventSource {
                    frames: &rendered.frames,
                    root: read_root,
                    zero_in_destination: edit
                        .domain
                        .musical_event_source
                        .map_or(0, |source| source.zero_in_destination),
                },
                edit,
                tempo_map,
                &self.shutdown,
            )?;
        }
        drop(scratch_reservation);
        mapping::apply_fade(&mut rendered.frames, 0, edit, tempo_map, &self.shutdown)?;
        let audio = Audio::from_derived(source.sample_rate(), rendered.frames, reservation)?;
        self.retain(Entry {
            source: source.clone(),
            key: EntryKey::Root {
                domain: Arc::clone(&edit.domain),
                tempo,
                origin,
            },
            algorithm_version: hex_warp::ALGORITHM_VERSION,
            conversion: asset.conversion_version(),
            source_rate: asset.source_sample_rate(),
            prepared: audio.clone(),
            bytes,
        })?;
        Ok(audio)
    }
    fn reserve_with_eviction(&self, bytes: usize) -> Result<Reservation, Invalid> {
        loop {
            match self.budget.reserve(bytes) {
                Ok(reservation) => return Ok(reservation),
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
        }
    }
    pub(crate) fn compile_clip(
        &self,
        project: &CanonicalProject,
        clip: &hex_project::Clip,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
    ) -> Result<Option<Clip>, Invalid> {
        self.compile_clip_at_tempo(project.tempo_map(), clip, asset, source)
    }
    /// Current streamed clip eligibility, shared by compilation and recovery
    /// planning. Processing roots that materialize PCM keep their original
    /// resident boundary; this pure query grants no decoding/admission custody.
    pub fn clip_requires_resident_source(clip: &hex_project::Clip) -> bool {
        clip.audio_edit().is_some_and(|edit| {
            (edit.content_view.is_some() && edit.plain_reversed_source(clip).is_none())
                || edit.placement.is_some()
                || edit.domain.events.is_some()
                || edit.domain.pitch_cents != 0
                || edit.domain.warp_enabled
                || !edit.domain.source_gain.is_empty()
                || !edit.domain.fade.is_neutral()
        })
    }

    fn compile_clip_at_tempo(
        &self,
        tempo_map: &hex_project::TempoMap,
        clip: &hex_project::Clip,
        asset: &Asset,
        source: PreparedAudioSourceRef<'_>,
    ) -> Result<Option<Clip>, Invalid> {
        let Some(edit) = clip.audio_edit() else {
            return direct_clip(clip, source).map(Some);
        };
        edit.validate(clip, tempo_map, asset.playback_frames())?;
        if let PreparedAudioSourceRef::StreamedWithPitch(raw, derived_sources) = source
            && let Ok((map, law)) =
                streamed_repitch_map(edit, tempo_map, raw.playback_frames(), &self.shutdown)
        {
            let expected = raw.with_repitch(&map, law)?;
            let derived = derived_sources
                .get(&expected.key())
                .filter(|candidate| candidate.as_ref() == &expected)
                .ok_or(Invalid("matching repitch stream is unavailable"))?;
            let (_, _, view_start, view_end) = edit.projected_domain(tempo_map)?;
            return Clip::streamed_region(
                Arc::clone(derived),
                clip.start(),
                clip.duration(),
                view_start,
                view_end - view_start,
                false,
            )
            .map(|prepared| Some(prepared.with_gain(clip.gain())));
        }
        if let PreparedAudioSourceRef::StreamedWithPitch(raw, pitched) = source
            && edit.is_plain_pitch_capture(clip)
            && clip.source_length() == asset.playback_frames()
        {
            let expected =
                raw.with_plain_pitch(edit.domain.pitch_cents, edit.domain.warp_resampler)?;
            let derived = pitched
                .get(&expected.key())
                .filter(|candidate| candidate.as_ref() == &expected)
                .ok_or(Invalid("matching pitched stream is unavailable"))?;
            return Clip::streamed_region_with_fade(
                Arc::clone(derived),
                clip.start(),
                clip.duration(),
                0,
                clip.duration(),
                false,
                edit.domain.fade,
            )
            .map(|prepared| Some(prepared.with_gain(clip.gain())));
        }
        let source = match source {
            PreparedAudioSourceRef::StreamedWithPitch(raw, _) => {
                PreparedAudioSourceRef::Streamed(raw)
            }
            source => source,
        };
        if edit.content_view.is_some() {
            if let PreparedAudioSourceRef::Streamed(source) = source
                && let Some((offset, length)) = edit.plain_reversed_source(clip)
            {
                return Clip::streamed_region_reversed(
                    Arc::clone(source),
                    clip.start(),
                    clip.duration(),
                    offset,
                    length,
                    false,
                )
                .map(|prepared| Some(prepared.with_gain(clip.gain())));
            }
            return self
                .compile_content(tempo_map, clip, edit, asset, source)
                .map(Some);
        }
        if edit.placement.is_some() {
            return self
                .compile_view(tempo_map, clip, edit, asset, source)
                .map(Some);
        }
        let (_, _, view_start, view_end) = edit.projected_domain(tempo_map)?;
        let domain = &edit.domain;
        let direct = !Self::clip_requires_resident_source(clip);
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
                PreparedAudioSourceRef::Streamed(source)
                | PreparedAudioSourceRef::StreamedWithPitch(source, _) => Clip::streamed_region(
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
                "audio events, pitch, warp, source-envelope and fade preparation currently require resident audio; streamed source changes were not committed",
            ));
        };
        let audio = self.prepare_audio_at_tempo(tempo_map, edit, asset, source)?;
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

/// A root's processing origin only enters its render through tempo points
/// relative to it, `tempo_at`, and differences `frame(origin + x) - frame(origin)`
/// / `beat(frame(origin) + f) - origin`. Under one constant tempo, a non-negative
/// origin on an exact Q32 frame boundary makes every such term independent of
/// the origin, so all translation-free origins prepare identical PCM. Copies of a
/// musical clip on the beat grid of a constant-tempo song can then share a root.
pub(crate) fn translation_free_origin(
    tempo: &hex_project::TempoMap,
    origin: i128,
) -> Result<bool, Invalid> {
    let ([point], Ok(raw)) = (tempo.points(), u64::try_from(origin)) else {
        return Ok(false);
    };
    if point.beat() != hex_project::BeatTime::ZERO {
        return Ok(false);
    }
    let frame_q32 = tempo.beat_to_frame_q32(hex_project::BeatTime::from_raw(raw))?;
    Ok(frame_q32 & ((1_u128 << 32) - 1) == 0)
}

/// A neutral fade never reads its projection, which records the clip's absolute
/// onset. Translated copies otherwise share every rendered domain input.
fn same_translated_domain(a: &AudioClipDomain, b: &AudioClipDomain) -> bool {
    a.content == b.content
        && a.source_phase == b.source_phase
        && a.looping == b.looping
        && a.fade == b.fade
        && a.fade_start == b.fade_start
        && (a.fade_projection == b.fade_projection || a.fade.is_neutral())
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
        PreparedAudioSourceRef::Streamed(source)
        | PreparedAudioSourceRef::StreamedWithPitch(source, _) => Clip::streamed_region_with_fade(
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

/// Conservative metadata envelope for one sequential uncached native transform.
/// Bounded growable map boundaries, two maps, attack/gain points, group bindings
/// and identities, tempo/projection copies and fixed wrappers are independent of
/// PCM and of the warp library's separate scratch ceiling.
pub fn native_metadata_workspace_bytes() -> Option<usize> {
    use std::mem::size_of;
    let maps = hex_warp::MAX_MAP_SEGMENTS.checked_mul(
        4usize
            .checked_mul(size_of::<(u64, bool)>())?
            .checked_add(2usize.checked_mul(size_of::<(u64, u64)>())?)?
            .checked_add(2usize.checked_mul(size_of::<hex_warp::MapSegment>())?)?,
    )?;
    let points = hex_project::MAX_AUDIO_POINTS
        .checked_mul(4usize.checked_mul(size_of::<hex_warp::GainPoint>() + size_of::<u64>())?)?;
    let sources = hex_project::MAX_AUDIO_EVENTS
        .checked_mul(size_of::<GroupSource<'static>>() + size_of::<(Audio, u32, u32)>())?;
    maps.checked_add(points)?
        .checked_add(sources)?
        .checked_add(hex_project::MAX_TEMPO_POINTS.checked_mul(128)?)?
        .checked_add(64 * 1024)
}
