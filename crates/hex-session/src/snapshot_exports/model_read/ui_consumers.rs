//! Finite borrowed facts used by UI consumers of a retained project snapshot.
//!
//! These wrappers keep every model borrow tied to the enclosing snapshot read.
//! They expose copyable values and iterators over purpose-specific wrappers;
//! no project, source, edit root, asset owner, or mutable runtime state escapes.
#[cfg(test)]
#[path = "ui_consumers/trim_tests.rs"]
mod trim_tests;
use super::{
    SnapshotClipRead, SnapshotDeviceRead, SnapshotProjectRead, SnapshotRead, SnapshotTrackRead,
};
use hex_project::{
    AssetId, BeatTime, ClipFade, ClipFlavor, ClipId, GainTouchStep, Meter, MeterPoint,
    MeterPosition, MidiNote, NoteId, SceneId, Tempo, TempoPoint, TrackId,
};

#[derive(Clone, Copy)]
pub struct SnapshotMidiClipRead<'a> {
    clip: &'a hex_project::MidiClip,
    track: TrackId,
}

#[derive(Clone, Copy)]
pub struct SnapshotMidiNoteRead<'a> {
    note: &'a MidiNote,
}

#[derive(Clone, Copy)]
pub struct SnapshotMidiSourceRead<'a> {
    source: &'a hex_project::MidiSource,
}

/// Drawing facts from this exact retained prepared preview, never its owner.
#[derive(Clone, Copy)]
pub struct SnapshotMidiClipPreviewRead<'a> {
    preview: &'a hex_engine::MidiClipPreview,
}

#[derive(Clone, Copy)]
pub struct SnapshotSceneRead<'a> {
    scene: &'a hex_project::Scene,
}

#[derive(Clone, Copy)]
pub struct SnapshotSessionCellRead<'a> {
    cell: &'a hex_project::SessionCell,
}

#[derive(Clone, Copy)]
pub enum SnapshotSessionCellContentRead<'a> {
    Empty,
    Stop,
    Midi(SnapshotSessionMidiClipRead<'a>),
    Audio(SnapshotSessionAudioClipRead<'a>),
}

#[derive(Clone, Copy)]
pub struct SnapshotSessionMidiClipRead<'a> {
    clip: &'a hex_project::SessionMidiClip,
}

#[derive(Clone, Copy)]
pub struct SnapshotSessionAudioClipRead<'a> {
    clip: &'a hex_project::SessionAudioClip,
}

#[derive(Clone, Copy)]
pub struct SnapshotTempoRead<'a> {
    map: &'a hex_project::TempoMap,
}

#[derive(Clone, Copy)]
pub struct SnapshotMeterRead<'a> {
    map: &'a hex_project::MeterMap,
}

#[derive(Clone, Copy)]
pub struct SnapshotAssetRead<'a> {
    asset: &'a hex_project::Asset,
}

#[derive(Clone, Copy)]
pub struct SnapshotAudioEditRead<'a> {
    edit: &'a hex_project::AudioClipEdit,
}

#[derive(Clone, Copy)]
pub struct SnapshotGainSpanRead<'a> {
    span: &'a hex_project::GainTouchSpan,
}

impl<'a> SnapshotRead<'a> {
    /// This exact snapshot's prepared clip summary and scalar source window.
    pub fn clip_waveform(self, clip: ClipId) -> Option<SnapshotClipWaveformRead<'a>> {
        self.snapshot
            .audio_clip_waveforms
            .get(&clip)
            .map(|prepared| SnapshotClipWaveformRead { prepared })
    }

    /// Prospective audio geometry from this exact snapshot's clip, asset and tempo.
    pub fn with_audio_clip_preview<R>(
        self,
        id: ClipId,
        operation: hex_project::AudioClipPreviewOperation,
        frame: u64,
        inspect: impl for<'read> FnOnce(hex_project::AudioClipPreviewRead<'read>) -> R,
    ) -> Result<Option<R>, hex_project::Invalid> {
        let project = &self.snapshot.project;
        let Some(clip) = project.clip(id) else {
            return Ok(None);
        };
        let asset = project
            .asset(clip.asset())
            .ok_or(hex_project::Invalid("audio clip references missing media"))?;
        clip.with_audio_preview(
            operation,
            frame,
            asset.playback_frames(),
            project.tempo_map(),
            inspect,
        )
        .map(Some)
    }

    /// Current/default root metadata only; no owning edit or domain is captured.
    pub fn with_audio_event_root<R>(
        self,
        id: ClipId,
        inspect: impl for<'read> FnOnce(hex_project::AudioClipRootRead<'read>) -> R,
    ) -> Result<Option<R>, hex_project::Invalid> {
        let project = &self.snapshot.project;
        let Some(clip) = project.clip(id) else {
            return Ok(None);
        };
        project
            .asset(clip.asset())
            .ok_or(hex_project::Invalid("audio clip references missing media"))?;
        Ok(Some(
            clip.with_audio_event_root(project.tempo_map(), inspect),
        ))
    }

    pub fn default_audio_warp_markers(
        self,
        id: ClipId,
    ) -> Result<Option<[hex_project::WarpMarker; 2]>, hex_project::Invalid> {
        let project = &self.snapshot.project;
        let Some(clip) = project.clip(id) else {
            return Ok(None);
        };
        project
            .asset(clip.asset())
            .ok_or(hex_project::Invalid("audio clip references missing media"))?;
        hex_project::default_audio_warp_markers(clip, project.tempo_map()).map(Some)
    }

    pub fn midi_clip_frame_bounds(self, id: ClipId) -> Option<(u64, u64)> {
        self.midi_clip_preview(id)
            .map(|preview| preview.frame_bounds())
    }

    pub fn midi_clip_preview(self, id: ClipId) -> Option<SnapshotMidiClipPreviewRead<'a>> {
        self.snapshot
            .midi_preview
            .get(id)
            .map(|preview| SnapshotMidiClipPreviewRead { preview })
    }

    /// The existing canonical gain-preview law, scoped to the same snapshot.
    pub fn gain_preview_at(self, track: TrackId, frame: u64) -> Option<f32> {
        let source = self.snapshot.project.track(track)?;
        if source.gain_envelope().is_empty() && source.gain_lane().spans().is_empty() {
            Some(source.gain())
        } else {
            self.snapshot
                .gain_preview
                .as_ref()
                .ok()?
                .value_at(track, frame)
        }
    }

    pub fn gain_preview_error(self) -> Option<&'static str> {
        self.snapshot
            .gain_preview
            .as_ref()
            .err()
            .map(|error| error.0)
    }

    /// `None` means preparation failed; `Some(false)` means no prepared
    /// automation for this track, preserving the authored static-gain fallback.
    /// The visitor receives only Copy facts; finish this read before GPUI work.
    pub fn visit_gain_preview_segments(
        self,
        track: TrackId,
        start: u64,
        end: u64,
        visit: impl FnMut(hex_engine::GainPreviewSegment),
    ) -> Option<bool> {
        self.snapshot
            .gain_preview
            .as_ref()
            .ok()
            .map(|preview| preview.visit_segments(track, start, end, visit))
    }
}

/// Borrowed prepared summary queries; no waveform or PCM owner is exposed.
#[derive(Clone, Copy)]
pub struct SnapshotClipWaveformRead<'a> {
    prepared: &'a crate::service::AudioClipWaveform,
}

impl<'a> SnapshotClipWaveformRead<'a> {
    pub fn waveform(self) -> crate::WaveformRead<'a> {
        crate::read_shared_waveform(&self.prepared.waveform)
    }
    pub fn offset(self) -> u64 {
        self.prepared.offset
    }
    pub fn length(self) -> u64 {
        self.prepared.length
    }
}

impl<'a> SnapshotMidiClipPreviewRead<'a> {
    pub fn frame_bounds(self) -> (u64, u64) {
        (self.preview.frame_start, self.preview.frame_end)
    }

    pub fn authored_note_count(self) -> usize {
        self.preview.authored_note_count
    }

    pub fn occurrence_count(self) -> usize {
        self.preview.occurrence_count
    }

    pub fn samples(self) -> impl ExactSizeIterator<Item = hex_engine::MidiPreviewSample> + 'a {
        self.preview.samples.iter().copied()
    }
}

impl<'a> SnapshotProjectRead<'a> {
    pub fn has_assets(self) -> bool {
        self.project.assets().next().is_some()
    }
    pub fn clip_flavor(self, id: ClipId) -> Option<ClipFlavor> {
        self.project.clip_flavor(id)
    }

    pub fn authored_routing(self) -> impl ExactSizeIterator<Item = hex_project::ReturnFeed> + 'a {
        self.project.authored_routing().iter().copied()
    }

    pub fn audio_feeds(self) -> impl ExactSizeIterator<Item = hex_project::AudioFeed> + 'a {
        self.project.audio_feeds().iter().copied()
    }

    pub fn midi_routes(self) -> impl ExactSizeIterator<Item = hex_project::MidiRoute> + 'a {
        self.project.midi_routes().iter().copied()
    }

    /// MIDI Arrangement clip lookup. Session clips are exposed by `session_cell`.
    pub fn midi_clip(self, id: ClipId) -> Option<SnapshotMidiClipRead<'a>> {
        self.project.tracks().iter().find_map(|track| {
            track
                .midi_clips()
                .iter()
                .find(|clip| clip.id() == id)
                .map(|clip| SnapshotMidiClipRead {
                    clip,
                    track: track.id(),
                })
        })
    }

    pub fn midi_note(self, id: NoteId) -> Option<SnapshotMidiNoteRead<'a>> {
        self.project
            .midi_note(id)
            .map(|note| SnapshotMidiNoteRead { note })
    }

    /// Resolve the current Arrangement or Session source without exposing its owner.
    pub fn midi_source(self, id: ClipId) -> Option<SnapshotMidiSourceRead<'a>> {
        self.project
            .midi_source(id)
            .map(|source| SnapshotMidiSourceRead { source })
    }

    pub fn scenes(self) -> impl ExactSizeIterator<Item = SnapshotSceneRead<'a>> + 'a {
        self.project
            .scenes()
            .iter()
            .map(|scene| SnapshotSceneRead { scene })
    }

    pub fn scene(self, id: SceneId) -> Option<SnapshotSceneRead<'a>> {
        self.project
            .scenes()
            .iter()
            .find(|scene| scene.id() == id)
            .map(|scene| SnapshotSceneRead { scene })
    }

    pub fn session_cells(self) -> impl ExactSizeIterator<Item = SnapshotSessionCellRead<'a>> + 'a {
        self.project
            .session_cells()
            .iter()
            .map(|cell| SnapshotSessionCellRead { cell })
    }

    /// Missing cells remain distinguishable from explicit stop cells.
    pub fn session_cell(
        self,
        track: TrackId,
        scene: SceneId,
    ) -> SnapshotSessionCellContentRead<'a> {
        match self.project.session_cell(track, scene) {
            None => SnapshotSessionCellContentRead::Empty,
            Some(hex_project::SessionCellContent::Stop) => SnapshotSessionCellContentRead::Stop,
            Some(hex_project::SessionCellContent::Midi(clip)) => {
                SnapshotSessionCellContentRead::Midi(SnapshotSessionMidiClipRead { clip })
            }
            Some(hex_project::SessionCellContent::Audio(clip)) => {
                SnapshotSessionCellContentRead::Audio(SnapshotSessionAudioClipRead { clip })
            }
        }
    }

    pub fn session_midi_clip(self, id: ClipId) -> Option<SnapshotSessionMidiClipRead<'a>> {
        self.project
            .session_midi_clip(id)
            .map(|clip| SnapshotSessionMidiClipRead { clip })
    }

    pub fn session_audio_clip(self, id: ClipId) -> Option<SnapshotSessionAudioClipRead<'a>> {
        self.project
            .session_audio_clip(id)
            .map(|clip| SnapshotSessionAudioClipRead { clip })
    }

    pub fn asset(self, id: AssetId) -> Option<SnapshotAssetRead<'a>> {
        self.project
            .asset(id)
            .map(|asset| SnapshotAssetRead { asset })
    }

    pub fn tempo(self) -> SnapshotTempoRead<'a> {
        SnapshotTempoRead {
            map: self.project.tempo_map(),
        }
    }

    pub fn meter(self) -> SnapshotMeterRead<'a> {
        SnapshotMeterRead {
            map: self.project.meter_map(),
        }
    }
}

/// Canonical chain class and scoped device borrow, in the model's persisted order.
#[derive(Clone, Copy)]
pub enum SnapshotTrackDeviceRead<'a> {
    Instrument(SnapshotDeviceRead<'a>),
    Effect(SnapshotDeviceRead<'a>),
}

impl<'a> SnapshotTrackRead<'a> {
    pub fn ordered_device_chain(self) -> impl Iterator<Item = SnapshotTrackDeviceRead<'a>> + 'a {
        self.track.ordered_device_chain().map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device) => {
                SnapshotTrackDeviceRead::Instrument(SnapshotDeviceRead {
                    device,
                    class: hex_engine::NativeDeviceClass::Instrument,
                })
            }
            hex_project::TrackDeviceRef::Effect(device) => {
                SnapshotTrackDeviceRead::Effect(SnapshotDeviceRead {
                    device,
                    class: hex_engine::NativeDeviceClass::Effect,
                })
            }
        })
    }
    pub fn sends(self) -> impl ExactSizeIterator<Item = hex_project::SendLevel> + 'a {
        self.track.sends().iter().copied()
    }

    pub fn color(self) -> hex_project::ElementColor {
        self.track.color()
    }

    pub fn midi_clips(self) -> impl ExactSizeIterator<Item = SnapshotMidiClipRead<'a>> + 'a {
        let track_id = self.track.id();
        self.track
            .midi_clips()
            .iter()
            .map(move |clip| SnapshotMidiClipRead {
                clip,
                track: track_id,
            })
    }

    pub fn gain_spans(self) -> impl ExactSizeIterator<Item = SnapshotGainSpanRead<'a>> + 'a {
        self.track
            .gain_lane()
            .spans()
            .iter()
            .map(|span| SnapshotGainSpanRead { span })
    }
}

impl<'a> SnapshotClipRead<'a> {
    pub fn content_token_capacity(self) -> Result<usize, hex_project::Invalid> {
        self.clip.content_token_capacity()
    }
    pub fn capture_content(
        self,
        token: &mut hex_project::ClipContentToken,
    ) -> Result<(), hex_project::Invalid> {
        self.clip.capture_content(token)
    }
    pub fn matches_content(self, token: &hex_project::ClipContentToken) -> bool {
        self.clip.matches_content(token)
    }
    pub fn flavor(self) -> ClipFlavor {
        self.clip.flavor()
    }

    /// Original source phase, including looped clip starts.
    pub fn source_phase(self) -> u64 {
        self.clip.source_phase()
    }

    /// Authored retained-domain fade. This is deliberately distinct from `fade()`.
    pub fn retained_fade(self) -> ClipFade {
        self.clip.retained_fade()
    }

    pub fn audio_edit(self) -> Option<SnapshotAudioEditRead<'a>> {
        self.clip
            .audio_edit()
            .map(|edit| SnapshotAudioEditRead { edit })
    }
}

impl<'a> SnapshotMidiClipRead<'a> {
    pub fn preview_edge_trim(
        self,
        edge: hex_project::TrimEdge,
        beat: BeatTime,
    ) -> Result<hex_project::MidiClipTrim, hex_project::Invalid> {
        self.clip.preview_edge_trim(edge, beat)
    }
    pub fn content_token_capacity(self) -> Result<usize, hex_project::Invalid> {
        self.clip.content_token_capacity()
    }
    pub fn capture_content(
        self,
        token: &mut hex_project::ClipContentToken,
    ) -> Result<(), hex_project::Invalid> {
        self.clip.capture_content(token)
    }
    pub fn matches_content(self, token: &hex_project::ClipContentToken) -> bool {
        self.clip.matches_content(token)
    }
    pub fn id(self) -> ClipId {
        self.clip.id()
    }
    pub fn track(self) -> TrackId {
        self.track
    }
    pub fn name(self) -> &'a str {
        self.clip.name()
    }
    pub fn flavor(self) -> ClipFlavor {
        self.clip.flavor()
    }
    pub fn start(self) -> BeatTime {
        self.clip.start()
    }
    pub fn arrangement_length(self) -> BeatTime {
        self.clip.arrangement_length()
    }
    pub fn source_length(self) -> BeatTime {
        self.clip.source_length()
    }
    pub fn source_start(self) -> BeatTime {
        self.clip.source_start()
    }
    pub fn loop_start(self) -> BeatTime {
        self.clip.loop_start()
    }
    pub fn loop_end(self) -> BeatTime {
        self.clip.loop_end()
    }
    pub fn looping(self) -> bool {
        self.clip.looping()
    }
    pub fn notes(self) -> impl ExactSizeIterator<Item = SnapshotMidiNoteRead<'a>> + 'a {
        self.clip
            .notes()
            .iter()
            .map(|note| SnapshotMidiNoteRead { note })
    }
}

impl SnapshotMidiNoteRead<'_> {
    pub fn id(self) -> NoteId {
        self.note.id()
    }
    pub fn start(self) -> BeatTime {
        self.note.start()
    }
    pub fn duration(self) -> BeatTime {
        self.note.duration()
    }
    pub fn pitch(self) -> u8 {
        self.note.pitch()
    }
    pub fn channel(self) -> u8 {
        self.note.channel()
    }
    pub fn velocity(self) -> u8 {
        self.note.velocity()
    }
}

impl<'a> SnapshotMidiSourceRead<'a> {
    pub fn source_length(self) -> BeatTime {
        self.source.source_length()
    }
    pub fn source_start(self) -> BeatTime {
        self.source.source_start()
    }
    pub fn loop_start(self) -> BeatTime {
        self.source.loop_start()
    }
    pub fn loop_end(self) -> BeatTime {
        self.source.loop_end()
    }
    pub fn looping(self) -> bool {
        self.source.looping()
    }
    pub fn notes(self) -> impl ExactSizeIterator<Item = SnapshotMidiNoteRead<'a>> + 'a {
        self.source
            .notes()
            .iter()
            .map(|note| SnapshotMidiNoteRead { note })
    }
}

impl<'a> SnapshotSceneRead<'a> {
    pub fn id(self) -> SceneId {
        self.scene.id()
    }
    pub fn name(self) -> &'a str {
        self.scene.name()
    }
}

impl<'a> SnapshotSessionCellRead<'a> {
    pub fn track(self) -> TrackId {
        self.cell.track()
    }
    pub fn scene(self) -> SceneId {
        self.cell.scene()
    }
    pub fn content(self) -> SnapshotSessionCellContentRead<'a> {
        match self.cell.content() {
            hex_project::SessionCellContent::Stop => SnapshotSessionCellContentRead::Stop,
            hex_project::SessionCellContent::Midi(clip) => {
                SnapshotSessionCellContentRead::Midi(SnapshotSessionMidiClipRead { clip })
            }
            hex_project::SessionCellContent::Audio(clip) => {
                SnapshotSessionCellContentRead::Audio(SnapshotSessionAudioClipRead { clip })
            }
        }
    }
}

impl<'a> SnapshotSessionMidiClipRead<'a> {
    pub fn id(self) -> ClipId {
        self.clip.id()
    }
    pub fn name(self) -> &'a str {
        self.clip.name()
    }
    pub fn flavor(self) -> ClipFlavor {
        self.clip.flavor()
    }
    pub fn source_length(self) -> BeatTime {
        self.clip.source().source_length()
    }
    pub fn source_start(self) -> BeatTime {
        self.clip.source().source_start()
    }
    pub fn loop_start(self) -> BeatTime {
        self.clip.source().loop_start()
    }
    pub fn loop_end(self) -> BeatTime {
        self.clip.source().loop_end()
    }
    pub fn looping(self) -> bool {
        self.clip.source().looping()
    }
    pub fn notes(self) -> impl ExactSizeIterator<Item = SnapshotMidiNoteRead<'a>> + 'a {
        self.clip
            .source()
            .notes()
            .iter()
            .map(|note| SnapshotMidiNoteRead { note })
    }
}

impl<'a> SnapshotSessionAudioClipRead<'a> {
    pub fn id(self) -> ClipId {
        self.clip.id()
    }
    pub fn name(self) -> &'a str {
        self.clip.name()
    }
    pub fn flavor(self) -> ClipFlavor {
        self.clip.flavor()
    }
    pub fn asset(self) -> AssetId {
        self.clip.asset()
    }
    pub fn source_start(self) -> u64 {
        self.clip.source_start()
    }
    pub fn source_end(self) -> u64 {
        self.clip.source_end()
    }
    pub fn gain(self) -> hex_project::ClipGain {
        self.clip.gain()
    }
}

impl<'a> SnapshotTempoRead<'a> {
    pub fn sample_rate(self) -> u32 {
        self.map.sample_rate()
    }
    pub fn tempo_at(self, beat: BeatTime) -> Tempo {
        self.map.tempo_at(beat)
    }
    pub fn beat_to_frame(self, beat: BeatTime) -> Result<u64, hex_project::Invalid> {
        self.map.beat_to_frame(beat)
    }
    pub fn frame_to_beat(self, frame: u64) -> Result<BeatTime, hex_project::Invalid> {
        self.map.frame_to_beat(frame)
    }
    pub fn beat_lower_bound_for_frame(self, frame: u64) -> Result<u128, hex_project::Invalid> {
        self.map.beat_lower_bound_for_frame(frame)
    }
    pub fn points(self) -> impl ExactSizeIterator<Item = TempoPoint> + 'a {
        self.map.points().iter().copied()
    }
}

impl<'a> SnapshotMeterRead<'a> {
    pub fn meter_at(self, beat: BeatTime) -> Meter {
        self.map.meter_at(beat)
    }
    pub fn position(self, beat: BeatTime) -> Result<MeterPosition, hex_project::Invalid> {
        self.map.position(beat)
    }
    pub fn points(self) -> impl ExactSizeIterator<Item = MeterPoint> + 'a {
        self.map.points().iter().copied()
    }
    /// Exact bounded backing slice of the map's `Copy` meter points.
    pub fn points_slice(self) -> &'a [MeterPoint] {
        self.map.points()
    }
}

impl<'a> SnapshotAssetRead<'a> {
    pub fn path(self) -> &'a std::path::Path {
        self.asset.path()
    }
    pub fn source_sample_rate(self) -> u32 {
        self.asset.source_sample_rate()
    }
    pub fn source_frames(self) -> u64 {
        self.asset.source_frames()
    }
    pub fn playback_frames(self) -> u64 {
        self.asset.playback_frames()
    }
}

impl<'a> SnapshotAudioEditRead<'a> {
    pub fn pitch_cents(self) -> i16 {
        self.edit.domain.pitch_cents
    }
    pub fn musical_start(self) -> Option<BeatTime> {
        self.edit.musical_start
    }
    pub fn warp_mode(self) -> Option<hex_project::WarpMode> {
        self.edit.domain.warp.as_ref().map(|warp| warp.mode)
    }
    pub fn warp_markers(self) -> impl ExactSizeIterator<Item = hex_project::WarpMarker> + 'a {
        self.edit
            .domain
            .warp
            .as_ref()
            .map_or(&[][..], |warp| warp.markers.as_slice())
            .iter()
            .copied()
    }
    pub fn source_gain_points(
        self,
    ) -> impl ExactSizeIterator<Item = hex_project::SourceGainPoint> + 'a {
        self.edit.domain.source_gain.iter().copied()
    }
    pub fn view_start(self) -> u64 {
        self.edit.view_start
    }
    pub fn view_end(self) -> u64 {
        self.edit.view_end
    }
    pub fn source_offset(self) -> u64 {
        self.edit.domain.source_offset
    }
    pub fn source_length(self) -> u64 {
        self.edit.domain.source_length
    }
    pub fn source_phase(self) -> u64 {
        self.edit.domain.source_phase
    }
    pub fn looping(self) -> bool {
        self.edit.domain.looping
    }
    pub fn sample_bpm(self) -> Option<Tempo> {
        self.edit.domain.sample_bpm
    }
    pub fn warp_enabled(self) -> bool {
        self.edit.domain.warp_enabled
    }
    pub fn retained_fade(self) -> ClipFade {
        self.edit.domain.fade
    }
    pub fn clock(self) -> hex_project::AudioDomainClock {
        self.edit.domain.clock
    }
}

impl<'a> SnapshotGainSpanRead<'a> {
    pub fn start(self) -> BeatTime {
        self.span.start
    }
    pub fn end(self) -> BeatTime {
        self.span.end
    }
    pub fn steps(self) -> impl ExactSizeIterator<Item = GainTouchStep> + 'a {
        self.span.steps.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_preview_and_event_facades_use_the_exact_retained_clip_asset_and_tempo() {
        use crate::snapshot_exports::{SessionExportFamily, SnapshotExportReservation};
        use hex_project::{
            AudioClipPreviewOperation, AudioDomainClock, Command, Created, TrimEdge,
        };
        let mut session = crate::Session::new(48_000, 8, 1024).unwrap();
        let Created::Imported { clip, .. } = session
            .editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "/test/metadata-only.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("expected imported audio");
        };
        let capture = |session: &crate::Session, revision| {
            Some(crate::service::view_state_owned(
                session,
                hex_engine::SharedMidiArrangementPreview::new_on_control(
                    hex_engine::MidiArrangementPreview::empty_on_control(),
                ),
                revision,
                1,
                false,
                None,
            ))
        };
        let (family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 4,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let old = family
            .insert_event_read_on_control(&producer, &mut capture(&session, 7))
            .unwrap();
        session
            .editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 24_000,
            })
            .unwrap();
        session
            .editor
            .execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .unwrap();
        let Created::Clip(tail) = session
            .editor
            .execute(Command::SplitClip {
                clip,
                frame: 48_000,
            })
            .unwrap()
        else {
            panic!("expected split tail");
        };
        let current = family
            .insert_event_read_on_control(&producer, &mut capture(&session, 8))
            .unwrap();
        for (read, clock, view_end, pitch, count, next_id, preview_length, source_start) in [
            (&old, 96_000, 96_000, 0, 1, 2, 36_000, 0),
            (&current, 72_000, 24_000, 137, 2, 3, 12_000, 24_000),
        ] {
            family
                .read_with(read, |snapshot| {
                    let (facts, actual_count, actual_next, last_end) = snapshot
                        .with_audio_event_root(clip, |root| {
                            let events = root.editable_events().unwrap();
                            (
                                root.facts(),
                                events.len(),
                                events.next_id(),
                                events.iter().last().unwrap().end,
                            )
                        })
                        .unwrap()
                        .unwrap();
                    assert_eq!(facts.domain.clock, AudioDomainClock::Frames(clock));
                    assert_eq!(
                        (facts.view_start, facts.view_end, facts.domain.pitch_cents),
                        (0, view_end, pitch)
                    );
                    assert_eq!(
                        (actual_count, actual_next, last_end),
                        (count, next_id, clock)
                    );
                    let preview = snapshot
                        .with_audio_clip_preview(
                            clip,
                            AudioClipPreviewOperation::Trim(TrimEdge::Right),
                            36_000,
                            |read| read.facts(),
                        )
                        .unwrap()
                        .unwrap();
                    assert_eq!(preview.duration, preview_length);
                    let markers = snapshot.default_audio_warp_markers(clip).unwrap().unwrap();
                    assert_eq!(
                        (markers[0].source_frame, markers[1].source_frame),
                        (source_start, 96_000)
                    );
                    assert!(matches!(
                        snapshot.with_audio_event_root(clip, |root| root.root_frame_at(clock + 1)),
                        Ok(Some(Err(_)))
                    ));
                })
                .unwrap();
        }
        family
            .read_with(&old, |snapshot| {
                assert_eq!(snapshot.with_audio_event_root(tail, |_| ()).unwrap(), None);
                assert_eq!(snapshot.default_audio_warp_markers(tail).unwrap(), None);
                assert_eq!(
                    snapshot
                        .with_audio_clip_preview(
                            tail,
                            AudioClipPreviewOperation::Trim(TrimEdge::Right),
                            1,
                            |_| ()
                        )
                        .unwrap(),
                    None
                );
            })
            .unwrap();
    }

    #[test]
    fn prepared_midi_and_gain_facts_stay_with_the_retained_snapshot_after_tempo_edit() {
        use crate::snapshot_exports::{SessionExportFamily, SnapshotExportReservation};
        use hex_project::{Command, Created};

        let mut session = crate::Session::new(48_000, 8, 1024).unwrap();
        let Created::Track(track) = session
            .editor
            .execute(Command::AddMidiTrack {
                name: "keys".into(),
            })
            .unwrap()
        else {
            panic!("expected track");
        };
        let Created::Clip(clip) = session
            .editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(4).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("expected clip");
        };
        session
            .editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let capture = |session: &crate::Session, revision| {
            let (_, midi) = session.compile_with_midi_preview().unwrap();
            Some(crate::service::view_state_owned(
                session, midi, revision, 1, false, None,
            ))
        };
        let (family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 4,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let old = family
            .insert_event_read_on_control(&producer, &mut capture(&session, 7))
            .unwrap();
        session
            .editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        session
            .editor
            .execute(Command::Gain { track, gain: 0.5 })
            .unwrap();
        let current = family
            .insert_event_read_on_control(&producer, &mut capture(&session, 8))
            .unwrap();
        for (read, bounds, gate_end, gain) in [
            (&old, (96_000, 192_000), 120_000, 1.0),
            (&current, (192_000, 384_000), 240_000, 0.5),
        ] {
            family
                .read_with(read, |snapshot| {
                    assert_eq!(snapshot.midi_clip_frame_bounds(clip), Some(bounds));
                    let preview = snapshot.midi_clip_preview(clip).unwrap();
                    assert_eq!(preview.authored_note_count(), 1);
                    assert_eq!(preview.occurrence_count(), 1);
                    let sample = preview.samples().next().unwrap();
                    assert_eq!(
                        (sample.frame_start, sample.frame_gate_end, sample.pitch),
                        (bounds.0, gate_end, 60)
                    );
                    assert_eq!(snapshot.gain_preview_at(track, bounds.0), Some(gain));
                })
                .unwrap();
        }
    }

    #[test]
    fn tempo_and_meter_queries_keep_exact_project_conversions() {
        use hex_project::{Command, Editor};

        let mut editor = Editor::new(
            hex_project::Project::new(48_000).expect("valid test sample rate"),
            8,
        )
        .expect("valid history capacity");
        let change = BeatTime::from_quarters(2).expect("small beat is representable");
        editor
            .execute(Command::SetTempoPoint {
                beat: change,
                tempo: Tempo::new(60_000_000).expect("valid tempo"),
            })
            .expect("tempo marker can be added");
        editor
            .execute(Command::SetMeterPoint {
                beat: change,
                meter: Meter::new(3, 4).expect("valid meter"),
            })
            .expect("meter marker can be added");
        let read = SnapshotProjectRead {
            project: editor.project(),
        };
        let beat = BeatTime::from_quarters(4).expect("small beat is representable");
        let tempo = read.tempo();
        assert_eq!(tempo.tempo_at(BeatTime::ZERO).micro_bpm(), 120_000_000);
        assert_eq!(tempo.tempo_at(change).micro_bpm(), 60_000_000);
        assert_eq!(tempo.beat_to_frame(beat), Ok(144_000));
        assert_eq!(tempo.frame_to_beat(144_000), Ok(beat));
        let meter = read.meter();
        let position = meter.position(beat).expect("beat has a meter position");
        assert_eq!((position.bar(), position.beat()), (2, 3));
        assert_eq!(meter.meter_at(change).numerator(), 3);
        assert_eq!(meter.points().len(), 2);
        assert_eq!(meter.points_slice().len(), 2);
        assert_eq!(meter.points_slice()[1].beat(), change);
    }

    #[test]
    fn session_membership_and_midi_facts_are_borrowed_and_distinct() {
        use hex_project::{Command, Created, Editor};

        let mut editor = Editor::new(
            hex_project::Project::new(48_000).expect("valid test sample rate"),
            8,
        )
        .expect("valid history capacity");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .expect("midi track can be added")
        {
            Created::Track(track) => track,
            other => panic!("unexpected track result: {other:?}"),
        };
        let scene = match editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene can be added")
        {
            Created::Scene(scene) => scene,
            other => panic!("unexpected scene result: {other:?}"),
        };
        assert!(matches!(
            SnapshotProjectRead {
                project: editor.project(),
            }
            .session_cell(track, scene),
            SnapshotSessionCellContentRead::Empty
        ));
        editor
            .execute(Command::SetSessionStopCell { track, scene })
            .expect("stop cell can be set");
        assert!(matches!(
            SnapshotProjectRead {
                project: editor.project(),
            }
            .session_cell(track, scene),
            SnapshotSessionCellContentRead::Stop
        ));
        editor
            .execute(Command::ClearSessionCell { track, scene })
            .expect("stop cell can be cleared");
        assert!(matches!(
            SnapshotProjectRead {
                project: editor.project(),
            }
            .session_cell(track, scene),
            SnapshotSessionCellContentRead::Empty
        ));
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "notes".into(),
                source_length: BeatTime::from_quarters(4).expect("small beat is representable"),
            })
            .expect("session clip can be added")
        {
            Created::SessionClip(clip) => clip,
            other => panic!("unexpected session clip result: {other:?}"),
        };
        let note = match editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .expect("midi note can be added")
        {
            Created::Note(note) => note,
            other => panic!("unexpected midi note result: {other:?}"),
        };
        let arrangement_clip = match editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(8).expect("small beat is representable"),
                length: BeatTime::from_quarters(4).expect("small beat is representable"),
            })
            .expect("arrangement clip can be placed")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected arrangement clip result: {other:?}"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip: arrangement_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 48,
                channel: 1,
                velocity: 90,
            })
            .expect("arrangement midi note can be added");

        let read = SnapshotProjectRead {
            project: editor.project(),
        };
        match read.session_cell(track, scene) {
            SnapshotSessionCellContentRead::Midi(session_clip) => {
                assert_eq!(session_clip.id(), clip);
                assert_eq!(session_clip.name(), "notes");
                let notes = session_clip.notes().collect::<Vec<_>>();
                assert_eq!(notes.len(), 1);
                assert_eq!(
                    (notes[0].id(), notes[0].pitch(), notes[0].velocity()),
                    (note, 60, 100)
                );
            }
            _ => panic!("expected a session midi clip"),
        }
        assert!(read.session_midi_clip(clip).is_some());
        assert!(read.midi_clip(clip).is_none());
        assert_eq!(read.midi_note(note).map(|note| note.pitch()), Some(60));
        let session_source = read.midi_source(clip).expect("session source resolves");
        assert_eq!(
            session_source.source_length(),
            BeatTime::from_quarters(4).unwrap()
        );
        assert_eq!(
            session_source.notes().next().map(|note| note.id()),
            Some(note)
        );
        let arrangement_source = read
            .midi_source(arrangement_clip)
            .expect("arrangement source resolves");
        assert_eq!(
            arrangement_source.source_length(),
            BeatTime::from_quarters(4).unwrap()
        );
        let arrangement_note = arrangement_source.notes().next().expect("note exists");
        assert_eq!(
            (arrangement_note.pitch(), arrangement_note.velocity()),
            (48, 90)
        );
        assert_eq!(read.scenes().next().map(|scene| scene.id()), Some(scene));
        assert_eq!(read.session_cells().count(), 1);
    }

    #[test]
    fn retained_fade_and_audio_edit_facts_keep_the_authored_domain() {
        use hex_project::{Command, Created, Editor};

        let mut editor = Editor::new(
            hex_project::Project::new(48_000).expect("valid test sample rate"),
            8,
        )
        .expect("valid history capacity");
        let clip = match editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "/test/audio.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("audio metadata can be imported")
        {
            Created::Imported { clip, .. } => clip,
            other => panic!("unexpected import result: {other:?}"),
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 4_800,
                fade_out: 9_600,
            })
            .expect("fade can be set");
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 137 })
            .expect("pitch can be set");
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .expect("audio can use its musical domain");
        let tail = match editor
            .execute(Command::SplitClip {
                clip,
                frame: 31_337,
            })
            .expect("audio clip can be split")
        {
            Created::Clip(tail) => tail,
            other => panic!("unexpected split result: {other:?}"),
        };

        let project_read = SnapshotProjectRead {
            project: editor.project(),
        };
        let track = project_read
            .tracks()
            .next()
            .expect("imported audio track exists");
        let clip_read = track.clip(tail).expect("split tail exists");
        assert_eq!(clip_read.fade().fade_in(), 0);
        assert_eq!(clip_read.retained_fade().fade_in(), 4_800);
        let edit = clip_read.audio_edit().expect("musical edit is present");
        assert!(edit.view_end() > edit.view_start());
        assert_eq!(edit.retained_fade().fade_in(), 4_800);
        assert_eq!(edit.sample_bpm(), None);
        assert_eq!(edit.source_phase(), clip_read.source_phase());
    }
}
