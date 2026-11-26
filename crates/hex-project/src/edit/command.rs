//! Canonical edit command, placement and creation-receipt types.
use crate::{AssetId, ClipFade, ClipId, DeviceId, NoteId, ReturnId, TrackId, TrimEdge};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDeviceSlot {
    Instrument,
    Effect,
}

/// Placement of an instrument loaded with a newly imported source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentLoadPlacement {
    First,
    Append,
    Replace(DeviceId),
}

/// Destination for a new instrument loaded with an original asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentLoadDestination {
    Track {
        track: TrackId,
        placement: InstrumentLoadPlacement,
    },
    NewTrack {
        before_track: Option<TrackId>,
    },
}

/// Owner captured for an atomic original-asset replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstrumentAssetOwner {
    /// The instrument currently owned by this track.
    Track(TrackId),
    /// A retained source that is not linked to any track.
    Source(crate::SourceId),
}

impl InstrumentLoadPlacement {
    pub fn replacement(self) -> Option<DeviceId> {
        match self {
            Self::Replace(device) => Some(device),
            Self::First | Self::Append => None,
        }
    }
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
    /// Group selected subtrees in display order at the last member's position
    /// and parent. A selected ancestor owns the position of its descendants.
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
    /// Place a linked Arrangement section referencing a canonical Console scene.
    PlaceLinkedScene {
        scene: crate::SceneId,
        start: crate::BeatTime,
        end: crate::BeatTime,
        color: crate::ElementColor,
    },
    /// Clear the Console scene link on one Arrangement section without altering its range or content.
    UnlinkSectionScene(crate::SectionId),
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
    /// Remove the section's entire half-open time interval, including device
    /// automation, and close the gap while preserving both sides of the splice.
    DeleteArrangementSection(crate::SectionId),
    /// Insert a song-wide copy, including device envelopes and touch spans,
    /// immediately after the source, allocating new IDs.
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
    /// Move or copy a complete song section to replace a target section.
    /// Moves retain source identity; copies receive a fresh ID.
    ReplaceArrangementSection {
        source: crate::SectionId,
        target: crate::SectionId,
        copy: bool,
    },
    /// Insert a captured song section at `at`, rippling later music, markers,
    /// tempo, meter, locators and device automation right. Splits an enclosing
    /// section at `at`, or marks an intervening gap past the final section.
    PasteArrangementSection {
        clipboard: std::sync::Arc<crate::SectionClipboard>,
        at: crate::BeatTime,
        gap_name: String,
        gap_color: crate::ElementColor,
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
    AddArrangementLocator {
        frame: u64,
    },
    MoveArrangementLocator {
        locator: crate::LocatorId,
        frame: u64,
    },
    RenameArrangementLocator {
        locator: crate::LocatorId,
        name: Option<String>,
    },
    RemoveArrangementLocator(crate::LocatorId),
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
    /// Replace or clear a track mixer automation lane (balance/pan, mute, or send level).
    SetTrackMixerAutomation {
        track: TrackId,
        target: crate::MixerLaneTarget,
        points: Vec<crate::MixerAutomationPoint>,
    },
    /// Freeze an ordinary track by adopting a newly rendered project-rate WAV asset,
    /// leaving all original clips, MIDI, instrument, devices, and automation intact.
    FreezeTrack {
        track: TrackId,
        path: PathBuf,
        start: u64,
        duration: u64,
        frames: u64,
    },
    /// Freeze an ordinary track using an already registered project-rate asset.
    FreezeTrackWithAsset {
        track: TrackId,
        asset: AssetId,
        start: u64,
        duration: u64,
    },
    /// Unfreeze a frozen track, restoring live playback of its untouched original content.
    UnfreezeTrack {
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
    /// Assign or clear a track's single selected stereo audio input in one atomic edit.
    AssignTrackAudioInput {
        destination: TrackId,
        source: Option<crate::RoutingOwner>,
    },
    /// Set a track's exclusive post-fader dry output destination.
    SetTrackOutput {
        track: TrackId,
        destination: crate::TrackOutputDestination,
    },
    /// Set a track's canonical input monitoring mode (In / Auto / Off).
    SetTrackInputMonitoring {
        track: TrackId,
        mode: crate::InputMonitoring,
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
    /// Replace one exact top-level Main effect with a fresh authored identity.
    ReplaceMainDevice {
        replacing: DeviceId,
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
    /// Replace one direct effect on an ordinary track with a fresh identity.
    /// The local ordinal and the separately authored instrument gap survive.
    ReplaceTrackDevice {
        track: TrackId,
        replacing: DeviceId,
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
    /// Replace the exact instrument on its receiving track with a fresh device identity.
    /// The source binding and effects survive, and the track's instrument comes first.
    ReplaceTrackInstrument {
        track: TrackId,
        replacing: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
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
    /// Append a selection from another track in canonical source order as one edit.
    MoveTrackDevices {
        source: TrackId,
        selected: crate::DeviceSelectionIds,
        destination: TrackId,
    },
    /// Move the selected units together to a gap in their current chain.
    ReorderDevices {
        owner: crate::DeviceChainOwner,
        selected: crate::DeviceSelectionIds,
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
    /// Replace one exact top-level Return effect with a fresh authored identity.
    ReplaceReturnDevice {
        return_track: ReturnId,
        replacing: DeviceId,
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
    /// Set original Hex stereo balance.
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
    /// One prepared import binds the source to a new instrument on an existing
    /// or newly inserted track. Exact replacements use the native placement rule.
    /// The unbound state template has no asset references; the new source
    /// ID is bound inside this candidate, never predicted by a UI caller.
    ImportDeviceAssetIntoNewInstrument {
        destination: InstrumentLoadDestination,
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
        state: hex_lattice_contract::NativeDeviceState,
        source_slot: hex_lattice_contract::DeviceAssetSlot,
    },
    /// Atomically import an original source and bind or rebind an existing instrument.
    /// Captured identity/state/provider must still match; no IDs are predicted.
    ImportDeviceAssetReplacingInstrument {
        owner: InstrumentAssetOwner,
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
    /// Consolidate one or more same-track, same-kind clips into one local
    /// timeline from zero. Preserve original audio sources and other placements.
    GroupClips(Vec<ClipId>),
    /// Split selection edges and group included material independently per track,
    /// retaining the full selected extent including empty margins and internal gaps.
    /// Empty tracks are skipped; returns the last populated track's resulting clip.
    /// All fragments and groups form one atomic edit; mixed media on a row refuses.
    GroupTimeRange(crate::TimeRange),
    /// Group populated lanes, or place blank MIDI clips if the entire range is empty.
    GroupTimeRangeOrCreate(crate::TimeRange),
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
    /// Configure a bounded deterministic next-scene follow action on a Console scene.
    SetSceneFollowAction {
        scene: crate::SceneId,
        action: crate::SceneFollowAction,
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
    /// Atomically place a new Arrangement MIDI clip and author its recorded notes
    /// in one history step, cutting any overlapped clips on the target track.
    PlaceMidiClipWithNotes {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
        notes: Vec<MidiNoteInsert>,
    },
    /// Atomically place a new Arrangement MIDI clip and author expressive recorded
    /// notes (with per-note `NoteExpression` and bounded `NoteExpressionPoint`s)
    /// in one history step, cutting any overlapped clips on the target track.
    PlaceMidiClipWithExpressiveNotes {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
        notes: Vec<MidiExpressiveNoteInsert>,
    },
    /// Atomically extend a non-looping forward MIDI clip (and its source extent if
    /// needed) and author recorded notes in one history step.
    ExtendMidiClipWithNotes {
        clip: ClipId,
        length: crate::BeatTime,
        notes: Vec<MidiNoteInsert>,
    },
    /// Atomically extend a non-looping forward MIDI clip (and its source extent if
    /// needed) and author expressive recorded notes in one history step.
    ExtendMidiClipWithExpressiveNotes {
        clip: ClipId,
        length: crate::BeatTime,
        notes: Vec<MidiExpressiveNoteInsert>,
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
    /// Replace, add, or clear (when `points` is empty) one clip-local MIDI CC
    /// or Pitch Bend expression lane in source-local beat coordinates.
    SetMidiExpressionLane {
        clip: ClipId,
        kind: crate::MidiLaneKind,
        points: Vec<crate::MidiLanePoint>,
    },
    /// Set static and/or time-varying per-note MPE expression (pitch offset in
    /// cents, timbre/CC74, pressure) for one note inside a MIDI clip.
    SetMidiNoteExpression {
        clip: ClipId,
        note: NoteId,
        expression: crate::NoteExpression,
        points: Vec<crate::NoteExpressionPoint>,
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
    /// Author a bounded batch of MIDI notes with per-note expression and
    /// expression-curve points into an existing Arrangement or Console MIDI clip
    /// in one atomic history step.
    AddMidiNotesWithExpression {
        clip: ClipId,
        notes: Vec<MidiExpressiveNoteInsert>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiExpressiveNoteInsert {
    pub active: bool,
    pub start: crate::BeatTime,
    pub duration: crate::BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
    pub expression: crate::NoteExpression,
    pub expression_points: Vec<crate::NoteExpressionPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    NativeSource(crate::SourceId),
    ArrangementSection(crate::SectionId),
    ArrangementLocator(crate::LocatorId),
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
                | Self::PasteArrangementSection { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_load_placement_reports_replacement_target_only_for_replace() {
        let device = DeviceId(7);
        assert_eq!(InstrumentLoadPlacement::First.replacement(), None);
        assert_eq!(InstrumentLoadPlacement::Append.replacement(), None);
        assert_eq!(
            InstrumentLoadPlacement::Replace(device).replacement(),
            Some(device)
        );
    }

    #[test]
    fn requires_section_publication_distinguishes_structural_from_marker_only_edits() {
        let structural = Command::DeleteArrangementSection(crate::SectionId(1));
        assert!(structural.requires_section_publication());

        let marker_only = Command::ResizeArrangementSectionBoundary {
            left: crate::SectionId(1),
            right: crate::SectionId(2),
            boundary: crate::BeatTime::from_quarters(4).unwrap(),
            marker_only: true,
        };
        assert!(!marker_only.requires_section_publication());

        let non_section = Command::ToggleLoopRegion;
        assert!(!non_section.requires_section_publication());
    }
}
