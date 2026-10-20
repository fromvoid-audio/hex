//! Finite borrowed facts used by UI consumers of a retained project snapshot.
//!
//! These wrappers keep every model borrow tied to the enclosing snapshot read.
//! They expose copyable values and iterators over purpose-specific wrappers;
//! no project, source, edit root, asset owner, or mutable runtime state escapes.
use super::{SnapshotClipRead, SnapshotProjectRead, SnapshotTrackRead};
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

impl<'a> SnapshotProjectRead<'a> {
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

impl<'a> SnapshotTrackRead<'a> {
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

impl SnapshotAudioEditRead<'_> {
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
