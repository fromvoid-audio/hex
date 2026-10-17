//! Canonical, control-side audio/MIDI arrangement and transactional editing.
//! Audio/source positions are project-rate frames; MIDI clip/note positions are beats.
mod audio_events;
pub use audio_events::{AudioEvent, AudioEventEdit, AudioEvents, MAX_AUDIO_EVENTS};
mod audio_edit;
mod clip_fade_trim;
mod clip_name;
pub use audio_edit::{
    AudioClipDomain, AudioClipEdit, AudioDomainClock, AudioFadeProjection, BeatsPreserve,
    BeatsTail, MAX_AUDIO_DOMAIN_FRAMES, MAX_AUDIO_POINTS, MAX_SOURCE_GAIN, ProjectedAudioFade,
    SOURCE_GAIN_UNITY, SourceGainPoint, WarpMarker, WarpMode, WarpSettings, audio_projection_beat,
    audio_projection_frame, default_audio_warp, preview_audio_clip_stretch,
};
mod appearance;
mod clip_conversion;
pub use appearance::{ClipFlavor, ElementColor};
mod clip_clipboard;
mod clip_copy;
pub use clip_clipboard::ClipClipboard;
mod clip_fragment;
mod conversion;
mod device;
mod device_assets;
pub use device::{
    DEFAULT_INSTRUMENT_TYPE_ID, Device, MAX_DEVICES, MAX_DEVICES_PER_TRACK,
    default_instrument_state,
};
mod edit;
pub use clip_fragment::{preview_clip_fragment, preview_clip_fragment_with_tempo};
pub use conversion::{
    IDENTITY_CONVERSION, SINC_CONVERSION_V1, converted_frame_count, validate_conversion_metadata,
};
mod history;
mod provenance;
pub use provenance::{ImportProvenance, MAX_IMPORT_DOCUMENT_BYTES};
mod time_range;
pub use time_range::{RangeClipboard, TimeRange};
mod gain_envelope;
mod gain_touch;
pub use gain_touch::{
    GAIN_TOUCH_UNSUPPORTED, GainLane, GainTouchContext, GainTouchSpan, GainTouchStep,
    MAX_GAIN_TOUCH_SPANS_PER_TRACK, MAX_GAIN_TOUCH_STEPS_PER_SPAN, validate_gain_lane_parts,
};
mod midi;
mod midi_batch;
mod midi_playback;
pub use gain_envelope::{
    AUTOMATION_EDIT_UNSUPPORTED, GainInterpolation, GainPoint, MAX_GAIN_POINTS,
    MAX_GAIN_POINTS_PER_TRACK, validate_gain_envelope,
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
    Scene, SessionAudioClip, SessionCell, SessionCellContent, SessionGrid, SessionMidiClip,
};
pub mod timebase;
pub use edit::{Command, Created, MidiNoteInsert, MidiNoteUpdate, TrackDeviceSlot};
pub use history::Editor;
pub use midi::{MidiClip, MidiClipTrim, MidiNote, MidiSource, MidiSourceIdentity, TrackContent};
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
use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};
pub use timebase::{
    BeatPulse, BeatTime, MAX_METER_POINTS, MAX_TEMPO_POINTS, Meter, MeterMap, MeterMapCopyCustody,
    MeterMapCopyError, MeterMapCopyPhase, MeterMapOwnedAllocationLayout, MeterPoint, MeterPosition,
    Tempo, TempoMap, TempoMapCopyCustody, TempoMapCopyError, TempoMapCopyPhase,
    TempoMapOwnedAllocationLayout, TempoPoint, query_beat_pulses,
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
identifier!(NoteId);
identifier!(SceneId);
identifier!(DeviceId);
identifier!(ReturnId);
identifier!(RouteId);

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

/// Location and validated metadata. Paths/names retain the user's casing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub(crate) id: AssetId,
    pub(crate) path: PathBuf,
    pub(crate) source_sample_rate: u32,
    pub(crate) source_frames: u64,
    pub(crate) playback_frames: u64,
    pub(crate) conversion_version: u32,
}
impl Asset {
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
pub use clip_fade::ClipFade;

/// Edge used by the canonical absolute-frame clip trim command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimEdge {
    /// Inclusive timeline start frame.
    Left,
    /// Exclusive timeline end frame.
    Right,
}
/// Saved audio-clip gain in integer millidecibels, before track processing.
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
    pub(crate) name: String,
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
    pub(crate) color: crate::ElementColor,
    pub(crate) gain_lane: GainLane,
    pub(crate) id: TrackId,
    pub(crate) name: String,
    pub(crate) gain: f32,
    pub(crate) balance: f32,
    pub(crate) muted: bool,
    pub(crate) soloed: bool,
    pub(crate) devices: Vec<Device>,
    pub(crate) instrument: Option<Device>,
    /// Number of serial effects that precede the optional instrument.
    /// `None` exactly when `instrument` is absent.
    pub(crate) instrument_position: Option<usize>,
    pub(crate) content: TrackContent,
    pub(crate) sends: Vec<SendLevel>,
}
impl Track {
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
        self.instrument.as_ref()
    }
    /// Number of serial effects before the instrument, if one is present.
    pub fn instrument_position(&self) -> Option<usize> {
        self.instrument_position
    }
    /// Borrowed device order for clients that need to present the canonical
    /// chain. `Effect` values remain in `devices()` order; the instrument is
    /// yielded at its persisted gap.
    pub fn ordered_device_chain(&self) -> OrderedDeviceChain<'_> {
        OrderedDeviceChain {
            effects: &self.devices,
            instrument: self.instrument.as_ref(),
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
    pub(crate) tuning: hex_tuning::ProjectTuning,
    pub(crate) routing: Vec<ReturnFeed>,
    pub(crate) audio_feeds: Vec<AudioFeed>,
    pub(crate) midi_routes: Vec<MidiRoute>,
    pub(crate) import_provenance: Option<ImportProvenance>,
    pub(crate) sample_rate: u32,
    pub(crate) next_id: u64,
    pub(crate) assets: BTreeMap<AssetId, Asset>,
    pub(crate) tracks: Vec<Track>,
    pub(crate) returns: Vec<ReturnTrack>,
    pub(crate) session: SessionGrid,
    pub(crate) loop_region: Option<LoopRegion>,
    pub(crate) master_gain: f32,
    pub(crate) tempo_map: TempoMap,
    pub(crate) meter_map: MeterMap,
}
impl Project {
    pub fn new(sample_rate: u32) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid("project sample rate must be 8000..192000 hz"));
        }
        Ok(Self {
            tuning: hex_tuning::ProjectTuning::default(),
            routing: Vec::new(),
            audio_feeds: Vec::new(),
            midi_routes: Vec::new(),
            sample_rate,
            next_id: 1,
            import_provenance: None,
            assets: BTreeMap::new(),
            tracks: Vec::new(),
            returns: Vec::new(),
            session: SessionGrid::default(),
            loop_region: None,
            master_gain: 1.0,
            tempo_map: TempoMap::constant(sample_rate, Tempo::new(120_000_000)?)?,
            meter_map: MeterMap::common_time(),
        })
    }
    /// Project-saved tuning and audition settings; no local library identity.
    pub fn tuning(&self) -> hex_tuning::ProjectTuning {
        self.tuning
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
        let state = default_instrument_state()?;
        let (first_id, _) = self.allocate_id_range(missing)?;
        let mut id = first_id;
        for track in &mut self.tracks {
            if track.instrument.is_none() {
                track.instrument = Some(Device {
                    id: DeviceId(id),
                    state: state.clone(),
                    bypassed: false,
                });
                track.instrument_position = Some(0);
                id = id
                    .checked_add(1)
                    .ok_or(Invalid("project id space exhausted"))?;
            }
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
    /// Linear master output gain in the inclusive range 0..=2.
    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }
    /// Canonical piecewise-constant tempo map. Audio clips remain frame-anchored.
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
    pub fn device(&self, id: DeviceId) -> Option<&Device> {
        self.tracks
            .iter()
            .flat_map(|track| track.devices().iter().chain(track.instrument.iter()))
            .chain(self.returns.iter().flat_map(|track| track.devices.iter()))
            .find(|device| device.id() == id)
    }
    pub(crate) fn device_mut(&mut self, id: DeviceId) -> Result<&mut Device, Invalid> {
        self.tracks
            .iter_mut()
            .find_map(|track| {
                track
                    .devices
                    .iter_mut()
                    .find(|device| device.id == id)
                    .or_else(|| track.instrument.as_mut().filter(|device| device.id == id))
            })
            .or_else(|| {
                self.returns
                    .iter_mut()
                    .find_map(|track| track.devices.iter_mut().find(|device| device.id == id))
            })
            .ok_or(Invalid("device no longer exists"))
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
