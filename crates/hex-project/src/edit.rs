use crate::{
    Asset, AssetId, Clip, ClipFade, ClipId, DeviceId, IDENTITY_CONVERSION, Invalid, LoopRegion,
    MAX_ASSETS, MAX_CLIPS, MAX_NAME_BYTES, MAX_TRACKS, MeterPoint, NoteId, Project, ReturnId,
    TempoPoint, Track, TrackContent, TrackId, TrimEdge, converted_frame_count,
    validate_conversion_metadata,
};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDeviceSlot {
    Instrument,
    Effect,
}

/// Validated edits on the control plane. One successful command is one undo step.
#[derive(Debug, Clone)]
pub enum Command {
    /// Initialize active and A/B state as one edit through existing constructors.
    InitializeDevice(Box<crate::DeviceInitialization>),
    /// Copy Arrangement placements while retaining explicit shared content.
    DuplicateAliasClips(Vec<ClipId>),
    /// Duplicate a selected Arrangement group as aliases at one destination.
    /// Refuse any destination that would change an original member.
    DuplicateAliasClipsAt {
        clips: Vec<ClipId>,
        anchor: ClipId,
        track: TrackId,
        position: crate::ClipGroupPosition,
    },
    /// Insert one Arrangement track and place a linked copy in a single edit.
    DuplicateAliasClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        position: crate::ClipGroupPosition,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    /// Detach selected members without copying decoded media.
    MakeClipsUnique(Vec<ClipId>),
    /// Copy current local clip settings to its group, preserving every placement.
    ApplyClipSettingsToAliases(ClipId),
    /// Copy a Console cell to the next empty cell, adding a scene if necessary.
    DuplicateSessionCell {
        track: TrackId,
        scene: crate::SceneId,
        alias: bool,
    },
    /// Register an independent, currently unassigned native source chain.
    /// Exactly one device is designated as the instrument; other devices are
    /// source-owned processing, never copied from a track implicitly.
    CreateNativeSource {
        name: String,
        devices: Vec<hex_lattice_contract::NativeDeviceState>,
        instrument_index: usize,
    },
    /// Link this track to a singleton source, or unlink without deleting it.
    /// Additional linked tracks share the same immutable instrument instance.
    BindTrackSource {
        track: TrackId,
        source: Option<crate::SourceId>,
    },
    /// Select the sole track that emits a source's audio, or leave it unselected.
    /// A selected track must already be linked to this source.
    SetNativeSourceReceiver {
        source: crate::SourceId,
        track: Option<TrackId>,
    },
    RenameNativeSource {
        source: crate::SourceId,
        name: String,
    },
    /// Explicit authored-state deletion. The source must be unlinked from every track.
    RemoveNativeSource(crate::SourceId),
    /// Create one effects-only group around normalized sibling subtrees.
    GroupTracks {
        tracks: crate::TrackSelectionIds,
        name: String,
    },
    /// Remove the group bus, retaining ordered children under its former parent.
    UngroupTrack(TrackId),
    /// Move complete subtrees to an explicitly captured parent/child boundary.
    ReparentTracks {
        tracks: crate::TrackSelectionIds,
        destination: crate::TrackDestination,
    },
    /// Duplicate the complete selected subtrees after their last source root.
    DuplicateTracks {
        tracks: crate::TrackSelectionIds,
        without_clips: bool,
    },
    /// Adopt all prepared track bounces as one reversible edit. No file I/O.
    /// `adoption` deactivates the bounced clip/range sources in that same
    /// edit with the contextual activation commands; new-track adoption only.
    CommitArrangementBounce {
        target: crate::BounceTarget,
        in_place: bool,
        adoption: crate::BounceAdoption,
        files: Vec<crate::BounceFile>,
    },
    /// Set one authored clip's activation across Arrangement or Session.
    SetClipActive {
        clip: ClipId,
        active: bool,
    },
    /// Split only intersected Arrangement clips at the selected frame edges and
    /// set the selected spans atomically, preserving both exterior fragments.
    SetTimeRangeActive {
        range: crate::TimeRange,
        active: bool,
    },
    /// Apply one clip-wide value to a frozen Arrangement selection atomically.
    EditClips {
        clips: Vec<ClipId>,
        edit: crate::ClipEdit,
    },
    /// Paint a structural Main-lane marker; no musical content is created.
    AddArrangementSection {
        name: String,
        color: crate::ElementColor,
        start: crate::BeatTime,
        end: crate::BeatTime,
    },
    /// Isolated marker range edit; overlaps are refused and music stays fixed.
    SetArrangementSectionRange {
        section: crate::SectionId,
        start: crate::BeatTime,
        end: crate::BeatTime,
    },
    /// Move one marker without changing its length or any musical content.
    /// Destination overlap trims/removes other markers; an interior cut retains
    /// their left identity and allocates a fresh right identity/name/color copy.
    MoveArrangementSectionMarker {
        section: crate::SectionId,
        start: crate::BeatTime,
    },
    /// Resize one marker edge, carving overlapping markers without editing music.
    /// Surviving fragments follow the same identity law as marker movement.
    ResizeArrangementSectionMarker {
        section: crate::SectionId,
        edge: TrimEdge,
        boundary: crate::BeatTime,
    },
    RenameArrangementSection {
        section: crate::SectionId,
        name: String,
    },
    SetArrangementSectionColor {
        section: crate::SectionId,
        color: crate::ElementColor,
    },
    /// Remove the marker only, preserving all musical content and song time.
    RemoveArrangementSection(crate::SectionId),
    /// Insert empty time or remove edge time song-wide, keeping marker start fixed.
    /// Left edits its prefix; Right edits its suffix. No time stretching occurs.
    ResizeArrangementSection {
        section: crate::SectionId,
        edge: TrimEdge,
        length: crate::BeatTime,
    },
    /// Resize one edge in place. Crossed time becomes empty; other section
    /// positions and the remaining song timeline stay fixed. No stretching.
    ResizeArrangementSectionEdge {
        section: crate::SectionId,
        edge: TrimEdge,
        boundary: crate::BeatTime,
    },
    /// Move the seam of two touching sections, preserving their outer span.
    /// Content mode removes time from the shrinking edge and inserts empty time
    /// at the growing edge in one history/publication operation. Marker-only mode
    /// preserves all musical data. Both sections must retain positive lengths.
    ResizeArrangementSectionBoundary {
        left: crate::SectionId,
        right: crate::SectionId,
        boundary: crate::BeatTime,
        marker_only: bool,
    },
    /// Remove the section's entire half-open time interval and close the gap.
    DeleteArrangementSection(crate::SectionId),
    /// Insert a song-wide copy immediately after the source, allocating new IDs.
    DuplicateArrangementSection(crate::SectionId),
    /// Copy a section's complete song interval before/after a stable target.
    /// The source remains; one private candidate owns both copy and insertion,
    /// fresh identities, publication and Undo/Redo. The target may be the source.
    DuplicateArrangementSectionAt {
        section: crate::SectionId,
        target: crate::SectionId,
        after: bool,
    },
    /// Copy a complete song section at an absolute beat at/after the final marker.
    /// A nonempty gap receives the supplied validated marker name/color. The
    /// original song remains; copy and gap share one preparation/history edge.
    AppendArrangementSectionCopy {
        section: crate::SectionId,
        start: crate::BeatTime,
        gap_name: String,
        gap_color: crate::ElementColor,
    },
    /// Cut a complete song section and place it at an absolute final beat
    /// strictly beyond the current last marker, preserving its identity.
    /// Close the old interval, then mark the gap after the remaining markers
    /// (or zero for a sole section). Unmarked music is retained. Move and gap
    /// share one preparation/history edge; equal-end placement uses reorder.
    MoveArrangementSectionBeyondEnd {
        section: crate::SectionId,
        start: crate::BeatTime,
        gap_name: String,
        gap_color: crate::ElementColor,
    },
    /// Replace a target's complete song interval with source content. Moves
    /// close the old source interval and retain its ID; copies preserve it and
    /// allocate a new result ID. Both carry source name/color and retire target.
    /// All non-replaced music ripples intact. Self replacement is a no-op;
    /// one private candidate owns preparation, refusal and Undo/Redo.
    ReplaceArrangementSection {
        source: crate::SectionId,
        target: crate::SectionId,
        copy: bool,
    },
    /// Split marker metadata at a strict interior beat, preserving all music.
    /// The left marker retains its ID; the right gets a fresh ID and the same
    /// name/color. One validated command owns both adjoining positive ranges.
    SplitArrangementSection {
        section: crate::SectionId,
        at: crate::BeatTime,
    },
    /// Merge the earliest-to-latest selected marker span, including intervening
    /// markers/gaps, retaining earliest identity/name/color and all music.
    MergeArrangementSections(Vec<crate::SectionId>),
    /// One song-wide cut-and-insert permutation, resolved against stable IDs.
    ReorderSectionBefore {
        section: crate::SectionId,
        before: crate::SectionId,
    },
    /// One song-wide cut-and-insert permutation after a stable target ID.
    ReorderSectionAfter {
        section: crate::SectionId,
        after: crate::SectionId,
    },
    /// Replace validated project tuning and audition settings in one history step.
    SetTuning(hex_tuning::ProjectTuning),
    /// Event IDs come from the clip/revision-scoped editable_audio_events view.
    EditAudioEvents {
        clip: ClipId,
        operation: crate::AudioEventEdit,
    },
    /// Rename any Arrangement or Session clip by stable ID.
    RenameClip {
        clip: ClipId,
        name: String,
    },
    SetTrackColor {
        track: TrackId,
        color: crate::ElementColor,
    },
    SetClipFlavor {
        clip: ClipId,
        flavor: crate::ClipFlavor,
    },
    /// Unsupported transcription request; refuses without changing content or history.
    ConvertAudioClipToMidi(ClipId),
    /// Adopt a rendered project-rate WAV for a MIDI clip, retaining identity.
    ConvertMidiClipToAudio {
        clip: ClipId,
        path: PathBuf,
        frames: u64,
    },
    /// One prepared history operation creates a normal hybrid track with a device.
    InsertTrackWithDevice {
        name: String,
        before_track: Option<TrackId>,
        slot: TrackDeviceSlot,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Like `InsertTrackWithDevice`, with an automatic name: `device_name` for an
    /// instrument slot, otherwise the model-owned default name.
    InsertDefaultTrackWithDevice {
        before_track: Option<TrackId>,
        slot: TrackDeviceSlot,
        state: hex_lattice_contract::NativeDeviceState,
        device_name: String,
    },
    /// Create an automatically named device-bearing track with finite linear gain in `0..=2`.
    InsertDefaultTrackWithDeviceAndGain {
        before_track: Option<TrackId>,
        slot: TrackDeviceSlot,
        state: hex_lattice_contract::NativeDeviceState,
        device_name: String,
        gain: f32,
    },
    /// Atomically splice a bounded touch span, preserving the underlying base curve.
    SpliceGainTouch {
        track: TrackId,
        expected_epoch: u64,
        span: crate::GainTouchSpan,
    },
    /// One completed take, including loop passes, commits as one history edit.
    SpliceGainTouchBatch {
        track: TrackId,
        expected_epoch: u64,
        spans: Vec<crate::GainTouchSpan>,
    },
    ReplaceTrackGainEnvelope {
        track: TrackId,
        points: Vec<crate::GainPoint>,
    },
    UpsertTrackGainPoint {
        track: TrackId,
        point: crate::GainPoint,
    },
    MoveTrackGainPoint {
        track: TrackId,
        from: crate::BeatTime,
        to: crate::BeatTime,
    },
    RemoveTrackGainPoint {
        track: TrackId,
        beat: crate::BeatTime,
    },
    /// Clear base and recorded overlays, retaining the recording epoch high-water mark.
    ClearTrackGainEnvelope {
        track: TrackId,
    },
    /// Validate proposed metadata, then refuse non-legacy adoption until playback support.
    SetRouting(Vec<crate::RoutingEdge>),
    /// Add/change a post-fader stereo feed to a track input; zero removes it.
    SetAudioFeed {
        source: crate::RoutingOwner,
        destination: TrackId,
        gain: f32,
    },
    /// Select a post-mixer key track. None returns to the internal detector.
    SetSidechain {
        device: DeviceId,
        source: Option<TrackId>,
    },
    /// Select authored MIDI from another track; None restores local MIDI.
    SetMidiSource {
        destination: TrackId,
        source: Option<TrackId>,
    },
    /// Adopt or change a bounded additive stereo Return feed; zero removes it.
    SetReturnFeed {
        source: ReturnId,
        destination: ReturnId,
        gain: f32,
    },
    /// Insert an opaque effect state on the singleton Main bus. Preparation
    /// validates provider class before a Session history transaction commits.
    InsertMainDevice {
        before: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Reorder within Main; cross-owner moves retain their existing commands.
    MoveMainDevice {
        device: DeviceId,
        before: Option<DeviceId>,
    },
    InsertDevice {
        track: TrackId,
        before: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Fill a removed native instrument slot with a new stable device identity.
    InsertInstrument {
        track: TrackId,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Browser instrument load: fill the empty slot (`replacing: None`) or replace
    /// the captured current instrument, and rename an automatic-named track to
    /// `device_name` in the same edit. See `track_name` for the naming policy.
    LoadInstrument {
        track: TrackId,
        replacing: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
        device_name: String,
    },
    RemoveInstrument {
        track: TrackId,
    },
    RemoveDevice(DeviceId),
    MoveDevice {
        device: DeviceId,
        track: TrackId,
        before: Option<DeviceId>,
    },
    /// Move any effect or the optional instrument in the canonical ordered
    /// chain. Effects and an instrument can cross tracks when the destination
    /// has capacity and, for an instrument, an empty slot. `before` may identify
    /// either node class on the destination; `None` appends.
    MoveTrackDevice {
        device: DeviceId,
        track: TrackId,
        before: Option<DeviceId>,
    },
    /// Move a complete unit within one retained source, keeping its parent.
    /// The designated instrument moves with its children. None appends; invalid
    /// membership placement must stay inside the owning chain.
    MoveNativeSourceDevice {
        source: crate::SourceId,
        device: DeviceId,
        before: Option<DeviceId>,
    },
    /// Create a fresh last child of a local/Main/Return effect or an unbound
    /// retained source parent in one candidate. Bound source edits still refuse.
    InsertNestedDevice {
        parent: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Attach a complete local/Main/Return effect unit as the parent's last
    /// child. None detaches it immediately after its former top-level unit.
    /// Unbound source units use the same rule. An unchanged parent is a no-op;
    /// bound source edits and foreign parents refuse.
    ReparentDevice {
        device: DeviceId,
        parent: Option<DeviceId>,
    },
    /// Splice half-open touches in one history edge. VST3 targets require the
    /// exact captured class; native/host targets require no class stamp.
    SpliceDeviceAutomationTouch {
        device: DeviceId,
        target: crate::DeviceAutomationTarget,
        vst3_class: Option<[u8; 16]>,
        spans: Vec<crate::DeviceAutomationTouchSpan>,
    },
    /// Adopt a complete group of two to eight distinct raw VST3 ParamIDs on
    /// one exact device and class. Lanes are ordered by raw ID, including zero.
    SpliceVst3AutomationTouchGroup {
        device: DeviceId,
        class: [u8; 16],
        lanes: Vec<(u32, Vec<crate::DeviceAutomationTouchSpan>)>,
    },
    /// Replace a complete validated module/route bank in one history edge.
    ReplaceDeviceModulation {
        device: DeviceId,
        modulation: crate::DeviceModulation,
    },
    ReplaceDeviceAutomationTarget {
        device: DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
        points: Vec<crate::AutomationPoint>,
    },
    /// Provider-parameter convenience; host trims use the typed target command.
    ReplaceDeviceAutomation {
        device: DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        points: Vec<crate::AutomationPoint>,
    },
    /// One explicit manual edit selects base only for touched retained lanes.
    /// Ordinary state/preset/comparison replacements retain their read selection.
    ManualDeviceEdit {
        device: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
        targets: Vec<hex_lattice_contract::DeviceAutomationTarget>,
    },
    SetDeviceAutomationReadSelection {
        device: DeviceId,
        target: hex_lattice_contract::DeviceAutomationTarget,
        selection: crate::AutomationReadSelection,
    },
    /// Resume every retained device envelope in one canonical history edit.
    ReenableDeviceAutomation,
    ReplaceDeviceState {
        device: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Atomically replace opaque device state and reserve fresh identities.
    /// Expected source, active A/B slot and allocator cursor prevent stale UI edits overwriting
    /// another edit; history preserves the allocator high-water mark on undo.
    ReplaceDeviceStateWithIdentityReservation {
        /// None preserves inventory; Some atomically replaces it after an exact
        /// authored-inventory comparison, without inferring provider-ID mappings.
        automation: Option<crate::DeviceAutomationReplacement>,
        device: DeviceId,
        expected_state: hex_lattice_contract::NativeDeviceState,
        expected_variant: crate::DeviceVariant,
        state: hex_lattice_contract::NativeDeviceState,
        expected_next_id: u64,
        reserved_ids: u64,
    },
    SetDeviceBypass {
        device: DeviceId,
        bypassed: bool,
    },
    /// Toggle the device's current canonical bypass state as one history edit.
    ToggleDeviceBypass {
        device: DeviceId,
    },
    /// Compare provider settings without changing bypass, routing or host trims.
    CompareDevice {
        device: DeviceId,
        expected: crate::DeviceVariant,
        action: crate::DeviceComparisonAction,
    },
    /// Restore a validated complete comparison pair (for current-format presets).
    ReplaceDeviceComparison {
        device: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
        comparison: Option<crate::DeviceComparison>,
    },
    AddReturnTrack {
        name: String,
    },
    /// Create a return bus with finite linear gain in `0..=2` in one edit.
    AddReturnTrackWithGain {
        name: String,
        gain: f32,
    },
    RemoveReturnTrack(ReturnId),
    RenameReturnTrack {
        return_track: ReturnId,
        name: String,
    },
    ReturnGain {
        return_track: ReturnId,
        gain: f32,
    },
    ReturnMute {
        return_track: ReturnId,
        muted: bool,
    },
    /// Zero removes the sparse send; positive levels use the linear 0..2 domain.
    SetReturnSend {
        track: TrackId,
        return_track: ReturnId,
        gain: f32,
    },
    InsertReturnDevice {
        return_track: ReturnId,
        before: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
    },
    MoveReturnDevice {
        device: DeviceId,
        before: Option<DeviceId>,
    },
    DeleteTimeRange {
        range: crate::TimeRange,
    },
    PasteTimeRange {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    /// Remove one validated chain selection as one history edit.
    RemoveDevices {
        owner: crate::DeviceChainOwner,
        selected: crate::DeviceSelectionIds,
    },
    /// Insert captured devices with fresh identities and complete authored state.
    PasteDevices {
        clipboard: std::sync::Arc<crate::DeviceClipboard>,
        owner: crate::DeviceChainOwner,
        before: Option<DeviceId>,
    },
    /// Insert complete captured tracks in canonical order with fresh identities.
    PasteTracks {
        clipboard: std::sync::Arc<crate::TrackClipboard>,
        destination: crate::TrackDestination,
    },
    /// Paste complete clips, retaining relative timing and stable multi-track owners.
    PasteClips {
        clipboard: std::sync::Arc<crate::ClipClipboard>,
        track: TrackId,
        frame: u64,
        insert: bool,
    },
    /// Keyboard paste that preserves original track IDs for multi-row clipboards.
    PasteTimeRangeOnSourceTracks {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    InsertTimeRange {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    /// Keyboard ripple paste that preserves original track IDs for multi-row clipboards.
    InsertTimeRangeOnSourceTracks {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    MoveTimeRange {
        range: crate::TimeRange,
        track: TrackId,
        frame: u64,
    },
    /// Copy a selected time rectangle to a destination in one undoable edit.
    CopyTimeRangeTo {
        range: crate::TimeRange,
        track: TrackId,
        frame: u64,
    },
    AddTrack {
        name: String,
    },
    /// Insert an empty hybrid track before a stable anchor, or append with None.
    InsertTrack {
        name: String,
        before_track: Option<TrackId>,
    },
    /// Insert an ordinary track named by the model (`track N`); the name stays
    /// automatic until the user renames it.
    InsertDefaultTrack {
        before_track: Option<TrackId>,
    },
    /// Insert an automatically named track with a validated initial linear gain.
    InsertDefaultTrackWithGain {
        before_track: Option<TrackId>,
        gain: f32,
    },
    /// Atomically create one track, its source asset and a full-length one-shot clip.
    ImportAudioTrack {
        name: String,
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
    },
    /// Atomically import original source metadata and project-rate prepared PCM.
    ImportPreparedAudioTrack {
        name: String,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Atomically insert a new hybrid track before a stable track id (or at the
    /// end when `before_track` is `None`) and place the prepared audio at `start`.
    ImportPreparedAudioTrackAt {
        name: String,
        before_track: Option<TrackId>,
        start: u64,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Atomically import prepared media and place a full-length clip on an existing hybrid track.
    ImportPreparedAudioClip {
        track: TrackId,
        start: u64,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Import prepared audio and cut its destination interval from both clip
    /// collections on the target track as one drop-specific edit.
    ImportPreparedAudioClipWithOverwrite {
        track: TrackId,
        start: u64,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Atomically add prepared media and place a one-shot source in a Session cell.
    ImportPreparedSessionAudioClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
        source_start: u64,
        source_end: u64,
    },
    /// Place an existing ordinary, online project asset as one import member,
    /// following the same placement law as the new-asset import commands
    /// without adding an asset. See `crate::import_existing`.
    ImportExistingAudio {
        asset: AssetId,
        placement: crate::ExistingAudioPlacement,
    },
    /// Move a complete ordinary track before a stable anchor, or to the end
    /// with None. Already adjacent, self, and already-last drops are no-ops.
    /// Returns use distinct ReturnId owners and remain in their own partition;
    /// Main is not a Track. Neither can be addressed by this typed command.
    MoveTrackBefore {
        track: TrackId,
        before: Option<TrackId>,
    },
    /// Move complete selected ordinary tracks as one block in canonical order.
    /// A selected anchor or an already-adjacent block is a no-op. Identity-owned
    /// content, routing and automation remain attached; one history edit owns it.
    MoveTracksBefore {
        tracks: crate::TrackSelectionIds,
        before: Option<TrackId>,
    },
    RemoveTrack(TrackId),
    /// Remove explicit tracks through the existing cleanup as one history edit.
    RemoveTracks(Vec<TrackId>),
    SetTrackTags {
        track: TrackId,
        tags: Vec<crate::TrackTag>,
    },
    RenameTrack {
        track: TrackId,
        name: String,
    },
    Gain {
        track: TrackId,
        gain: f32,
    },
    /// Change a bounded set of static track gains atomically in one history step.
    SetTrackGains {
        gains: Vec<(TrackId, f32)>,
    },
    /// Set original Hex stereo balance; this is not a measured Live pan law.
    Balance {
        track: TrackId,
        balance: f32,
    },
    MasterGain {
        gain: f32,
    },
    /// Replace all project tempo points and retain musical clip onsets.
    /// Audio keeps its source processing; colliding tails are non-destructively
    /// cropped at the next placed clip's onset, preserving all clip identities.
    SetProjectTempo {
        tempo: crate::Tempo,
    },
    SetTempoPoint {
        beat: crate::BeatTime,
        tempo: crate::Tempo,
    },
    SetMeterPoint {
        beat: crate::BeatTime,
        meter: crate::Meter,
    },
    Mute {
        track: TrackId,
        muted: bool,
    },
    /// Toggle track solo. Exclusive solo clears other solos only when enabling.
    ToggleSolo {
        track: TrackId,
        exclusive: bool,
    },
    /// Toggle one frozen selection atomically. Mixed states enable every member;
    /// all-soloed members disable together. Exclusive enabling clears outsiders,
    /// while disabling preserves them. Stale membership refuses before mutation.
    ToggleSoloTracks {
        tracks: crate::TrackSelectionIds,
        exclusive: bool,
    },
    SetLoopRegion {
        start: u64,
        end: u64,
        enabled: bool,
    },
    ToggleLoopRegion,
    ClearLoopRegion,
    /// Add source metadata for PCM that is already at the project rate.
    AddAsset {
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
    },
    /// Original device source. Conversion metadata describes the expected clip
    /// playback shape; this command does not materialize a converted derivative.
    ImportDeviceAsset {
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
    },
    /// One prepared import creates source metadata and fills an empty instrument
    /// slot. The neutral state template has no asset references; the new source
    /// ID is bound inside this candidate, never predicted by a UI caller.
    ImportDeviceAssetIntoEmptyInstrument {
        track: TrackId,
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
        state: hex_lattice_contract::NativeDeviceState,
        source_slot: hex_lattice_contract::DeviceAssetSlot,
    },
    /// Atomically import an original source and bind or rebind an existing instrument.
    /// Captured identity/state/provider must still match; no IDs are predicted.
    ImportDeviceAssetReplacingInstrument {
        track: TrackId,
        device: DeviceId,
        expected_state: hex_lattice_contract::NativeDeviceState,
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
        state: hex_lattice_contract::NativeDeviceState,
        source_slot: hex_lattice_contract::DeviceAssetSlot,
    },
    RemoveAsset(AssetId),
    /// Point offline assets at found files as one history step, bringing them
    /// back online. Undo returns them to offline at their old paths. Paths must
    /// be absolute; only offline, non-device assets can be relinked, and callers
    /// must have prepared media matching the saved rate and frame count.
    RelinkAssets(Vec<(AssetId, PathBuf)>),
    /// Place audio as one atomic hybrid overwrite, retaining old outer fragments.
    PlaceClip {
        track: TrackId,
        asset: AssetId,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
    },
    /// Place a new clip after the selected clip using its same media interval.
    DuplicateClip(ClipId),
    /// Copy a frozen whole-clip selection after its complete extent, atomically.
    DuplicateClips(Vec<ClipId>),
    /// Wrap one same-track, same-kind selection atomically; never replace nonselected material.
    GroupClips(Vec<ClipId>),
    /// Split selection edges and group included material independently per track,
    /// retaining the full selected extent including empty margins and internal gaps.
    /// Empty tracks are skipped; returns the last populated track's resulting clip.
    /// All fragments and groups form one atomic edit; mixed media on a row refuses.
    GroupTimeRange(crate::TimeRange),
    /// Extract a selected root-local audio span into one clip, retaining outer fragments.
    GroupAudioEventRange {
        clip: ClipId,
        start: u64,
        end: u64,
    },
    /// Reverse finite audible content atomically, preserving sources and edit settings.
    ReverseClips(Vec<ClipId>),
    /// Split a clip at an absolute timeline frame.
    SplitClip {
        clip: ClipId,
        frame: u64,
    },
    /// Split every Arrangement clip crossing one absolute frame on the given
    /// tracks as one atomic edit.
    SplitTracksAtFrame {
        tracks: Vec<TrackId>,
        frame: u64,
    },
    /// Split both boundaries of a finite Arrangement range as one edit. Existing
    /// boundaries and gaps are no-ops; authored material and global time stay put.
    SplitTimeRange {
        range: crate::TimeRange,
    },
    /// Trim an absolute timeline edge, clamping to the nearest valid clip region.
    TrimClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    /// Resize an Arrangement audio clip with its lower fade endpoint as one edit.
    TrimClipFadeEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    /// Move audio as one atomic hybrid overwrite, preserving its original identity.
    MoveClip {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Drop spelling of MoveClip; both use the same final-row overwrite plan.
    MoveClipWithOverwrite {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Move/copy one selected Arrangement group with a single prepared history edit.
    MoveClipsWithOverwrite {
        clips: Vec<ClipId>,
        anchor: ClipId,
        track: TrackId,
        position: crate::ClipGroupPosition,
        copy: bool,
    },
    /// Atomically insert a hybrid track before a stable anchor/end and move an
    /// existing audio clip there, preserving its ID and source metadata.
    MoveClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: u64,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    /// Copy an Arrangement audio clip to a destination, replacing overlap atomically.
    CopyClipTo {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Copy an Arrangement audio clip onto a newly inserted hybrid track.
    CopyClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: u64,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    /// Explicit source interval; changing the source interval resets loop phase to zero.
    TrimClip {
        clip: ClipId,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
    },
    /// Fit an edge fade or a two-voice source-handle crossfade to a validated
    /// Arrangement selection. Source content never translates; failures are atomic.
    CreateArrangementFades {
        range: crate::TimeRange,
    },
    /// Fit one source-event edge fade. Positions use the retained root's declared
    /// frame/beat clock; Console source windows and event joins explicitly refuse.
    CreateAudioEventFades {
        clip: ClipId,
        start: u64,
        end: u64,
    },
    /// Set linear fade lengths and reanchor them to the clip's current duration.
    SetClipFades {
        clip: ClipId,
        fade_in: u64,
        fade_out: u64,
    },
    /// Reanchor a complete fade envelope to the current Arrangement clip.
    SetClipFadeEnvelope {
        clip: ClipId,
        fade: ClipFade,
    },
    /// Set Arrangement or Session audio clip gain, in -60_000..=24_000 millidecibels.
    SetAudioClipGain {
        clip: ClipId,
        millidb: i32,
    },
    /// Integer cents; resampling is root-anchored and preserves placement duration.
    SetAudioClipPitch {
        clip: ClipId,
        cents: i16,
    },
    SetAudioClipMusical {
        clip: ClipId,
        enabled: bool,
    },
    /// Change the declared original sample tempo while Warp is active.
    SetAudioClipSampleBpm {
        clip: ClipId,
        tempo: crate::Tempo,
    },
    /// Affine retiming of the retained source map; opposite visible edge stays fixed.
    StretchAudioClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    /// Recompute a clip-local loop or playback-start operation from current source.
    /// Outer growth carves both clip kinds; cycle edits retain outer occupancy.
    ResizeAudioClipView {
        clip: ClipId,
        operation: crate::AudioViewResizeOperation,
    },
    SetAudioClipWarp {
        clip: ClipId,
        settings: Option<crate::WarpSettings>,
    },
    SetAudioClipSourceGain {
        clip: ClipId,
        points: Vec<crate::SourceGainPoint>,
    },
    /// Enable or disable the audio clip's source loop; disabling resets its phase.
    SetClipLooping {
        clip: ClipId,
        looping: bool,
    },
    /// Set a looping audio clip's source-relative play position.
    SetClipSourcePhase {
        clip: ClipId,
        phase: u64,
    },
    RemoveClip(ClipId),
    /// Relative commands are resolved against canonical state, so queued repeats
    /// do not overwrite one another using an outdated UI snapshot.
    NudgeClip {
        clip: ClipId,
        frames: i64,
    },
    TrimClipEndBy {
        clip: ClipId,
        frames: i64,
    },
    GainBy {
        track: TrackId,
        amount: f32,
    },
    BalanceBy {
        track: TrackId,
        amount: f32,
    },
    MasterGainBy {
        amount: f32,
    },
    ToggleMute(TrackId),
    /// Mute every explicit member if any is unmuted; otherwise unmute all.
    /// Reject stale membership before mutation; one command is one undo step.
    ToggleTracksMute(crate::TrackSelectionIds),
    AddMidiTrack {
        name: String,
    },
    AddScene {
        name: String,
    },
    /// Insert before an existing stable scene, or explicitly append with None.
    InsertScene {
        name: String,
        before: Option<crate::SceneId>,
    },
    /// Copy a scene immediately after it with fresh IDs for copied MIDI content.
    DuplicateScene(crate::SceneId),
    /// Reorder one scene before a stable target, or append it when `before` is `None`.
    MoveScene {
        scene: crate::SceneId,
        before: Option<crate::SceneId>,
    },
    RenameScene {
        scene: crate::SceneId,
        name: String,
    },
    RemoveScene(crate::SceneId),
    CreateSessionMidiClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        source_length: crate::BeatTime,
    },
    CreateSessionAudioClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        asset: AssetId,
        source_start: u64,
        source_end: u64,
    },
    CopyArrangementAudioClipToSession {
        source: ClipId,
        scene: crate::SceneId,
    },
    CopyArrangementMidiToSession {
        source: ClipId,
        scene: crate::SceneId,
    },
    /// Create one normal track and optional scene, then copy through the same
    /// Session law. Editor prepares the whole candidate and commits one history edge.
    CopyArrangementAudioClipToNewSessionTrack {
        source: ClipId,
        before_track: Option<TrackId>,
        scene: crate::SessionSceneTarget,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    CopyArrangementMidiToNewSessionTrack {
        source: ClipId,
        before_track: Option<TrackId>,
        scene: crate::SessionSceneTarget,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    MoveSessionCell {
        from_track: TrackId,
        from_scene: crate::SceneId,
        to_track: TrackId,
        to_scene: crate::SceneId,
    },
    DuplicateSessionMidiCell {
        track: TrackId,
        from_scene: crate::SceneId,
        to_scene: crate::SceneId,
    },
    DuplicateSessionAudioCell {
        track: TrackId,
        from_scene: crate::SceneId,
        to_scene: crate::SceneId,
    },
    /// Change a Console audio cycle without replacing its retained PCM.
    SetSessionAudioClipLoop {
        clip: ClipId,
        loop_range: Option<crate::SessionAudioLoop>,
    },
    SetSessionAudioClipSource {
        clip: ClipId,
        asset: AssetId,
        source_start: u64,
        source_end: u64,
    },
    ClearSessionCell {
        track: TrackId,
        scene: crate::SceneId,
    },
    /// Explicitly replaces any existing cell content with a stop marker.
    SetSessionStopCell {
        track: TrackId,
        scene: crate::SceneId,
    },
    SetSessionMidiClipSource {
        clip: ClipId,
        source_length: crate::BeatTime,
        source_start: crate::BeatTime,
        loop_start: crate::BeatTime,
        loop_end: crate::BeatTime,
        looping: bool,
    },
    RenameSessionMidiClip {
        clip: ClipId,
        name: String,
    },
    PlaceMidiClip {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
    },
    /// Paint an empty MIDI clip, cutting existing audio/MIDI at its boundaries.
    PlaceMidiClipWithOverwrite {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
    },
    /// Resize a MIDI Arrangement edge without deleting authored source notes.
    TrimMidiClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        beat: crate::BeatTime,
    },
    /// Scale retained MIDI source timing around the opposite Arrangement edge.
    /// Collapsed notes/volume boundaries and overflow refuse the complete edit.
    StretchMidiClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        beat: crate::BeatTime,
    },
    /// Split a finite MIDI Arrangement clip at an absolute musical beat.
    SplitMidiClip {
        clip: ClipId,
        beat: crate::BeatTime,
    },
    /// Atomically replace the finite source and arrangement region for a MIDI clip.
    SetMidiClipSource {
        clip: ClipId,
        source_length: crate::BeatTime,
        source_start: crate::BeatTime,
        loop_start: crate::BeatTime,
        loop_end: crate::BeatTime,
        looping: bool,
        length: crate::BeatTime,
    },
    /// Recompute a clip-local MIDI loop edit from its current source/window.
    /// Outer growth uses the common occupancy planner; cycle edits keep length.
    ResizeMidiClipView {
        clip: ClipId,
        operation: crate::MidiViewResizeOperation,
    },
    /// Replace one clip-owned source-beat volume curve in either MIDI domain.
    ReplaceMidiClipVolume {
        clip: ClipId,
        volume: crate::MidiClipVolume,
    },
    /// Replace the clip-local scalar independently of its source-beat curve.
    SetMidiClipGain {
        clip: ClipId,
        gain: crate::ClipGain,
    },
    /// Author one note using the same source/pitch overwrite rule as `AddMidiNotes`.
    AddMidiNote {
        clip: ClipId,
        start: crate::BeatTime,
        duration: crate::BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    },
    /// Atomically author several notes in one MIDI source as one history step.
    /// Later input wins on the same pitch across channels, retaining only an
    /// older positive left prefix. Only surviving authors receive new IDs.
    AddMidiNotes {
        clip: ClipId,
        notes: Vec<MidiNoteInsert>,
    },
    /// Atomically remove and author notes in one MIDI source as one history step.
    /// Insertions use the same destructive same-pitch rule as `AddMidiNotes`.
    EditMidiNotes {
        clip: ClipId,
        remove: Vec<NoteId>,
        insert: Vec<MidiNoteInsert>,
    },
    /// Author notes beyond the current source extent in one atomic edit/history step.
    /// Only source length may grow, exactly to the furthest inserted note end;
    /// Arrangement placement, source start and loop markers remain unchanged.
    /// Existing strict note commands continue to refuse notes outside the source.
    EditMidiNotesWithSourceExtent {
        clip: ClipId,
        remove: Vec<NoteId>,
        insert: Vec<MidiNoteInsert>,
    },
    /// Duplicate positive intersections with a source-local half-open time range
    /// immediately after that range. Stored note order determines fresh-ID order;
    /// canonical same-pitch overwrite applies at the destination. Source length
    /// grows through the complete destination range, including trailing silence.
    /// Playback start, loop markers and Arrangement placement remain unchanged.
    /// The clip volume interval follows the same copy/overwrite in that one edit.
    /// Empty notes are a no-op only when neither extent nor volume changes.
    DuplicateMidiSourceRange {
        clip: ClipId,
        start: crate::BeatTime,
        end: crate::BeatTime,
    },
    /// Move/resize one stable note, overwriting older same-pitch intersections.
    UpdateMidiNote {
        note: NoteId,
        start: crate::BeatTime,
        duration: crate::BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    },
    /// Atomically update several notes in one MIDI clip as one history step.
    /// Exclude all selected originals together, then resolve authored geometry
    /// in input order using the same overwrite rule as `AddMidiNotes`.
    /// Surviving notes keep their original IDs and source order.
    UpdateMidiNotes {
        clip: ClipId,
        notes: Vec<MidiNoteUpdate>,
    },
    /// Update stable note identities, growing only the source extent to their
    /// requested ends. Arrangement placement and playback/loop markers stay
    /// fixed; ordinary UpdateMidiNotes retains its strict source boundary.
    UpdateMidiNotesWithSourceExtent {
        clip: ClipId,
        notes: Vec<MidiNoteUpdate>,
    },
    /// Translate an Arrangement MIDI source by a positive prefix, then apply
    /// stable note updates in the rebased storage coordinates. At least one
    /// requested onset must be inside the new prefix. Placement, direction and
    /// elapsed windows stay fixed, including those of linked clips.
    UpdateMidiNotesWithSourcePrefix {
        clip: ClipId,
        prefix: crate::BeatTime,
        notes: Vec<MidiNoteUpdate>,
    },
    /// Atomically enable or disable existing notes without changing their geometry.
    SetMidiNotesActive {
        clip: ClipId,
        notes: Vec<NoteId>,
        active: bool,
    },
    RemoveMidiNote(NoteId),
    /// Atomically remove several notes in one MIDI clip as one history step.
    RemoveMidiNotes {
        clip: ClipId,
        notes: Vec<NoteId>,
    },
    MoveMidiClip {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    /// Drop-specific MIDI move that carves the destination beat interval out
    /// of both Arrangement clip collections before placing this MIDI clip.
    MoveMidiClipWithOverwrite {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    /// Atomically insert a hybrid track before a stable anchor/end and move an
    /// existing MIDI clip there, preserving its ID and note/source data.
    MoveMidiClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: crate::BeatTime,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    /// Copy an Arrangement MIDI clip to a destination, replacing overlap atomically.
    CopyMidiClipTo {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    /// Copy an Arrangement MIDI clip onto a newly inserted hybrid track.
    CopyMidiClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: crate::BeatTime,
        /// Optional finite linear gain in `0..=2`; `None` retains the normal track default.
        initial_gain: Option<f32>,
    },
    RemoveMidiClip(ClipId),
    DuplicateMidiClip(ClipId),
}

/// Complete canonical field replacement for one existing MIDI note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteUpdate {
    pub note: NoteId,
    pub start: crate::BeatTime,
    pub duration: crate::BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

/// Musical fields for one newly inserted MIDI note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteInsert {
    pub active: bool,
    pub start: crate::BeatTime,
    pub duration: crate::BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    NativeSource(crate::SourceId),
    ArrangementSection(crate::SectionId),
    Track(TrackId),
    /// Inclusive track-only ID range born in one atomic paste.
    Tracks {
        first: TrackId,
        last: TrackId,
    },
    ReturnTrack(ReturnId),
    Device(DeviceId),
    Asset(AssetId),
    Clip(ClipId),
    /// Actual Arrangement clip births in this inclusive range are copies from
    /// one group command. Filter committed clips; note-ID holes are permitted.
    Clips {
        first: ClipId,
        last: ClipId,
    },
    Note(NoteId),
    Scene(crate::SceneId),
    SessionClip(ClipId),
    /// Inclusive contiguous note-ID range created in `clip`; select IDs in the
    /// current clip whose values fall within this range. Only authored members
    /// surviving this atomic edit consume IDs, in their original input order.
    Notes {
        clip: ClipId,
        first: NoteId,
        last: NoteId,
    },
    Imported {
        track: TrackId,
        asset: AssetId,
        clip: ClipId,
    },
    None,
}

pub(crate) fn name_valid(name: &str) -> Result<(), Invalid> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        Err(Invalid(
            "name must be nonempty, without control characters, and at most 1024 bytes",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn insert_track_before(
    project: &mut Project,
    name: String,
    before_track: Option<TrackId>,
) -> Result<TrackId, Invalid> {
    name_valid(&name)?;
    if project.tracks.len() >= MAX_TRACKS {
        return Err(Invalid("track limit reached"));
    }
    let index = match before_track {
        Some(track) => project
            .tracks
            .iter()
            .position(|candidate| candidate.id == track)
            .ok_or(Invalid("track insertion anchor no longer exists"))?,
        None => project.tracks.len(),
    };
    let parent_group = project.tracks.get(index).and_then(Track::parent_group);
    let track = new_track(project, name, crate::TrackKind::Ordinary, parent_group)?;
    let id = track.id();
    project.tracks.insert(index, track);
    Ok(id)
}

pub(crate) fn insert_track_before_with_gain(
    project: &mut Project,
    name: String,
    before_track: Option<TrackId>,
    gain: Option<f32>,
) -> Result<TrackId, Invalid> {
    if gain.is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value)) {
        return Err(Invalid("gain must be finite and within 0..2"));
    }
    let track = insert_track_before(project, name, before_track)?;
    if let Some(gain) = gain {
        project.track_mut(track)?.gain = gain;
    }
    Ok(track)
}

pub(crate) fn new_track(
    project: &mut Project,
    name: String,
    kind: crate::TrackKind,
    parent_group: Option<TrackId>,
) -> Result<Track, Invalid> {
    name_valid(&name)?;
    if project.tracks.len() >= MAX_TRACKS {
        return Err(Invalid("track limit reached"));
    }
    if parent_group.is_some_and(|id| !project.track(id).is_some_and(Track::is_group)) {
        return Err(Invalid("track parent must be an existing group"));
    }
    let id = TrackId(project.allocate_id()?);
    let color = crate::appearance::new_track_color(project);
    Ok(Track {
        group_output_mutes: Default::default(),
        tags: Vec::new(),
        kind,
        parent_group,
        color,
        gain_lane: crate::GainLane::default(),
        id,
        name,
        automatic_name: false,
        gain: 1.0,
        balance: 0.0,
        muted: false,
        soloed: false,
        devices: Vec::new(),
        instrument: None,
        retained_source_path: false,
        instrument_position: None,
        content: TrackContent::default(),
        sends: Vec::new(),
    })
}
pub(crate) fn interval_valid(
    start: u64,
    duration: u64,
    offset: u64,
    length: u64,
    asset: &Asset,
) -> Result<(), Invalid> {
    if duration == 0 || length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    start
        .checked_add(duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let end = offset
        .checked_add(length)
        .ok_or(Invalid("source end overflow"))?;
    if end > asset.playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    Ok(())
}

pub(crate) fn preview_edge_trim(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_playback_frames: u64,
) -> Result<Clip, Invalid> {
    if clip.edit.is_some() {
        return Err(Invalid("musical audio trim requires the project tempo map"));
    }
    if clip.duration == 0 || clip.source_length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    let timeline_end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let source_end = clip
        .source_offset
        .checked_add(clip.source_length)
        .ok_or(Invalid("source end overflow"))?;
    if source_end > asset_playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    if (clip.looping && clip.source_phase >= clip.source_length)
        || (!clip.looping && clip.source_phase != 0)
    {
        return Err(Invalid("clip source phase is outside its source interval"));
    }

    // Consume available earlier media through the existing plain trim law,
    // then represent only the pre-source interval as silent output.
    if !clip.looping && edge == TrimEdge::Left {
        let first = clip.start.saturating_sub(clip.source_offset);
        if frame < first {
            let rewound = preview_edge_trim(clip, edge, first, asset_playback_frames)?;
            return crate::audio_edit::extend_frame_left(&rewound, frame)?
                .ok_or(Invalid("audio source cannot retain a silent prefix"));
        }
    }
    let end = timeline_end;
    let (start, duration, source_offset, source_length, source_phase) = match (clip.looping, edge) {
        (true, TrimEdge::Left) => {
            let new_start = frame.clamp(0, end - 1);
            let phase = if new_start >= clip.start {
                (u128::from(clip.source_phase)
                    + u128::from(new_start - clip.start) % u128::from(clip.source_length))
                    % u128::from(clip.source_length)
            } else {
                let delta = u128::from(clip.start - new_start) % u128::from(clip.source_length);
                (u128::from(clip.source_phase) + u128::from(clip.source_length) - delta)
                    % u128::from(clip.source_length)
            } as u64;
            (
                new_start,
                end - new_start,
                clip.source_offset,
                clip.source_length,
                phase,
            )
        }
        (true, TrimEdge::Right) => {
            let new_end = frame.clamp(clip.start + 1, u64::MAX);
            (
                clip.start,
                new_end - clip.start,
                clip.source_offset,
                clip.source_length,
                clip.source_phase,
            )
        }
        (false, TrimEdge::Left) => {
            let min_start = clip.start.saturating_sub(clip.source_offset);
            let max_advance = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let max_start = clip
                .start
                .checked_add(max_advance)
                .ok_or(Invalid("timeline end overflow"))?;
            let new_start = frame.clamp(min_start, max_start);
            if new_start >= clip.start {
                let advance = new_start - clip.start;
                (
                    new_start,
                    clip.duration - advance,
                    clip.source_offset + advance,
                    clip.source_length - advance,
                    0,
                )
            } else {
                let extension = clip.start - new_start;
                (
                    new_start,
                    clip.duration
                        .checked_add(extension)
                        .ok_or(Invalid("timeline end overflow"))?,
                    clip.source_offset - extension,
                    clip.source_length
                        .checked_add(extension)
                        .ok_or(Invalid("source length overflow"))?,
                    0,
                )
            }
        }
        (false, TrimEdge::Right) => {
            let source_end = clip
                .source_offset
                .checked_add(clip.source_length)
                .ok_or(Invalid("source end overflow"))?;
            let media_remaining = asset_playback_frames - source_end;
            let timeline_remaining = u64::MAX - end;
            let extension = media_remaining.min(timeline_remaining);
            // A one-shot may outlive its source with a silent tail. Shortening
            // consumes that silence first and only then the audible source.
            let minimum_end = end - clip.duration.saturating_sub(1);
            let maximum_end = end + extension;
            let new_end = frame.clamp(minimum_end, maximum_end);
            if new_end >= end {
                let extension = new_end - end;
                (
                    clip.start,
                    clip.duration + extension,
                    clip.source_offset,
                    clip.source_length + extension,
                    0,
                )
            } else {
                let duration = clip.duration - (end - new_end);
                (
                    clip.start,
                    duration,
                    clip.source_offset,
                    clip.source_length.min(duration),
                    0,
                )
            }
        }
    };

    let changed = start != clip.start
        || duration != clip.duration
        || source_offset != clip.source_offset
        || source_length != clip.source_length
        || source_phase != clip.source_phase;
    let fade = if changed {
        clip.fade.reanchor(duration)?
    } else {
        clip.fade
    };
    // Trim changes geometry only: activation, name, flavor, gain and edit
    // settings come from the original so trimming never reactivates a clip.
    Ok(Clip {
        start,
        duration,
        source_offset,
        source_length,
        source_phase,
        fade,
        ..clip.clone()
    })
}

pub(crate) fn split_clip_at(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
) -> Result<(Clip, Clip), Invalid> {
    if original.edit.is_some() {
        return Err(Invalid(
            "musical audio split requires the project tempo map",
        ));
    }
    let end = original
        .start
        .checked_add(original.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    if frame <= original.start || frame >= end {
        return Err(Invalid("split cursor must be inside the clip"));
    }
    let left_length = frame - original.start;
    if !original.looping && left_length >= original.source_length {
        return Err(Invalid("split cursor is beyond the one-shot source"));
    }
    let (right_source_offset, right_source_length, right_phase) = if original.looping {
        (
            original.source_offset,
            original.source_length,
            ((u128::from(original.source_phase) + u128::from(left_length))
                % u128::from(original.source_length)) as u64,
        )
    } else {
        (
            original
                .source_offset
                .checked_add(left_length)
                .ok_or(Invalid("source offset overflow"))?,
            original
                .source_length
                .checked_sub(left_length)
                .ok_or(Invalid("source length underflow"))?,
            0,
        )
    };
    let right_duration = end - frame;
    let right_fade = original.fade.with_offset(
        original
            .fade
            .offset()
            .checked_add(left_length)
            .ok_or(Invalid("fade offset overflow"))?,
        right_duration,
    )?;
    let mut left = original.clone();
    left.duration = left_length;
    if !left.looping {
        left.source_length = left_length;
        left.source_phase = 0;
    }
    let right = Clip {
        id: right_id,
        start: frame,
        duration: right_duration,
        source_offset: right_source_offset,
        source_length: right_source_length,
        source_phase: right_phase,
        edit: None,
        fade: right_fade,
        ..original.clone()
    };
    Ok((left, right))
}

pub(crate) fn split_clip_at_tempo(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
    tempo: &crate::TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    let Some(edit) = &original.edit else {
        if cuts_silent_tail(original, frame) {
            // A raw source offset cannot advance past the audible interval, so
            // crop a captured retained domain, as `retained_audio_fragment` does.
            let mut captured = original.clone();
            captured.edit = Some(crate::AudioClipEdit::capture(original).into());
            return split_clip_at_tempo(&captured, frame, right_id, tempo);
        }
        return split_clip_at(original, frame, right_id);
    };
    let end = original
        .start
        .checked_add(original.duration)
        .ok_or(Invalid("audio split end overflow"))?;
    if frame <= original.start || frame >= end {
        return Err(Invalid("split cursor must be inside the clip"));
    }
    if edit.content_view.is_some() || edit.placement.is_some() {
        let left = crate::audio_edit::trim_view(original, crate::TrimEdge::Right, frame, tempo)?;
        let mut right =
            crate::audio_edit::trim_view(original, crate::TrimEdge::Left, frame, tempo)?;
        right.id = right_id;
        return Ok((left, right));
    }
    let cut = edit.crop_at_frame(original, frame, tempo)?;
    if cut <= edit.view_start || cut >= edit.view_end {
        return Err(Invalid("split has no positive musical crop"));
    }
    let reference_start = edit
        .reference_start_raw()
        .map(|_| edit.reference_beat_at_destination(original, frame, tempo))
        .transpose()?;
    let mut left = original.clone();
    let mut right = original.clone();
    left.audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?
        .view_end = cut;
    let view = right
        .audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?;
    view.view_start = cut;
    view.set_reference_start_raw(reference_start)?;
    right.id = right_id;
    right.start = frame;
    if edit.processing_projection.is_some() {
        left.audio_edit_mut()
            .ok_or(Invalid("audio domain missing"))?
            .retain_frame_bounds(original.start, frame, tempo)?;
        right
            .audio_edit_mut()
            .ok_or(Invalid("audio domain missing"))?
            .retain_frame_bounds(frame, end, tempo)?;
    }
    crate::audio_edit::project_clip(&mut left, tempo)?;
    crate::audio_edit::project_clip(&mut right, tempo)?;
    Ok((left, right))
}

/// Whether `frame` falls inside a raw one-shot's silent tail, past its source.
pub(crate) fn cuts_silent_tail(clip: &Clip, frame: u64) -> bool {
    clip.edit.is_none()
        && !clip.looping
        && frame
            .checked_sub(clip.start)
            .is_some_and(|offset| offset >= clip.source_length && offset < clip.duration)
}

/// Crop one retained interval through the existing typed split law.
pub(crate) fn retained_audio_fragment(
    victim: &Clip,
    start: u64,
    end: u64,
    id: ClipId,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    let original_end = victim
        .start
        .checked_add(victim.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    if start < victim.start || start >= end || end > original_end {
        return Err(Invalid(
            "retained audio interval is outside its source clip",
        ));
    }
    // A raw one-shot can outlive its audible source. Its silent suffix cannot
    // be represented by advancing a raw source offset past that interval; crop
    // the existing retained domain instead, preserving silence and root fades.
    let captured;
    let victim =
        if victim.edit.is_none() && !victim.looping && victim.duration > victim.source_length {
            captured = {
                let mut clip = victim.clone();
                clip.edit = Some(crate::AudioClipEdit::capture(victim).into());
                clip
            };
            &captured
        } else {
            victim
        };
    let mut retained = if start > victim.start {
        split_clip_at_tempo(victim, start, id, tempo)?.1
    } else {
        victim.clone()
    };
    if end < original_end {
        retained = split_clip_at_tempo(&retained, end, id, tempo)?.0;
    }
    retained.id = id;
    Ok(retained)
}

impl Project {
    /// Build assets/tracks privately, then resolve all imported footprints against
    /// the original rows together. The Editor owns preparation and history.
    pub(crate) fn apply_audio_imports(
        &mut self,
        commands: Vec<Command>,
        warp: crate::AudioImportWarp,
        new_track_gain: Option<f32>,
    ) -> Result<Vec<Created>, Invalid> {
        let count = commands.len();
        let mut incoming = Vec::new();
        let mut owners = Vec::new();
        incoming
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported placements"))?;
        owners
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported owners"))?;
        for command in commands {
            let (track, start, asset, playback_frames) = match command {
                Command::ImportPreparedAudioTrackAt {
                    name,
                    before_track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                } => {
                    let track = insert_track_before(self, name, before_track)?;
                    if let Some(gain) = new_track_gain {
                        self.track_mut(track)?.gain = gain;
                    }
                    let asset = self.add_import_asset(
                        track,
                        path,
                        source_sample_rate,
                        source_frames,
                        playback_frames,
                        conversion_version,
                    )?;
                    (track, start, asset, playback_frames)
                }
                Command::ImportPreparedAudioClip {
                    track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                }
                | Command::ImportPreparedAudioClipWithOverwrite {
                    track,
                    start,
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                } => {
                    let asset = self.add_import_asset(
                        track,
                        path,
                        source_sample_rate,
                        source_frames,
                        playback_frames,
                        conversion_version,
                    )?;
                    (track, start, asset, playback_frames)
                }
                Command::ImportExistingAudio { asset, placement } => {
                    let playback_frames = self.existing_import_frames(asset)?;
                    let (track, start) = match placement {
                        crate::ExistingAudioPlacement::NewTrack {
                            name,
                            before_track,
                            start,
                        } => {
                            let track = insert_track_before(self, name, before_track)?;
                            if let Some(gain) = new_track_gain {
                                self.track_mut(track)?.gain = gain;
                            }
                            (track, start)
                        }
                        crate::ExistingAudioPlacement::Clip { track, start, .. } => {
                            self.track(track).ok_or(Invalid("track no longer exists"))?;
                            (track, start)
                        }
                        crate::ExistingAudioPlacement::Session { .. } => {
                            return Err(Invalid("unsupported arrangement audio import command"));
                        }
                    };
                    (track, start, asset, playback_frames)
                }
                _ => return Err(Invalid("unsupported arrangement audio import command")),
            };
            interval_valid(
                start,
                playback_frames,
                0,
                playback_frames,
                self.asset(asset).ok_or(Invalid("imported media missing"))?,
            )?;
            let mut clip = Clip {
                active: true,
                name: "clip".into(),
                flavor: crate::ClipFlavor::default(),
                id: ClipId(0),
                asset,
                start,
                duration: playback_frames,
                source_offset: 0,
                source_length: playback_frames,
                looping: false,
                source_phase: 0,
                fade: ClipFade::neutral(playback_frames)?,
                gain: crate::ClipGain::UNITY,
                edit: None,
            };
            if warp == crate::AudioImportWarp::Beats {
                let settings = crate::default_audio_warp(
                    &clip,
                    self.tempo_map(),
                    crate::WarpMode::Beats {
                        preserve: crate::BeatsPreserve::Transients,
                        tail: crate::BeatsTail::Forward,
                        decay: 100,
                    },
                )?;
                let mut edit = crate::AudioClipEdit::capture(&clip);
                edit.anchor(clip.start, self.tempo_map())?;
                let domain = std::sync::Arc::make_mut(&mut edit.domain);
                domain.warp = Some(settings);
                domain.warp_enabled = true;
                clip.edit = Some(std::sync::Arc::new(edit));
                crate::audio_edit::project_clip(&mut clip, self.tempo_map())?;
            }
            incoming.push(crate::ClipGroupPreview {
                track,
                content: crate::ClipGroupContent::Audio(clip),
            });
            owners.push((track, asset));
        }
        let Created::Clips { first, last } = crate::clip_group::place_batch(self, incoming, &[])?
        else {
            return Err(Invalid("audio import returned no clip identities"));
        };
        let mut results = Vec::new();
        results
            .try_reserve_exact(count)
            .map_err(|_| Invalid("could not reserve imported results"))?;
        for (index, (track, asset)) in owners.into_iter().enumerate() {
            let clip = ClipId(
                first
                    .0
                    .checked_add(index as u64)
                    .filter(|id| *id <= last.0)
                    .ok_or(Invalid("audio import identity range mismatch"))?,
            );
            results.push(Created::Imported { track, asset, clip });
        }
        Ok(results)
    }
}

impl Project {
    /// Called only on an isolated candidate by Editor; never on an audio thread.
    pub(crate) fn apply(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            Command::DuplicateAliasClips(clips) => crate::clip_duplicate::apply_alias(self, clips),
            Command::DuplicateAliasClipsAt {
                clips,
                anchor,
                track,
                position,
            } => crate::clip_duplicate::apply_alias_at(self, clips, anchor, track, position),
            Command::DuplicateAliasClipToNewTrack {
                clip,
                name,
                before_track,
                position,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_duplicate::apply_alias_at(self, vec![clip], clip, track, position)
            }
            Command::MakeClipsUnique(clips) => crate::clip_alias_commands::unique(self, clips),
            Command::ApplyClipSettingsToAliases(clip) => {
                crate::clip_alias_commands::apply_settings(self, clip)
            }
            Command::DuplicateSessionCell {
                track,
                scene,
                alias,
            } => crate::clip_alias_commands::duplicate_console(self, track, scene, alias),
            source_command @ (Command::CreateNativeSource { .. }
            | Command::BindTrackSource { .. }
            | Command::SetNativeSourceReceiver { .. }
            | Command::RenameNativeSource { .. }
            | Command::RemoveNativeSource(_)) => crate::native_sources::apply(self, source_command),
            Command::CommitArrangementBounce {
                target,
                in_place,
                adoption,
                files,
            } => crate::bounce::commit(self, target, in_place, adoption, files),
            command @ (Command::AddArrangementSection { .. }
            | Command::SetArrangementSectionRange { .. }
            | Command::MoveArrangementSectionMarker { .. }
            | Command::ResizeArrangementSectionMarker { .. }
            | Command::RenameArrangementSection { .. }
            | Command::SetArrangementSectionColor { .. }
            | Command::RemoveArrangementSection(_)
            | Command::ReorderSectionBefore { .. }
            | Command::ReorderSectionAfter { .. }
            | Command::ResizeArrangementSection { .. }
            | Command::ResizeArrangementSectionEdge { .. }
            | Command::ResizeArrangementSectionBoundary { .. }
            | Command::DeleteArrangementSection(_)
            | Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::MoveArrangementSectionBeyondEnd { .. }
            | Command::ReplaceArrangementSection { .. }
            | Command::SplitArrangementSection { .. }
            | Command::MergeArrangementSections(_)) => {
                crate::arrangement_sections::apply(self, command)
            }
            Command::SetTuning(tuning) => {
                self.tuning = tuning;
                Ok(Created::None)
            }
            Command::EditAudioEvents { clip, operation } => {
                crate::audio_events::apply(self, clip, operation)?;
                Ok(Created::None)
            }
            Command::ConvertAudioClipToMidi(_) => Err(Invalid(crate::AUDIO_TO_MIDI_UNSUPPORTED)),
            Command::ConvertMidiClipToAudio { clip, path, frames } => {
                crate::clip_conversion::midi_to_audio(self, clip, path, frames)
            }
            Command::SpliceGainTouchBatch {
                track,
                expected_epoch,
                spans,
            } => {
                if spans.is_empty() || spans.len() > crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                    return Err(Invalid("gain touch take span count is outside its bound"));
                }
                if self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain_lane
                    .recording_epoch
                    != expected_epoch
                {
                    return Err(Invalid("gain touch replacement epoch is stale"));
                }
                let mut epoch = expected_epoch;
                for span in &spans {
                    span.validate()?;
                    if span.context.recording_epoch <= epoch {
                        return Err(Invalid("gain touch take epochs must strictly increase"));
                    }
                    if span.context.sample_rate != self.sample_rate {
                        return Err(Invalid("gain touch sample rate differs from project"));
                    }
                    epoch = span.context.recording_epoch;
                }
                // Project::apply runs only on Editor's isolated candidate. A
                // later refusal discards every earlier pass and all its IDs.
                let mut epoch = expected_epoch;
                for span in spans {
                    let next = span.context.recording_epoch;
                    self.apply(Command::SpliceGainTouch {
                        track,
                        expected_epoch: epoch,
                        span,
                    })?;
                    epoch = next;
                }
                Ok(Created::None)
            }
            Command::SpliceGainTouch {
                track,
                expected_epoch,
                span,
            } => {
                let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
                if span.context.sample_rate != self.sample_rate {
                    return Err(Invalid("gain touch sample rate differs from project"));
                }
                let proposed_units = owner.gain_lane.touch_units(expected_epoch, &span)?;
                let other = self
                    .tracks()
                    .iter()
                    .filter(|owner| owner.id() != track)
                    .try_fold(0_usize, |count, owner| {
                        count
                            .checked_add(owner.gain_lane.units()?)
                            .ok_or(Invalid("gain lane count overflow"))
                    })?;
                if other
                    .checked_add(proposed_units)
                    .is_none_or(|count| count > crate::MAX_GAIN_POINTS)
                {
                    return Err(Invalid("project gain lane limit reached"));
                }
                let proposed = owner.gain_lane.with_touch(expected_epoch, span)?;
                self.track_mut(track)?.gain_lane = proposed;
                Ok(Created::None)
            }
            Command::ReplaceTrackGainEnvelope { track, points } => {
                self.replace_gain_envelope(track, points)
            }
            Command::UpsertTrackGainPoint { track, point } => self.upsert_gain_point(track, point),
            Command::MoveTrackGainPoint { track, from, to } => {
                self.move_gain_point(track, from, to)
            }
            Command::RemoveTrackGainPoint { track, beat } => self.remove_gain_point(track, beat),
            Command::ClearTrackGainEnvelope { track } => {
                let lane = &mut self.track_mut(track)?.gain_lane;
                lane.base.clear();
                lane.spans.clear();
                Ok(Created::None)
            }
            Command::SetTrackColor { track, color } => {
                self.track_mut(track)?.color = color;
                Ok(Created::None)
            }
            Command::SetClipActive { clip, active } => {
                crate::clip_activation::set(self, clip, active)
            }
            Command::SetTimeRangeActive { range, active } => {
                crate::clip_activation::set_range(self, &range, active)
            }
            Command::SetClipFlavor { clip, flavor } => self.set_clip_flavor(clip, flavor),
            Command::ReplaceMidiClipVolume { clip, volume } => {
                self.replace_midi_clip_volume(clip, volume)
            }
            Command::SetMidiClipGain { clip, gain } => self.set_midi_clip_gain(clip, gain),
            Command::SetRouting(edges) => self.adopt_routing(edges),
            Command::SetAudioFeed {
                source,
                destination,
                gain,
            } => self.set_audio_feed(source, destination, gain),
            Command::SetSidechain { device, source } => self.set_sidechain(device, source),
            Command::SetMidiSource {
                destination,
                source,
            } => self.set_midi_source(destination, source),
            Command::SetReturnFeed {
                source,
                destination,
                gain,
            } => self.set_return_feed(source, destination, gain),
            Command::ManualDeviceEdit {
                device,
                state,
                targets,
            } => self.manual_device_edit(device, state, targets),
            Command::SetDeviceAutomationReadSelection {
                device,
                target,
                selection,
            } => self.set_device_automation_read_selection(device, target, selection),
            Command::ReenableDeviceAutomation => self.reenable_device_automation(),
            Command::SpliceDeviceAutomationTouch {
                device,
                target,
                vst3_class,
                spans,
            } => self.splice_device_automation_touch(device, target, vst3_class, spans),
            Command::SpliceVst3AutomationTouchGroup {
                device,
                class,
                lanes,
            } => self.splice_vst3_automation_touch_group(device, class, lanes),
            Command::ReplaceDeviceModulation { device, modulation } => {
                self.replace_device_modulation(device, modulation)
            }
            Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points,
            } => self.replace_device_automation_target(device, target, points),
            Command::ReplaceDeviceAutomation {
                device,
                parameter,
                points,
            } => self.replace_device_automation(device, parameter, points),
            Command::InitializeDevice(initialization) => initialization.apply(self),
            main_command @ (Command::InsertMainDevice { .. } | Command::MoveMainDevice { .. }) => {
                crate::main_devices::apply(self, main_command)
            }
            device_command @ (Command::InsertDevice { .. }
            | Command::InsertInstrument { .. }
            | Command::RemoveInstrument { .. }
            | Command::RemoveDevice(_)
            | Command::MoveDevice { .. }
            | Command::MoveTrackDevice { .. }
            | Command::MoveNativeSourceDevice { .. }
            | Command::ReparentDevice { .. }
            | Command::InsertNestedDevice { .. }
            | Command::ReplaceDeviceState { .. }
            | Command::ReplaceDeviceStateWithIdentityReservation { .. }
            | Command::SetDeviceBypass { .. }
            | Command::ToggleDeviceBypass { .. }
            | Command::CompareDevice { .. }
            | Command::ReplaceDeviceComparison { .. }) => {
                crate::device::apply(self, device_command)
            }
            return_command @ (Command::AddReturnTrack { .. }
            | Command::AddReturnTrackWithGain { .. }
            | Command::RemoveReturnTrack(_)
            | Command::RenameReturnTrack { .. }
            | Command::ReturnGain { .. }
            | Command::ReturnMute { .. }
            | Command::SetReturnSend { .. }
            | Command::InsertReturnDevice { .. }
            | Command::MoveReturnDevice { .. }) => crate::returns::apply(self, return_command),
            Command::DeleteTimeRange { range } => crate::time_range::clear(self, &range),
            Command::PasteTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste(self, &clipboard, track, frame),
            Command::PasteTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste_on_source_tracks(self, &clipboard, track, frame),
            Command::InsertTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert(self, &clipboard, track, frame),
            Command::InsertTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert_on_source_tracks(self, &clipboard, track, frame),
            Command::MoveTimeRange {
                range,
                track,
                frame,
            } => crate::time_range::move_range(self, &range, track, frame),
            Command::CopyTimeRangeTo {
                range,
                track,
                frame,
            } => crate::time_range::copy_range(self, &range, track, frame),
            midi_command @ (Command::AddMidiTrack { .. }
            | Command::PlaceMidiClip { .. }
            | Command::PlaceMidiClipWithOverwrite { .. }
            | Command::SetMidiClipSource { .. }
            | Command::ResizeMidiClipView { .. }
            | Command::TrimMidiClipEdge { .. }
            | Command::StretchMidiClipEdge { .. }
            | Command::SplitMidiClip { .. }
            | Command::AddMidiNote { .. }
            | Command::AddMidiNotes { .. }
            | Command::EditMidiNotes { .. }
            | Command::EditMidiNotesWithSourceExtent { .. }
            | Command::DuplicateMidiSourceRange { .. }
            | Command::UpdateMidiNote { .. }
            | Command::UpdateMidiNotes { .. }
            | Command::UpdateMidiNotesWithSourceExtent { .. }
            | Command::SetMidiNotesActive { .. }
            | Command::RemoveMidiNote(_)
            | Command::RemoveMidiNotes { .. }
            | Command::MoveMidiClip { .. }
            | Command::MoveMidiClipWithOverwrite { .. }
            | Command::MoveMidiClipToNewTrack { .. }
            | Command::CopyMidiClipTo { .. }
            | Command::RemoveMidiClip(_)
            | Command::DuplicateMidiClip(_)) => crate::midi::apply(self, midi_command),
            Command::CopyMidiClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_copy::midi_to(self, clip, track, start)?;
                Ok(Created::Track(track))
            }
            session_command @ (Command::AddScene { .. }
            | Command::InsertScene { .. }
            | Command::DuplicateScene(_)
            | Command::MoveScene { .. }
            | Command::RenameScene { .. }
            | Command::RemoveScene(_)
            | Command::CreateSessionMidiClip { .. }
            | Command::CreateSessionAudioClip { .. }
            | Command::CopyArrangementAudioClipToSession { .. }
            | Command::CopyArrangementMidiToSession { .. }
            | Command::CopyArrangementAudioClipToNewSessionTrack { .. }
            | Command::CopyArrangementMidiToNewSessionTrack { .. }
            | Command::MoveSessionCell { .. }
            | Command::DuplicateSessionMidiCell { .. }
            | Command::DuplicateSessionAudioCell { .. }
            | Command::SetSessionAudioClipLoop { .. }
            | Command::SetSessionAudioClipSource { .. }
            | Command::ClearSessionCell { .. }
            | Command::SetSessionStopCell { .. }
            | Command::SetSessionMidiClipSource { .. }
            | Command::RenameSessionMidiClip { .. }) => {
                crate::session::apply(self, session_command)
            }
            Command::DuplicateClips(clips) => crate::clip_duplicate::apply(self, clips),
            Command::GroupClips(clips) => crate::clip_consolidate::apply(self, clips),
            Command::GroupTimeRange(range) => crate::clip_consolidate::time_range(self, &range),
            Command::GroupAudioEventRange { clip, start, end } => {
                crate::clip_consolidate::event_range(self, clip, start, end)
            }
            Command::ReverseClips(clips) => crate::clip_reverse::apply(self, clips),
            Command::DuplicateClip(clip) => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = original
                    .start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = self
                    .tracks
                    .iter()
                    .find(|t| t.clips().iter().any(|c| c.id == clip))
                    .map(|t| t.id)
                    .ok_or(Invalid("clip track missing"))?;
                crate::clip_copy::audio_to(self, clip, track, start)
            }
            Command::SplitClip { clip, frame } => {
                let (track_index, clip_index) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .find_map(|(track_index, track)| {
                        track
                            .clips()
                            .iter()
                            .position(|item| item.id == clip)
                            .map(|clip_index| (track_index, clip_index))
                    })
                    .ok_or(Invalid("clip no longer exists"))?;
                let original = self.tracks[track_index].clips()[clip_index].clone();
                if self
                    .tracks
                    .iter()
                    .map(|track| track.clips().len() + track.midi_clips().len())
                    .sum::<usize>()
                    >= MAX_CLIPS
                {
                    return Err(Invalid("clip limit reached"));
                }
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve clip storage"))?;
                let right_id = ClipId(self.allocate_id()?);
                let (left, right) =
                    crate::clip_fade_split::split(&original, frame, right_id, self.tempo_map())?;
                self.audio_clips_mut(self.tracks[track_index].id)?[clip_index] = left;
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .insert(clip_index + 1, right);
                Ok(Created::Clip(right_id))
            }
            Command::SplitTracksAtFrame { tracks, frame } => {
                crate::split_tracks::apply(self, &tracks, frame)
            }
            Command::SplitTimeRange { range } => crate::split_tracks::apply_range(self, &range),
            Command::NudgeClip { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = if frames < 0 {
                    original.start.saturating_sub(frames.unsigned_abs())
                } else {
                    original
                        .start
                        .checked_add(frames as u64)
                        .ok_or(Invalid("timeline start overflow"))?
                };
                let track = self
                    .tracks
                    .iter()
                    .find(|track| track.clips().iter().any(|item| item.id == clip))
                    .ok_or(Invalid("clip track missing"))?
                    .id;
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::TrimClipEndBy { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if original.looping || original.edit.is_some() {
                    let end = original
                        .start
                        .checked_add(original.duration)
                        .ok_or(Invalid("timeline end overflow"))?;
                    let desired = if frames < 0 {
                        end.saturating_sub(frames.unsigned_abs())
                    } else {
                        end.saturating_add(frames as u64)
                    };
                    return self.apply(Command::TrimClipEdge {
                        clip,
                        edge: TrimEdge::Right,
                        frame: desired,
                    });
                }
                let adjust = |value: u64| {
                    if frames < 0 {
                        value.checked_sub(frames.unsigned_abs())
                    } else {
                        value.checked_add(frames as u64)
                    }
                };
                let duration = adjust(original.duration).ok_or(Invalid("trim length overflow"))?;
                let source_length =
                    adjust(original.source_length).ok_or(Invalid("trim source length overflow"))?;
                self.apply(Command::TrimClip {
                    clip,
                    start: original.start,
                    duration,
                    source_offset: original.source_offset,
                    source_length,
                })
            }
            Command::GainBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("gain adjustment must be finite"));
                }
                let gain = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain
                    + amount)
                    .clamp(0.0, 2.0);
                self.apply(Command::Gain { track, gain })
            }
            Command::BalanceBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("balance adjustment must be finite"));
                }
                let balance = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .balance
                    + amount)
                    .clamp(-1.0, 1.0);
                self.apply(Command::Balance { track, balance })
            }
            Command::MasterGainBy { amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("master gain adjustment must be finite"));
                }
                let gain = (self.master_gain + amount).clamp(0.0, 2.0);
                self.apply(Command::MasterGain { gain })
            }
            Command::ToggleTracksMute(tracks) => {
                let mut muted = false;
                for id in tracks.ids() {
                    muted |= !self
                        .track(*id)
                        .ok_or(Invalid("track selection no longer exists"))?
                        .muted;
                }
                for track in &mut self.tracks {
                    if tracks.ids().contains(&track.id()) {
                        track.muted = muted;
                    }
                }
                Ok(Created::None)
            }
            Command::ToggleMute(track) => {
                let muted = !self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .muted;
                self.apply(Command::Mute { track, muted })
            }
            Command::InsertTrackWithDevice {
                name,
                before_track,
                slot,
                state,
            } => {
                let track = insert_track_before(self, name, before_track)?;
                match slot {
                    TrackDeviceSlot::Instrument => {
                        self.apply(Command::InsertInstrument { track, state })?;
                    }
                    TrackDeviceSlot::Effect => {
                        self.apply(Command::InsertDevice {
                            track,
                            before: None,
                            state,
                        })?;
                    }
                }
                Ok(Created::Track(track))
            }
            Command::InsertDefaultTrack { before_track } => Ok(Created::Track(
                crate::track_name::insert_default_track(self, before_track)?,
            )),
            Command::InsertDefaultTrackWithGain { before_track, gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                let track = crate::track_name::insert_default_track(self, before_track)?;
                self.track_mut(track)?.gain = gain;
                Ok(Created::Track(track))
            }
            Command::InsertDefaultTrackWithDevice {
                before_track,
                slot,
                state,
                device_name,
            } => crate::track_name::insert_default_track_with_device(
                self,
                before_track,
                slot,
                state,
                device_name,
            ),
            Command::InsertDefaultTrackWithDeviceAndGain {
                before_track,
                slot,
                state,
                device_name,
                gain,
            } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                let Created::Track(track) = crate::track_name::insert_default_track_with_device(
                    self,
                    before_track,
                    slot,
                    state,
                    device_name,
                )?
                else {
                    return Err(Invalid("device track creation returned no track"));
                };
                self.track_mut(track)?.gain = gain;
                Ok(Created::Track(track))
            }
            Command::LoadInstrument {
                track,
                replacing,
                state,
                device_name,
            } => crate::track_name::load_instrument(self, track, replacing, state, device_name),
            Command::AddTrack { name } => {
                Ok(Created::Track(insert_track_before(self, name, None)?))
            }
            Command::InsertTrack { name, before_track } => Ok(Created::Track(insert_track_before(
                self,
                name,
                before_track,
            )?)),
            Command::ImportAudioTrack {
                name,
                path,
                sample_rate,
                frames,
            } => self.apply(Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate: sample_rate,
                source_frames: frames,
                playback_frames: frames,
                conversion_version: IDENTITY_CONVERSION,
            }),
            Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => self.apply(Command::ImportPreparedAudioTrackAt {
                name,
                before_track: None,
                start: 0,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            }),
            Command::ImportPreparedAudioTrackAt {
                name,
                before_track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let track = insert_track_before(self, name, before_track)?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedAudioClip {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedAudioClipWithOverwrite {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => self.apply(Command::ImportPreparedAudioClip {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            }),
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
                source_start,
                source_end,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                self.apply(Command::CreateSessionAudioClip {
                    track,
                    scene,
                    name,
                    asset,
                    source_start,
                    source_end,
                })
            }
            Command::ImportExistingAudio { asset, placement } => {
                self.apply_existing_audio_import(asset, placement)
            }
            Command::MoveTrackBefore { track, before } => self.move_track_before(track, before),
            Command::MoveTracksBefore { tracks, before } => {
                self.move_tracks_before(&tracks, before)
            }
            Command::GroupTracks { tracks, name } => self.group_tracks(&tracks, name),
            Command::UngroupTrack(group) => self.ungroup_track(group),
            Command::ReparentTracks {
                tracks,
                destination,
            } => self.reparent_tracks(&tracks, destination),
            Command::DuplicateTracks {
                tracks,
                without_clips,
            } => {
                let destination = self.track_destination_after_selection(&tracks)?;
                let clipboard = self.capture_tracks(&tracks, without_clips)?;
                crate::track_clipboard::paste(self, &clipboard, destination)
            }
            Command::RemoveTracks(ids) => self.remove_track_subtrees(&ids),
            Command::RemoveTrack(id) => self.remove_track_subtrees(&[id]),
            Command::EditClips { clips, edit } => {
                crate::clip_selection_edit::apply(self, clips, edit)
            }
            Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix,
                notes,
            } => crate::midi_source_prefix::apply(self, clip, prefix, notes),
            Command::RenameClip { clip, name } => crate::clip_name::rename(self, clip, name),
            Command::SetTrackTags { track, tags } => {
                let tags = crate::track_tags::canonical_list(tags)?;
                self.track_mut(track)?.tags = tags;
                Ok(Created::None)
            }
            Command::RenameTrack { track, name } => {
                name_valid(&name)?;
                let target = self.track_mut(track)?;
                target.name = name;
                target.automatic_name = false;
                Ok(Created::None)
            }
            Command::Gain { track, gain } => crate::track_gain_group::apply(self, &[(track, gain)]),
            Command::SetTrackGains { gains } => crate::track_gain_group::apply(self, &gains),
            Command::Balance { track, balance } => {
                if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
                    return Err(Invalid("balance must be finite and within -1..1"));
                }
                self.track_mut(track)?.balance = balance;
                Ok(Created::None)
            }
            Command::MasterGain { gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("master gain must be finite and within 0..2"));
                }
                self.master_gain = gain;
                Ok(Created::None)
            }
            Command::SetProjectTempo { tempo } => {
                self.set_project_tempo(tempo)?;
                Ok(Created::None)
            }
            Command::SetTempoPoint { beat, tempo } => {
                self.set_tempo_point(TempoPoint::new(beat, tempo))?;
                Ok(Created::None)
            }
            Command::SetMeterPoint { beat, meter } => {
                self.meter_map = self.meter_map.replace_point(MeterPoint::new(beat, meter))?;
                Ok(Created::None)
            }
            Command::Mute { track, muted } => {
                self.track_mut(track)?.muted = muted;
                Ok(Created::None)
            }
            Command::ToggleSolo { track, exclusive } => {
                let currently_soloed = self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .soloed;
                if currently_soloed {
                    self.track_mut(track)?.soloed = false;
                } else {
                    if exclusive {
                        for item in &mut self.tracks {
                            item.soloed = false;
                        }
                    }
                    self.track_mut(track)?.soloed = true;
                }
                Ok(Created::None)
            }
            Command::ToggleSoloTracks { tracks, exclusive } => {
                let ids = tracks.ids();
                if ids.iter().any(|id| self.track(*id).is_none()) {
                    return Err(Invalid("solo selection no longer exists"));
                }
                let enable = !self
                    .tracks
                    .iter()
                    .filter(|track| ids.contains(&track.id()))
                    .all(|track| track.soloed);
                for track in &mut self.tracks {
                    if ids.contains(&track.id()) {
                        track.soloed = enable;
                    } else if enable && exclusive {
                        track.soloed = false;
                    }
                }
                Ok(Created::None)
            }
            Command::SetLoopRegion {
                start,
                end,
                enabled,
            } => {
                self.loop_region = Some(LoopRegion::new(start, end, enabled)?);
                Ok(Created::None)
            }
            Command::ToggleLoopRegion => {
                let region = self.loop_region.ok_or(Invalid("no loop region is set"))?;
                self.loop_region = Some(LoopRegion::new(
                    region.start(),
                    region.end(),
                    !region.enabled(),
                )?);
                Ok(Created::None)
            }
            Command::ClearLoopRegion => {
                self.loop_region = None;
                Ok(Created::None)
            }
            Command::AddAsset {
                path,
                sample_rate,
                frames,
            } => {
                if sample_rate != self.sample_rate {
                    return Err(Invalid(
                        "media rate differs; prepared conversion is required",
                    ));
                }
                let frames = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                self.add_prepared_asset(path, sample_rate, frames, frames, IDENTITY_CONVERSION)
                    .map(Created::Asset)
            }
            Command::ImportDeviceAsset {
                path,
                sample_rate,
                frames,
            } => {
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                self.add_prepared_asset(path, sample_rate, frames, playback, conversion)
                    .map(Created::Asset)
            }
            Command::ImportDeviceAssetIntoEmptyInstrument {
                track,
                path,
                sample_rate,
                frames,
                state,
                source_slot,
            } => {
                let target = self
                    .track(track)
                    .ok_or(Invalid("instrument track no longer exists"))?;
                if target.instrument().is_some() || !state.asset_references().is_empty() {
                    return Err(Invalid(
                        "device source import requires an empty instrument slot and unbound state",
                    ));
                }
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                let asset =
                    self.add_prepared_asset(path, sample_rate, frames, playback, conversion)?;
                let state = state
                    .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                        slot: source_slot,
                        asset: hex_lattice_contract::DeviceAssetKey::new(asset.value())
                            .map_err(|_| Invalid("invalid device source id"))?,
                    }])
                    .map_err(|_| Invalid("invalid device source binding"))?;
                // Editor applies this composite command to one private candidate;
                // preparation/refusal commits neither the asset nor instrument.
                self.apply(Command::InsertInstrument { track, state })?;
                Ok(Created::Asset(asset))
            }
            Command::ImportDeviceAssetReplacingInstrument {
                track,
                device,
                expected_state,
                path,
                sample_rate,
                frames,
                state,
                source_slot,
            } => {
                let target = self
                    .track(track)
                    .and_then(|track| track.instrument())
                    .filter(|target| target.id() == device && target.state() == &expected_state)
                    .ok_or(Invalid("instrument source replacement target changed"))?;
                if state.type_id() != target.state().type_id()
                    || state.input_gain_millidb() != expected_state.input_gain_millidb()
                    || state.output_gain_millidb() != expected_state.output_gain_millidb()
                    || state.asset_references() != expected_state.asset_references()
                    || (!state.asset_references().is_empty()
                        && state
                            .asset_references()
                            .iter()
                            .filter(|r| r.slot == source_slot)
                            .count()
                            != 1)
                {
                    return Err(Invalid(
                        "instrument replacement must preserve provider and host metadata",
                    ));
                }
                let playback = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                let conversion = crate::import_conversion_version(sample_rate, self.sample_rate);
                let asset =
                    self.add_prepared_asset(path, sample_rate, frames, playback, conversion)?;
                let key = hex_lattice_contract::DeviceAssetKey::new(asset.value())
                    .map_err(|_| Invalid("invalid device source id"))?;
                let references = if state.asset_references().is_empty() {
                    vec![hex_lattice_contract::DeviceAssetReference {
                        slot: source_slot,
                        asset: key,
                    }]
                } else {
                    state
                        .asset_references()
                        .iter()
                        .map(|reference| hex_lattice_contract::DeviceAssetReference {
                            slot: reference.slot,
                            asset: if reference.slot == source_slot {
                                key
                            } else {
                                reference.asset
                            },
                        })
                        .collect()
                };
                let state = state
                    .with_asset_references(references)
                    .map_err(|_| Invalid("invalid device source binding"))?;
                self.apply(Command::ReplaceDeviceState { device, state })?;
                Ok(Created::Asset(asset))
            }
            Command::RelinkAssets(relinks) => {
                if relinks.is_empty() || relinks.len() > crate::MAX_ASSETS {
                    return Err(Invalid(
                        "relink needs between one and the asset limit of files",
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for (id, path) in &relinks {
                    if !seen.insert(*id) {
                        return Err(Invalid("relink names an asset twice"));
                    }
                    if !self.asset_offline(*id) {
                        return Err(Invalid("only missing media can be relinked"));
                    }
                    if !path.is_absolute() {
                        return Err(Invalid("relinked media path must be absolute"));
                    }
                    let asset = self
                        .assets
                        .get(id)
                        .ok_or(Invalid("media no longer exists"))?;
                    if asset.original_source().is_some() {
                        return Err(Invalid("device media cannot be relinked in place"));
                    }
                }
                for (id, path) in relinks {
                    if let Some(asset) = self.assets.get_mut(&id) {
                        asset.path = path;
                    }
                    self.offline_assets.remove(&id);
                }
                Ok(Created::None)
            }
            Command::RemoveAsset(id) => {
                if self.device_uses_asset(id) {
                    return Err(Invalid("media is still referenced by a device"));
                }
                if self
                    .tracks
                    .iter()
                    .flat_map(Track::clips)
                    .any(|c| c.referenced_assets().any(|asset| asset == id))
                    || self.session.cells.iter().any(|cell| matches!(&cell.content, crate::SessionCellContent::Audio(clip) if clip.asset == id))
                {
                    return Err(Invalid("media is still referenced by a clip"));
                }
                self.assets
                    .remove(&id)
                    .ok_or(Invalid("media no longer exists"))?;
                self.offline_assets.remove(&id);
                Ok(Created::None)
            }
            Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset,
                source_length,
                looping,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                let media = self.asset(asset).ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                let fade = ClipFade::neutral(duration)?;
                let incoming = Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id: ClipId(0),
                    asset,
                    start,
                    duration,
                    source_offset,
                    source_length,
                    looping,
                    source_phase: 0,
                    fade,
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                };
                crate::clip_group::place_one(
                    self,
                    track,
                    crate::ClipGroupContent::Audio(incoming),
                    true,
                )
            }
            Command::MoveClip { clip, track, start } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                self.track(track)
                    .ok_or(Invalid("destination track no longer exists"))?;
                let source = self
                    .tracks
                    .iter()
                    .position(|t| t.clips().iter().any(|c| c.id == clip))
                    .ok_or(Invalid("clip no longer exists"))?;
                if self.tracks[source].id == track && original.start == start {
                    return Ok(Created::None);
                }
                let mut moved = self
                    .clip(clip)
                    .cloned()
                    .ok_or(Invalid("audio clip is missing"))?;
                crate::audio_edit::move_clip(&mut moved, start, self.tempo_map())?;
                crate::clip_group::place_one(
                    self,
                    track,
                    crate::ClipGroupContent::Audio(moved),
                    false,
                )
            }
            Command::MoveClipsWithOverwrite {
                clips,
                anchor,
                track,
                position,
                copy,
            } => crate::clip_group::apply(self, clips, anchor, track, position, copy),
            Command::MoveClipWithOverwrite { clip, track, start } => {
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::MoveClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                self.apply(Command::MoveClip { clip, track, start })?;
                Ok(Created::Track(track))
            }
            Command::RemoveDevices { owner, selected } => {
                crate::device_clipboard::remove(self, owner, &selected)
            }
            Command::PasteDevices {
                clipboard,
                owner,
                before,
            } => crate::device_clipboard::paste(self, &clipboard, owner, before),
            Command::PasteTracks {
                clipboard,
                destination,
            } => crate::track_clipboard::paste(self, &clipboard, destination),
            Command::PasteClips {
                clipboard,
                track,
                frame,
                insert,
            } => crate::clip_clipboard::paste(self, &clipboard, track, frame, insert),
            Command::CopyClipTo { clip, track, start } => {
                crate::clip_copy::audio_to(self, clip, track, start)
            }
            Command::CopyClipToNewTrack {
                clip,
                name,
                before_track,
                start,
                initial_gain,
            } => {
                let track = insert_track_before_with_gain(self, name, before_track, initial_gain)?;
                crate::clip_copy::audio_to(self, clip, track, start)?;
                Ok(Created::Track(track))
            }
            Command::TrimClip {
                clip,
                start,
                duration,
                source_offset,
                source_length,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let media = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                if original.edit.is_some() {
                    return crate::audio_edit::reset_region(
                        self,
                        clip,
                        start,
                        duration,
                        source_offset,
                        source_length,
                    );
                }
                let old_fade = original.fade;
                let old_source_offset = original.source_offset;
                let old_source_length = original.source_length;
                let fade = old_fade.reanchor(duration)?;
                let mut target = original.clone();
                target.start = start;
                target.duration = duration;
                target.source_offset = source_offset;
                target.source_length = source_length;
                if source_offset != old_source_offset || source_length != old_source_length {
                    target.source_phase = 0;
                }
                target.fade = fade;
                crate::clip_group::replace_audio(self, target)
            }
            Command::TrimClipEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed = if original.edit.is_some() {
                    crate::audio_edit::trim_view(original, edge, frame, self.tempo_map())?
                } else {
                    preview_edge_trim(original, edge, frame, asset_frames)?
                };
                crate::clip_group::replace_audio(self, trimmed)
            }
            Command::TrimClipFadeEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed =
                    original.preview_fade_edge_trim(edge, frame, asset_frames, self.tempo_map())?;
                crate::clip_group::replace_audio(self, trimmed)
            }
            Command::ResizeAudioClipView { clip, operation } => {
                crate::audio_edit::resize_view(self, clip, operation)
            }
            command @ (Command::SetAudioClipPitch { .. }
            | Command::SetAudioClipMusical { .. }
            | Command::SetAudioClipWarp { .. }
            | Command::SetAudioClipSampleBpm { .. }
            | Command::StretchAudioClipEdge { .. }
            | Command::SetAudioClipSourceGain { .. }) => crate::audio_edit::apply(self, command),
            Command::SetAudioClipGain { clip, millidb } => {
                let gain = crate::ClipGain::new(millidb)?;
                if self.clip(clip).is_some() {
                    self.clip_mut(clip)?.gain = gain;
                } else {
                    let target = self
                        .session
                        .cells
                        .iter_mut()
                        .find_map(|cell| {
                            if let crate::SessionCellContent::Audio(audio) = &mut cell.content {
                                (audio.id == clip).then_some(audio)
                            } else {
                                None
                            }
                        })
                        .ok_or(Invalid("audio clip no longer exists"))?;
                    target.gain = gain;
                }
                Ok(Created::None)
            }
            Command::CreateArrangementFades { range } => {
                crate::selection_fades::arrangement(self, &range)
            }
            Command::CreateAudioEventFades { clip, start, end } => {
                crate::selection_fades::event(self, clip, start, end)
            }
            Command::SetClipFadeEnvelope { clip, fade } => {
                let target = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if fade.span() != target.duration || fade.offset() != 0 {
                    return Err(Invalid("edited fade must match the current clip"));
                }
                if target.edit.is_some() {
                    return crate::audio_edit::set_fade_envelope(self, clip, fade);
                }
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
            } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::set_fades(self, clip, fade_in, fade_out);
                }
                let duration = self
                    .clip(clip)
                    .ok_or(Invalid("clip no longer exists"))?
                    .duration;
                let fade = ClipFade::new(duration, 0, fade_in, fade_out)?;
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipLooping { clip, looping } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, Some(looping), None);
                }
                let target = self.clip_mut(clip)?;
                if target.looping != looping {
                    target.looping = looping;
                    if !looping {
                        target.source_phase = 0;
                    }
                }
                Ok(Created::None)
            }
            Command::SetClipSourcePhase { clip, phase } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, None, Some(phase));
                }
                let target = self.clip_mut(clip)?;
                if !target.looping {
                    return Err(Invalid("source phase can only be set on a looping clip"));
                }
                if phase >= target.source_length {
                    return Err(Invalid("source phase must be inside the source interval"));
                }
                target.source_phase = phase;
                Ok(Created::None)
            }
            Command::RemoveClip(id) => {
                for track in &mut self.tracks {
                    if let Some(index) = track.clips().iter().position(|c| c.id == id) {
                        track.content.audio_clips.remove(index);
                        return Ok(Created::None);
                    }
                }
                Err(Invalid("clip no longer exists"))
            }
        }
    }
}

impl Project {
    /// Validate a new import member's track and metadata, then add its asset.
    fn add_import_asset(
        &mut self,
        track: TrackId,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    ) -> Result<AssetId, Invalid> {
        self.track(track).ok_or(Invalid("track no longer exists"))?;
        self.add_prepared_asset(
            path,
            source_sample_rate,
            source_frames,
            playback_frames,
            conversion_version,
        )
    }

    pub(crate) fn add_prepared_asset(
        &mut self,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    ) -> Result<AssetId, Invalid> {
        if path.as_os_str().is_empty() {
            return Err(Invalid("media needs a path and frames"));
        }
        validate_conversion_metadata(
            source_frames,
            source_sample_rate,
            playback_frames,
            self.sample_rate,
            conversion_version,
        )?;
        if self.assets.len() >= MAX_ASSETS {
            return Err(Invalid("asset limit reached"));
        }
        let id = AssetId(self.allocate_id()?);
        self.assets.insert(
            id,
            Asset {
                original_source: None,
                id,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            },
        );
        Ok(id)
    }
}

impl Command {
    /// Structural section edits use fenced resident preparation/publication.
    /// Metadata edits and marker merging do not alter musical material.
    pub fn requires_section_publication(&self) -> bool {
        matches!(
            self,
            Self::ReorderSectionBefore { .. }
                | Self::ReorderSectionAfter { .. }
                | Self::ResizeArrangementSection { .. }
                | Self::ResizeArrangementSectionEdge { .. }
                | Self::ResizeArrangementSectionBoundary {
                    marker_only: false,
                    ..
                }
                | Self::DeleteArrangementSection(_)
                | Self::DuplicateArrangementSection(_)
                | Self::DuplicateArrangementSectionAt { .. }
                | Self::AppendArrangementSectionCopy { .. }
                | Self::MoveArrangementSectionBeyondEnd { .. }
                | Self::ReplaceArrangementSection { .. }
        )
    }
}

#[cfg(test)]
mod overwrite_tests {
    use super::{Command, Created, insert_track_before};
    use crate::{Clip, ClipFade, MAX_CLIPS, Project};
    use std::path::PathBuf;

    #[test]
    fn audio_overwrite_clip_limit_failure_preserves_candidate_exactly() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let source = insert_track_before(&mut project, "source".into(), None).expect("source");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let _victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 10,
                source_offset: 0,
                source_length: 10,
                looping: true,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let incoming = match project
            .apply(Command::PlaceClip {
                track: source,
                asset,
                start: 0,
                duration: 2,
                source_offset: 20,
                source_length: 2,
                looping: false,
            })
            .expect("incoming")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected incoming: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 2;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        let before = project.clone();
        assert!(
            project
                .apply(Command::MoveClipWithOverwrite {
                    clip: incoming,
                    track: target,
                    start: 4,
                })
                .is_err()
        );
        assert_eq!(project, before);
    }

    #[test]
    fn prepared_audio_overwrite_can_replace_fully_covered_clip_at_clip_limit() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 4,
                source_offset: 0,
                source_length: 4,
                looping: false,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 1;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    active: true,
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        assert_eq!(
            project.track(target).expect("target").clips().len(),
            MAX_CLIPS
        );
        let created = project
            .apply(Command::ImportPreparedAudioClipWithOverwrite {
                track: target,
                start: 0,
                path: PathBuf::from("replacement.wav"),
                source_sample_rate: 48_000,
                source_frames: 4,
                playback_frames: 4,
                conversion_version: 0,
            })
            .expect("full-cover replacement at the clip limit");
        let Created::Imported { clip, .. } = created else {
            panic!("expected imported clip, got {created:?}");
        };
        let clips = project.track(target).expect("target").clips();
        assert_eq!(clips.len(), MAX_CLIPS);
        assert!(!clips.iter().any(|candidate| candidate.id() == victim));
        assert!(clips.iter().any(|candidate| candidate.id() == clip));
    }
}

#[cfg(test)]
#[path = "track_remove_tests.rs"]
mod track_remove_tests;
