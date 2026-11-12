//! # Purpose
//! Own the canonical control-side audio/MIDI model, stable IDs, validated edits, snapshots and
//! transactional history. Native source definitions retain complete authored
//! device chains and media references independently of track/history lifetime.
//! Tracks bind immutable views of retained sources and may share one source.
//! Each source names one optional bound audio receiver; other bindings contribute
//! MIDI while keeping local processing separate. Engine admission owns DSP and
//! enforces simultaneous playback capacity. Main-lane song sections are stable named/color/range
//! objects, separate from DSP clips. Structural reorder permutes complete
//! supported song content atomically through retained source/history contracts.
//! Clip-local MIDI volume has independent immutable identity
//! from note/source data, with bounded current-format and history fidelity.
//! Ordinary Arrangement placement on a hybrid TrackId replaces occupied
//! half-open intervals; retained fragments preserve their processing/source domains.
//! A selection-fitted crossfade may admit at most two overlapping audio voices
//! when opposing authored ramps cover their full intersection. MIDI and unfaded
//! or triple-voice overlaps refuse. Group edits resolve all incoming members
//! together. Loading validates this law without normalizing source documents;
//! storage retains older schemas' strictly disjoint contract. Source events and
//! DSP tails retain their own owners and are not top-level occupancy.
//!
//! Track hierarchy is one bounded preorder inventory with typed kind/parent IDs.
//! Group subtrees are indivisible for structural edits and clipboard membership;
//! group rows own mixer/effect state, never clips, instruments or Console cells.
//! Playback admission is owned by hex-engine and may refuse unsupported hierarchy.
//!
//! # Dependencies
//! Uses hex-tuning and neutral hex-lattice-contract metadata/state. The pure
//! hex-plugin-state codec validates class identity for VST3 lanes and state
//! replacements. This crate never loads a plug-in or depends on DSP implementations.
//!
//! # Does not own
//! Does not render DSP, access audio devices or maintain a parallel UI editing model. Audio
//! placement uses project-rate frames; MIDI uses checked Q32.32 quarter-note beats. Failed
//! preparation must preserve project, IDs and history.
mod sidechain;
pub use sidechain::{MAX_SIDECHAIN_ROUTES, SidechainRoute, SidechainRouteSnapshot};
mod arrangement_locators;
mod arrangement_occupancy;
pub use arrangement_locators::{
    ArrangementLocator, ArrangementLocatorAppearance, ArrangementLocatorSnapshot,
    MAX_ARRANGEMENT_LOCATORS,
};
mod arrangement_sections;
pub use arrangement_sections::{
    ArrangementSection, ArrangementSectionSnapshot, MAX_ARRANGEMENT_SECTIONS,
    SectionReorderPreview, SectionSplicePreview,
};
mod audio_event_fade;
mod audio_events;
pub use audio_event_fade::AudioEventFade;
mod project_rate;
pub use project_rate::SampleRateMode;
mod project_tempo;
mod track_gain_group;
pub use track_gain_group::RelativeTrackGains;
mod retained_audio;
pub use retained_audio::MAX_RETAINED_AUDIO_DEPTH;
mod track_groups;
mod track_name;
mod track_tags;
pub use track_tags::{MAX_TRACK_TAG_BYTES, MAX_TRACK_TAG_CHARS, MAX_TRACK_TAGS, TrackTag};
mod track_order;
pub use audio_events::{
    AudioEvent, AudioEventEdit, AudioEvents, MAX_AUDIO_EVENTS, RetainedAudioRoot,
};
pub use track_groups::{
    MAX_GROUP_DEPTH, TrackDestination, TrackHierarchy, TrackHierarchyRow, TrackKind,
};
pub use track_order::TrackSelectionIds;
mod audio_edit;
mod clip_activation;
mod clip_alias;
mod clip_alias_commands;
mod clip_alias_edit;
pub use clip_alias::{
    AliasContentSnapshot, AliasMemberSnapshot, AliasSourceSnapshot, ClipAliasesSnapshot,
};
mod audio_event_placement_preview;
mod audio_event_timeline;
pub use audio_event_placement_preview::AudioEventPlacementPreview;
mod clip_fade_split;
mod clip_fade_trim;
mod clip_name;
mod clip_selection_edit;
mod content_window;
mod selection_fades;
mod source_ranges;
pub use audio_edit::{
    AudioClipContent, AudioClipDomain, AudioClipEdit, AudioClipTiming, AudioContentView,
    AudioDomainClock, AudioFadeProjection, AudioImportWarp, AudioProcessingProjection,
    AudioViewMode, AudioViewOccurrence, AudioViewPlacement, AudioViewProjection, AudioViewResize,
    AudioViewResizeOperation, BeatsPreserve, BeatsTail, MAX_AUDIO_DOMAIN_FRAMES, MAX_AUDIO_POINTS,
    MAX_SOURCE_GAIN, MAX_SPARSE_AUDIO_TIMELINE_FRAMES, MusicalEventSource, ProjectedAudioCycles,
    ProjectedAudioFade, SOURCE_GAIN_UNITY, SourceGainPoint, WarpMarker, WarpMode, WarpResampler,
    WarpSettings, audio_projection_beat, audio_projection_frame, audio_projection_frame_q32,
    default_audio_warp, preview_audio_clip_stretch, preview_audio_view_resize, preview_view_resize,
};
pub use clip_selection_edit::{ClipCommandScope, ClipEdit, WarpAdjustment};
pub use content_window::{ContentWindow, RepeatBoundaryPlan, RepeatCycles};
mod appearance;
mod bounce;
mod clip_conversion;
pub use appearance::{ClipFlavor, ElementColor};
pub use bounce::{
    BounceAdoption, BounceDrop, BounceFile, BounceSource, BounceTarget, BounceTrack,
    MAX_ARRANGEMENT_BOUNCE_FRAMES,
};
pub use clip_conversion::AUDIO_TO_MIDI_UNSUPPORTED;
mod track_clipboard;
pub use track_clipboard::TrackClipboard;
mod clip_clipboard;
mod clip_consolidate;
mod clip_copy;
mod clip_duplicate;
mod clip_group;
mod clip_group_preview;
mod clip_reverse;
pub use clip_clipboard::ClipClipboard;
pub use clip_group::{ClipGroupContent, ClipGroupPosition, ClipGroupPreview};
pub use clip_group_preview::ClipGroupMovePreview;
mod clip_fragment;
mod conversion;
mod device;
mod device_automation;
mod device_modulation;
pub use device_modulation::{
    DeviceModulation, DeviceModulationRoute, MAX_MODULATION_ROUTES_PER_DEVICE,
    MAX_MODULATORS_PER_DEVICE, MAX_PROJECT_MODULATION_ROUTES, MAX_PROJECT_MODULATOR_STATE_BYTES,
    MAX_PROJECT_MODULATORS, ModulationAmount, ModulationDestination, ModulationOutputId,
    ModulationRange, ModulationRouteId, ModulationSourceId, Modulator,
};
mod device_inventory;
pub use device_inventory::{DeviceLocation, DeviceOwner};
mod device_unit_template;
pub use device_automation::{
    AutomationInterpolation, AutomationPoint, AutomationReadSelection, AutomationValue,
    DeviceAutomation, DeviceAutomationReplacement, DeviceAutomationTarget,
    DeviceAutomationTouchSpan, DeviceAutomationTouchStep, MAX_DEVICE_AUTOMATION_LANES,
    MAX_DEVICE_AUTOMATION_POINTS, MAX_DEVICE_AUTOMATION_POINTS_PER_LANE,
    MAX_DEVICE_AUTOMATION_TOUCH_SPANS, MAX_DEVICE_AUTOMATION_TOUCH_STEPS_PER_LANE,
    validate_device_automation,
};
pub use device_inventory::MAX_SERIAL_DEVICE_DEPTH;
pub use device_unit_template::{
    DeviceUnitClass, MAX_PORTABLE_DEVICE_UNIT_ASSETS, MAX_PORTABLE_DEVICE_UNIT_NODES,
    MAX_PORTABLE_DEVICE_UNIT_PAYLOAD_BYTES, PortableDeviceAsset, PortableDeviceNode,
    PortableDeviceUnit,
};
mod device_clipboard;
pub use device_clipboard::{DeviceChainOwner, DeviceClipboard, DeviceSelectionIds};
mod device_comparison;
mod device_initialization;
pub use device_initialization::DeviceInitialization;
mod main_devices;
pub use device_comparison::{DeviceComparison, DeviceComparisonAction, DeviceVariant};
mod native_sources;
pub use native_sources::{
    MAX_NATIVE_SOURCES, MAX_SOURCE_DEVICES, NativeSource, NativeSourceSnapshot,
};
mod device_assets;
pub use device::{
    DEFAULT_INSTRUMENT_TYPE_ID, Device, MAX_DEVICES, MAX_DEVICES_PER_TRACK,
    default_instrument_state,
};
mod edit;
pub use clip_fragment::{preview_clip_fragment, preview_clip_fragment_with_tempo};
pub use conversion::{
    IDENTITY_CONVERSION, SINC_CONVERSION_V1, SINC_CONVERSION_V2, converted_frame_count,
    import_conversion_version, validate_conversion_metadata,
};
mod history;
mod instrument_history;
mod midi_history;
pub use midi_history::MidiHistoryEffect;
mod provenance;
pub use provenance::{ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES};
mod time_range;
pub use time_range::{RangeClipboard, TimeRange};
mod gain_envelope;
mod gain_touch;
mod group_output_mute;
pub use gain_touch::{
    GAIN_TOUCH_UNSUPPORTED, GainLane, GainTouchContext, GainTouchSpan, GainTouchStep,
    MAX_GAIN_TOUCH_SPANS_PER_TRACK, MAX_GAIN_TOUCH_STEPS_PER_SPAN, validate_gain_lane_parts,
};
pub use group_output_mute::{GroupOutputMuteRange, MAX_GROUP_OUTPUT_MUTE_RANGES};
mod midi;
mod midi_batch;
mod midi_note_overwrite;
mod midi_playback;
mod midi_source_prefix;
mod midi_source_range;
mod midi_stretch;
pub use midi_stretch::MidiClipStretch;
mod midi_volume;
pub use gain_envelope::{
    AUTOMATION_EDIT_UNSUPPORTED, GainInterpolation, GainPoint, MAX_GAIN_POINTS,
    MAX_GAIN_POINTS_PER_TRACK, validate_gain_envelope,
};
pub use midi_volume::{
    MAX_MIDI_VOLUME_POINTS, MAX_MIDI_VOLUME_POINTS_PER_CLIP, MidiClipGain, MidiClipVolume,
    MidiClipVolumePoint,
};
mod playable_routing;
mod return_feeds;
mod returns;
mod routing;
pub use playable_routing::{
    AudioFeed, AudioFeedSnapshot, MAX_AUDIO_FEEDS, MAX_MIDI_ROUTES, MidiRoute, MidiRouteSnapshot,
};
pub use return_feeds::{MAX_RETURN_FEEDS, ReturnFeed, ReturnFeedSnapshot};
pub use routing::{
    MAX_AUTHORED_ROUTES, ROUTING_UNSUPPORTED, RoutingEdge, RoutingOwner, RoutingSignal,
};
mod session;
mod snapshot;
mod split_tracks;
pub use session::{
    Scene, SessionAudioClip, SessionAudioLoop, SessionCell, SessionCellContent, SessionGrid,
    SessionMidiClip, SessionSceneTarget, session_audio_source_frame,
};
pub mod timebase;
pub use edit::{
    Command, Created, InstrumentLoadPlacement, MidiNoteInsert, MidiNoteUpdate, TrackDeviceSlot,
};
pub use history::{Editor, SampleRateHistoryAction};
mod import_existing;
pub use import_existing::ExistingAudioPlacement;
pub use instrument_history::{InstrumentAssetHistoryDelta, InstrumentHistoryDelta};
pub use midi::{
    MidiClip, MidiClipTrim, MidiContent, MidiNote, MidiOneShotBoundary, MidiSource,
    MidiSourceIdentity, MidiSourceWeakIdentity, TrackContent,
};
mod midi_loop_resize;
pub use midi_loop_resize::{MidiClipLoopResize, MidiViewResizeOperation};
pub use midi_note_overwrite::{
    MidiNoteOverwriteChange, MidiNoteOverwriteGeometry, MidiNoteOverwritePreview,
};
pub use midi_playback::{
    MidiNoteOccurrence, MidiNoteOccurrenceIter, MidiNoteOccurrencePlan, MidiSourcePlayback,
};
pub use returns::{MAX_RETURN_SENDS_PER_TRACK, MAX_RETURN_TRACKS, ReturnTrack, SendLevel};
pub use snapshot::{
    AssetSnapshot, ClipSnapshot, DeviceSnapshot, MidiClipSnapshot, MidiNoteSnapshot,
    MidiSourceSnapshot, ReturnTrackSnapshot, SceneSnapshot, SendLevelSnapshot,
    SessionAudioClipSnapshot, SessionCellContentSnapshot, SessionCellSnapshot,
    SessionMidiClipSnapshot, SessionSnapshot, Snapshot, TrackContentSnapshot, TrackSnapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
    sync::Arc,
};
pub use timebase::{
    AUTOMATION_PROJECTION_VERSION, BeatPulse, BeatTime, CapturedAutomationProjection,
    InputQuantization, MAX_METER_POINTS, MAX_TEMPO_POINTS, Meter, MeterMap, MeterMapCopyCustody,
    MeterMapCopyError, MeterMapCopyPhase, MeterMapOwnedAllocationLayout, MeterPoint, MeterPosition,
    MusicalGridSegment, ResolvedInputGrid, Tempo, TempoMap, TempoMapCopyCustody, TempoMapCopyError,
    TempoMapCopyPhase, TempoMapOwnedAllocationLayout, TempoPoint, query_beat_pulses,
    visit_automation_projection_identity,
};

pub const MAX_TRACKS: usize = 256;
pub const MAX_ASSETS: usize = 4_096;
pub const MAX_CLIPS: usize = 16_384;
pub const MAX_NOTES: usize = 65_536;
pub const MAX_NAME_BYTES: usize = 1_024;
pub const MAX_SCENES: usize = 256;
pub const MAX_SESSION_CELLS: usize = 65_536;

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);
        impl $name {
            pub fn value(self) -> u64 {
                self.0
            }
        }
    };
}
identifier!(TrackId);
identifier!(AssetId);
identifier!(ClipId);
identifier!(AliasSourceId);
identifier!(NoteId);
identifier!(SceneId);
identifier!(DeviceId);
identifier!(ReturnId);
identifier!(RouteId);
identifier!(SectionId);
identifier!(LocatorId);
identifier!(SourceId);

/// Optional half-open timeline region used by prepared playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopRegion {
    start: u64,
    end: u64,
    enabled: bool,
}
impl LoopRegion {
    pub fn new(start: u64, end: u64, enabled: bool) -> Result<Self, Invalid> {
        if start >= end {
            return Err(Invalid("loop region start must be before its end"));
        }
        Ok(Self {
            start,
            end,
            enabled,
        })
    }
    pub fn start(self) -> u64 {
        self.start
    }
    pub fn end(self) -> u64 {
        self.end
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Invalid {}

/// Expected normalized original PCM content. Presence marks a device-original
/// asset, never an ordinary clip derivative. Values grant no native custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginalSourceFacts {
    law_version: u32,
    content_digest: [u8; 32],
}
impl OriginalSourceFacts {
    /// Domain/rate/frame/f32-bit law owned by the native resource catalog.
    pub const LAW_VERSION: u32 = 1;
    pub fn new(law_version: u32, content_digest: [u8; 32]) -> Result<Self, Invalid> {
        if law_version != Self::LAW_VERSION {
            return Err(Invalid("unsupported original source content law"));
        }
        Ok(Self {
            law_version,
            content_digest,
        })
    }
    pub fn law_version(self) -> u32 {
        self.law_version
    }
    pub fn content_digest(self) -> [u8; 32] {
        self.content_digest
    }
}

/// Location and validated metadata. Paths/names retain the user's casing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub(crate) original_source: Option<OriginalSourceFacts>,
    pub(crate) id: AssetId,
    pub(crate) path: PathBuf,
    pub(crate) source_sample_rate: u32,
    pub(crate) source_frames: u64,
    pub(crate) playback_frames: u64,
    pub(crate) conversion_version: u32,
}
impl Asset {
    pub fn original_source(&self) -> Option<OriginalSourceFacts> {
        self.original_source
    }
    pub fn id(&self) -> AssetId {
        self.id
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn frames(&self) -> u64 {
        self.playback_frames
    }
    /// Frame count in the original source file.
    pub fn source_frames(&self) -> u64 {
        self.source_frames
    }
    /// Sample rate of the original source file.
    pub fn source_sample_rate(&self) -> u32 {
        self.source_sample_rate
    }
    /// Frame count after conversion to the project rate.
    pub fn playback_frames(&self) -> u64 {
        self.playback_frames
    }
    /// Stable conversion algorithm identifier used to produce playback PCM.
    pub fn conversion_version(&self) -> u32 {
        self.conversion_version
    }
}

mod clip_fade;
pub use clip_fade::{ClipFade, FadeCurveLaw};

/// Edge used by clip trimming and structural section resizing.
/// Clip trim uses an absolute frame. Section resize changes the prefix/suffix
/// length while keeping its start fixed and rippling later song content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimEdge {
    /// Leading edge or section prefix.
    Left,
    /// Trailing edge or section suffix.
    Right,
}
/// Saved clip-local gain in integer millidecibels, before track processing.
/// The finite -60..=24 dB range does not imply normalization or output limiting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClipGain(i32);
impl ClipGain {
    pub const MIN_MILLIDB: i32 = -60_000;
    pub const MAX_MILLIDB: i32 = 24_000;
    pub const UNITY: Self = Self(0);

    pub fn new(millidb: i32) -> Result<Self, Invalid> {
        if !(Self::MIN_MILLIDB..=Self::MAX_MILLIDB).contains(&millidb) {
            return Err(Invalid("clip gain must be within -60..24 db"));
        }
        Ok(Self(millidb))
    }
    pub fn millidb(self) -> i32 {
        self.0
    }
    /// Control-side coefficient conversion; prepare once, never per audio sample.
    pub fn linear(self) -> f32 {
        if self == Self::UNITY {
            1.0
        } else {
            10.0_f64.powf(f64::from(self.0) / 20_000.0) as f32
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub(crate) active: bool,
    // Immutable authored label shared by private candidates/history. Rename
    // replaces this value; public snapshots retain their owned String contract.
    pub(crate) name: Arc<str>,
    pub(crate) flavor: crate::ClipFlavor,
    pub(crate) id: ClipId,
    pub(crate) asset: AssetId,
    pub(crate) start: u64,
    pub(crate) duration: u64,
    pub(crate) source_offset: u64,
    pub(crate) source_length: u64,
    pub(crate) looping: bool,
    pub(crate) source_phase: u64,
    pub(crate) fade: ClipFade,
    pub(crate) gain: ClipGain,
    pub(crate) edit: Option<Arc<AudioClipEdit>>,
}
impl Clip {
    /// Authored clip activation, independent of track mute or selection.
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn flavor(&self) -> crate::ClipFlavor {
        self.flavor
    }
    pub fn id(&self) -> ClipId {
        self.id
    }
    pub fn asset(&self) -> AssetId {
        self.asset
    }
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn duration(&self) -> u64 {
        self.duration
    }
    pub fn source_offset(&self) -> u64 {
        self.source_offset
    }
    pub fn source_length(&self) -> u64 {
        self.source_length
    }
    pub fn looping(&self) -> bool {
        self.looping
    }
    /// Source-relative playback position at the clip's timeline start.
    pub fn source_phase(&self) -> u64 {
        self.source_phase
    }
    pub fn fade(&self) -> ClipFade {
        self.fade
    }
    pub fn gain(&self) -> ClipGain {
        self.gain
    }
    /// Authored fade in its retained reference-frame domain. A musical fade is
    /// not a newly linear frame fade; use its read-only projected gain for geometry.
    pub fn retained_fade(&self) -> ClipFade {
        self.edit
            .as_ref()
            .map_or(self.fade, |edit| edit.domain.fade)
    }

    pub fn audio_edit(&self) -> Option<&AudioClipEdit> {
        self.edit.as_deref()
    }

    pub(crate) fn audio_edit_mut(&mut self) -> Option<&mut AudioClipEdit> {
        self.edit.as_mut().map(Arc::make_mut)
    }

    /// Trim the selected lower fade handle and its clip edge together. `frame`
    /// is the absolute edge (exclusive for the right edge), with source/domain
    /// bounds and the authored upper fade endpoint applied by the model.
    pub fn preview_fade_edge_trim(
        &self,
        edge: TrimEdge,
        frame: u64,
        asset_playback_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        crate::clip_fade_trim::preview(self, edge, frame, asset_playback_frames, tempo)
    }

    pub fn preview_edge_trim_with_tempo(
        &self,
        edge: TrimEdge,
        frame: u64,
        asset_playback_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        if self.edit.is_some() {
            crate::audio_edit::trim_view(self, edge, frame, tempo)
        } else {
            self.preview_edge_trim(edge, frame, asset_playback_frames)
        }
    }

    /// Return canonical edge-trim metadata for UI preview without cloning the project.
    /// `frame` is an absolute timeline frame and `asset_playback_frames` is the
    /// prepared project-rate asset length.
    pub fn preview_edge_trim(
        &self,
        edge: TrimEdge,
        frame: u64,
        asset_playback_frames: u64,
    ) -> Result<Self, Invalid> {
        crate::edit::preview_edge_trim(self, edge, frame, asset_playback_frames)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub(crate) group_output_mutes: Arc<[GroupOutputMuteRange]>,
    pub(crate) tags: Vec<TrackTag>,
    pub(crate) kind: TrackKind,
    pub(crate) parent_group: Option<TrackId>,
    pub(crate) color: crate::ElementColor,
    pub(crate) gain_lane: GainLane,
    pub(crate) id: TrackId,
    pub(crate) name: String,
    /// Whether `name` was generated by hex (a default or device-derived name)
    /// rather than chosen by the user. See [`Track::name_is_automatic`].
    pub(crate) automatic_name: bool,
    pub(crate) gain: f32,
    pub(crate) balance: f32,
    pub(crate) muted: bool,
    pub(crate) soloed: bool,
    pub(crate) devices: Vec<Device>,
    pub(crate) instrument: Option<native_sources::SourceBinding>,
    /// The bound receiver retains its source path after its own MIDI was printed.
    pub(crate) retained_source_path: bool,
    /// Number of serial effects that precede the optional instrument.
    /// `None` exactly when `instrument` is absent.
    pub(crate) instrument_position: Option<usize>,
    pub(crate) content: TrackContent,
    pub(crate) sends: Vec<SendLevel>,
}
impl Track {
    pub fn kind(&self) -> TrackKind {
        self.kind
    }
    pub fn is_group(&self) -> bool {
        self.kind == TrackKind::Group
    }
    pub fn parent_group(&self) -> Option<TrackId> {
        self.parent_group
    }
    pub fn color(&self) -> ElementColor {
        self.color
    }
    pub fn gain_lane(&self) -> &GainLane {
        &self.gain_lane
    }
    pub fn gain_envelope(&self) -> &[GainPoint] {
        self.gain_lane.base()
    }
    pub fn id(&self) -> TrackId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Whether hex chose this name (default or device-derived); see `track_name`.
    pub fn name_is_automatic(&self) -> bool {
        self.automatic_name
    }
    pub fn gain(&self) -> f32 {
        self.gain
    }
    /// Original Hex stereo balance in the inclusive range -1..=1.
    pub fn balance(&self) -> f32 {
        self.balance
    }
    pub fn muted(&self) -> bool {
        self.muted
    }
    pub fn soloed(&self) -> bool {
        self.soloed
    }
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }
    /// Optional instrument in the track's ordered device chain. `None` means
    /// MIDI on this track is silent.
    pub fn instrument(&self) -> Option<&Device> {
        self.instrument.as_deref()
    }
    /// Stable retained definition; deleting/unlinking this track never removes it.
    pub fn source_id(&self) -> Option<SourceId> {
        self.instrument
            .as_ref()
            .map(native_sources::SourceBinding::source_id)
    }
    /// Whether this receiver keeps its source path independently of track MIDI.
    pub fn retained_source_path(&self) -> bool {
        self.retained_source_path
    }
    /// Whether this track is the selected audio receiver for its bound source.
    pub fn receives_source_audio(&self) -> bool {
        self.instrument
            .as_ref()
            .is_some_and(|source| source.receiver() == Some(self.id))
    }
    /// Number of track-local effects before the source/audio merge, if present.
    pub fn instrument_position(&self) -> Option<usize> {
        self.instrument_position
    }
    /// Borrowed device order for clients that need to present the canonical
    /// chain. `Effect` values remain in `devices()` order; the instrument is
    /// yielded at its persisted gap.
    pub fn ordered_device_chain(&self) -> OrderedDeviceChain<'_> {
        OrderedDeviceChain {
            effects: &self.devices,
            instrument: self.instrument.as_deref(),
            instrument_position: self.instrument_position,
            effect_index: 0,
            instrument_yielded: false,
        }
    }
    /// Beat-anchored audio clips on this hybrid track.
    pub fn clips(&self) -> &[Clip] {
        self.content.audio_clips()
    }
    /// Beat-anchored MIDI clips on this track.
    pub fn midi_clips(&self) -> &[MidiClip] {
        self.content.midi_clips()
    }
    pub fn sends(&self) -> &[SendLevel] {
        &self.sends
    }
}

/// Explicit class/slot reference into a track's canonical ordered device chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDeviceRef<'a> {
    Instrument(&'a Device),
    Effect(&'a Device),
}

/// Allocation-free borrowed iteration over a track's effects and instrument.
pub struct OrderedDeviceChain<'a> {
    effects: &'a [Device],
    instrument: Option<&'a Device>,
    instrument_position: Option<usize>,
    effect_index: usize,
    instrument_yielded: bool,
}
impl<'a> Iterator for OrderedDeviceChain<'a> {
    type Item = TrackDeviceRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.instrument_yielded
            && self.instrument_position == Some(self.effect_index)
            && let Some(instrument) = self.instrument
        {
            self.instrument_yielded = true;
            return Some(TrackDeviceRef::Instrument(instrument));
        }
        let effect = self.effects.get(self.effect_index)?;
        self.effect_index += 1;
        Some(TrackDeviceRef::Effect(effect))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub(crate) aliases: clip_alias::ClipAliases,
    pub(crate) native_sources: Vec<NativeSource>,
    pub(crate) arrangement_sections: Vec<ArrangementSection>,
    pub(crate) arrangement_locators: std::sync::Arc<Vec<ArrangementLocator>>,
    pub(crate) tuning: hex_tuning::ProjectTuning,
    pub(crate) routing: Vec<ReturnFeed>,
    pub(crate) audio_feeds: Vec<AudioFeed>,
    pub(crate) midi_routes: Vec<MidiRoute>,
    pub(crate) sidechains: Vec<SidechainRoute>,
    pub(crate) import_provenance: Option<ImportProvenance>,
    pub(crate) sample_rate: u32,
    pub(crate) sample_rate_mode: SampleRateMode,
    pub(crate) next_id: u64,
    pub(crate) assets: BTreeMap<AssetId, Asset>,
    pub(crate) tracks: Vec<Track>,
    pub(crate) returns: Vec<ReturnTrack>,
    pub(crate) session: SessionGrid,
    pub(crate) loop_region: Option<LoopRegion>,
    pub(crate) master_gain: f32,
    pub(crate) main_devices: Vec<Device>,
    pub(crate) tempo_map: TempoMap,
    pub(crate) meter_map: MeterMap,
    /// Assets whose media was missing when the project opened. This is runtime
    /// availability, never saved: snapshots exclude it and reopening re-detects
    /// missing files. History clones carry it, so an edit that relinks an asset
    /// can undo back to offline. See [`Project::with_offline_assets`].
    pub(crate) offline_assets: BTreeSet<AssetId>,
}
impl Project {
    pub fn new(sample_rate: u32) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid("project sample rate must be 8000..192000 hz"));
        }
        Ok(Self {
            aliases: clip_alias::ClipAliases::default(),
            native_sources: Vec::new(),
            arrangement_sections: Vec::new(),
            arrangement_locators: Default::default(),
            tuning: hex_tuning::ProjectTuning::default(),
            routing: Vec::new(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sidechains: Vec::new(),
            sample_rate,
            sample_rate_mode: SampleRateMode::Fixed,
            next_id: 1,
            import_provenance: None,
            assets: BTreeMap::new(),
            tracks: Vec::new(),
            returns: Vec::new(),
            session: SessionGrid::default(),
            loop_region: None,
            master_gain: 1.0,
            main_devices: Vec::new(),
            tempo_map: TempoMap::constant(sample_rate, Tempo::new(120_000_000)?)?,
            meter_map: MeterMap::common_time(),
            offline_assets: BTreeSet::new(),
        })
    }
    /// Project-saved tuning and audition settings; no local library identity.
    pub fn tuning(&self) -> hex_tuning::ProjectTuning {
        self.tuning
    }

    /// Compare every canonical field except project tuning without cloning metadata.
    /// Used by prepared tuning publications; it does not authorize playback itself.
    pub fn same_except_tuning(&self, other: &Self) -> bool {
        let Self {
            aliases,
            native_sources,
            arrangement_sections,
            arrangement_locators,
            tuning: _,
            routing,
            audio_feeds,
            midi_routes,
            sidechains,
            import_provenance,
            sample_rate,
            sample_rate_mode,
            next_id,
            assets,
            tracks,
            returns,
            session,
            loop_region,
            master_gain,
            main_devices,
            tempo_map,
            meter_map,
            offline_assets,
        } = self;
        aliases == &other.aliases
            && native_sources == &other.native_sources
            && arrangement_sections == &other.arrangement_sections
            && arrangement_locators == &other.arrangement_locators
            && routing == &other.routing
            && audio_feeds == &other.audio_feeds
            && midi_routes == &other.midi_routes
            && sidechains == &other.sidechains
            && import_provenance == &other.import_provenance
            && sample_rate == &other.sample_rate
            && sample_rate_mode == &other.sample_rate_mode
            && next_id == &other.next_id
            && assets == &other.assets
            && tracks == &other.tracks
            && returns == &other.returns
            && session == &other.session
            && loop_region == &other.loop_region
            && main_devices == &other.main_devices
            && master_gain == &other.master_gain
            && tempo_map == &other.tempo_map
            && meter_map == &other.meter_map
            && offline_assets == &other.offline_assets
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Install the explicit default instrument on tracks decoded from schema
    /// versions that predate the canonical slot. New schema data must preserve
    /// `None` as an intentional silent slot and must not call this migration.
    pub fn install_legacy_default_instruments(&mut self) -> Result<(), Invalid> {
        let missing = self
            .tracks
            .iter()
            .filter(|track| track.instrument.is_none())
            .count();
        if missing == 0 {
            return Ok(());
        }
        let total_devices = crate::returns::device_count(self)?;
        if total_devices
            .checked_add(missing)
            .ok_or(Invalid("device count overflow"))?
            > MAX_DEVICES
        {
            return Err(Invalid("project device limit reached"));
        }
        if self
            .native_sources
            .len()
            .checked_add(missing)
            .is_none_or(|n| n > MAX_NATIVE_SOURCES)
            || missing
                .checked_mul(2)
                .and_then(|n| u64::try_from(n).ok())
                .and_then(|n| self.next_id.checked_add(n))
                .is_none()
        {
            return Err(Invalid(
                "legacy instrument source identities exceed project limits",
            ));
        }
        let state = default_instrument_state()?;
        let targets: Vec<_> = self
            .tracks
            .iter()
            .filter(|t| t.instrument.is_none())
            .map(|t| t.id)
            .collect();
        for track in targets {
            self.apply(Command::InsertInstrument {
                track,
                state: state.clone(),
            })?;
        }
        Ok(())
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }
    /// Return tracks are stored in pinned display order.
    pub fn return_tracks(&self) -> &[ReturnTrack] {
        &self.returns
    }
    pub fn session(&self) -> &SessionGrid {
        &self.session
    }
    pub fn scenes(&self) -> &[Scene] {
        &self.session.scenes
    }
    pub fn session_cells(&self) -> &[SessionCell] {
        &self.session.cells
    }
    pub fn session_cell(&self, track: TrackId, scene: SceneId) -> Option<&SessionCellContent> {
        self.session
            .cells
            .iter()
            .find(|cell| cell.track == track && cell.scene == scene)
            .map(|cell| &cell.content)
    }
    pub fn session_midi_clip(&self, id: ClipId) -> Option<&SessionMidiClip> {
        self.session
            .cells
            .iter()
            .find_map(|cell| match &cell.content {
                SessionCellContent::Midi(clip) if clip.id == id => Some(clip),
                _ => None,
            })
    }
    pub fn session_audio_clip(&self, id: ClipId) -> Option<&SessionAudioClip> {
        self.session
            .cells
            .iter()
            .find_map(|cell| match &cell.content {
                SessionCellContent::Audio(clip) if clip.id == id => Some(clip),
                _ => None,
            })
    }
    pub fn midi_source(&self, id: ClipId) -> Option<&MidiSource> {
        self.midi_clip(id)
            .map(MidiClip::source)
            .or_else(|| self.session_midi_clip(id).map(SessionMidiClip::source))
    }
    pub fn loop_region(&self) -> Option<LoopRegion> {
        self.loop_region
    }
    /// Effect-only singleton Main chain; never a regular track or clip destination.
    pub fn main_devices(&self) -> &[Device] {
        &self.main_devices
    }

    /// Linear master output gain in the inclusive range 0..=2.
    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }
    /// Canonical piecewise-constant tempo map. Global tempo edits preserve
    /// musical onsets; each audio clip retains its own source-processing clock.
    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo_map
    }
    /// Canonical meter map used for bar/beat display and snapping.
    pub fn meter_map(&self) -> &MeterMap {
        &self.meter_map
    }
    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }
    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// Whether the asset's media was missing at open; its clips play silence.
    pub fn asset_offline(&self, id: AssetId) -> bool {
        self.offline_assets.contains(&id)
    }

    pub fn offline_assets(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.offline_assets.iter().copied()
    }

    /// Mark assets whose media is missing as offline before the project is
    /// published. Only ordinary file assets used by Arrangement clips (or unused)
    /// may be offline: device-original sources and Session-grid audio cells
    /// still need their media, so they are refused. Paths and clips are kept.
    pub fn with_offline_assets(
        mut self,
        ids: impl IntoIterator<Item = AssetId>,
    ) -> Result<Self, Invalid> {
        for id in ids {
            let asset = self
                .assets
                .get(&id)
                .ok_or(Invalid("offline media refers to an unknown asset"))?;
            if asset.original_source().is_some() || self.device_uses_asset(id) {
                return Err(Invalid("device media cannot be offline"));
            }
            if self.session.cells.iter().any(
                |cell| matches!(&cell.content, SessionCellContent::Audio(clip) if clip.asset == id),
            ) {
                return Err(Invalid("session clip media cannot be offline yet"));
            }
            self.offline_assets.insert(id);
        }
        Ok(self)
    }
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    /// Look up an audio clip. MIDI clips use `midi_clip` even though ClipIds share
    /// the same global identifier space.
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.tracks
            .iter()
            .flat_map(Track::clips)
            .find(|c| c.id == id)
    }
    pub fn midi_clip(&self, id: ClipId) -> Option<&MidiClip> {
        self.tracks
            .iter()
            .flat_map(Track::midi_clips)
            .find(|c| c.id == id)
    }
    pub fn midi_note(&self, id: NoteId) -> Option<&MidiNote> {
        self.tracks
            .iter()
            .flat_map(Track::midi_clips)
            .flat_map(MidiClip::notes)
            .find(|n| n.id == id)
            .or_else(|| {
                self.session
                    .cells
                    .iter()
                    .find_map(|cell| match &cell.content {
                        SessionCellContent::Midi(clip) => {
                            clip.source.notes().iter().find(|note| note.id == id)
                        }
                        SessionCellContent::Audio(_) => None,
                        SessionCellContent::Stop => None,
                    })
            })
    }
    /// Next never-used project identity; opaque device editors reserve IDs
    /// with a compare-and-replace command, never by mutating this cursor.
    pub fn next_identity(&self) -> u64 {
        self.next_id
    }

    pub(crate) fn allocate_id(&mut self) -> Result<u64, Invalid> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or(Invalid("project id space exhausted"))?;
        Ok(id)
    }
    pub(crate) fn allocate_id_range(&mut self, count: usize) -> Result<(u64, u64), Invalid> {
        let count = u64::try_from(count).map_err(|_| Invalid("project id space exhausted"))?;
        if count == 0 {
            return Err(Invalid("project id range must not be empty"));
        }
        let first = self.next_id;
        let next = first
            .checked_add(count)
            .ok_or(Invalid("project id space exhausted"))?;
        let last = next - 1;
        self.next_id = next;
        Ok((first, last))
    }
    pub(crate) fn track_mut(&mut self, id: TrackId) -> Result<&mut Track, Invalid> {
        self.tracks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Invalid("track no longer exists"))
    }
    /// Mutate authored device state at its canonical owner; a retained source
    /// update replaces every immutable binding only after the edit succeeds.
    pub(crate) fn edit_device(
        &mut self,
        id: DeviceId,
        edit: impl FnOnce(&mut Device) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        if let Some(source) = self
            .native_sources
            .iter()
            .find(|s| s.devices().any(|d| d.id() == id))
            .map(NativeSource::id)
        {
            return self.edit_source_device(source, id, edit);
        }
        let device = self
            .tracks
            .iter_mut()
            .flat_map(|t| t.devices.iter_mut())
            .chain(self.returns.iter_mut().flat_map(|t| t.devices.iter_mut()))
            .chain(self.main_devices.iter_mut())
            .find(|d| d.id == id)
            .ok_or(Invalid("device no longer exists"))?;
        edit(device)
    }
    pub(crate) fn clip_mut(&mut self, id: ClipId) -> Result<&mut Clip, Invalid> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.content.audio_clips.as_mut_slice())
            .find(|c| c.id == id)
            .ok_or(Invalid("clip no longer exists"))
    }
    pub(crate) fn audio_clips_mut(&mut self, id: TrackId) -> Result<&mut Vec<Clip>, Invalid> {
        Ok(&mut self.track_mut(id)?.content.audio_clips)
    }
    pub(crate) fn midi_clips_mut(&mut self, clip: ClipId) -> Result<&mut Vec<MidiClip>, Invalid> {
        let index = self
            .tracks
            .iter()
            .position(|track| track.midi_clips().iter().any(|item| item.id == clip))
            .ok_or(Invalid("midi clip no longer exists"))?;
        Ok(&mut self.tracks[index].content.midi_clips)
    }
    pub(crate) fn midi_source_mut(&mut self, id: ClipId) -> Result<&mut MidiSource, Invalid> {
        if self.midi_clip(id).is_some() {
            let clips = self.midi_clips_mut(id)?;
            return clips
                .iter_mut()
                .find(|clip| clip.id == id)
                .map(|clip| std::sync::Arc::make_mut(&mut clip.source))
                .ok_or(Invalid("midi clip no longer exists"));
        }
        self.session
            .cells
            .iter_mut()
            .find_map(|cell| match &mut cell.content {
                SessionCellContent::Midi(clip) if clip.id == id => {
                    Some(std::sync::Arc::make_mut(&mut clip.source))
                }
                _ => None,
            })
            .ok_or(Invalid("midi source no longer exists"))
    }
}
