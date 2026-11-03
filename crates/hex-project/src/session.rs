//! Sparse, independently-owned Session cells. Clips own source data and local
//! processing metadata, without Arrangement placement or a back-reference.
use crate::{
    BeatTime, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MAX_SCENES, MAX_SESSION_CELLS,
    MidiSource, NoteId, SceneId, TrackId,
};
use std::sync::Arc;

/// Stable scene intent captured before asynchronous Console work. Insert(None)
/// explicitly appends one row; an existing row is never inferred from selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSceneTarget {
    Existing(SceneId),
    Insert { before: Option<SceneId> },
}
impl SessionSceneTarget {
    pub fn validate(self, project: &crate::Project) -> Result<(), Invalid> {
        let scene = match self {
            Self::Existing(scene) => Some(scene),
            Self::Insert { before } => {
                if project.scenes().len() >= MAX_SCENES {
                    return Err(Invalid("scene limit reached"));
                }
                before
            }
        };
        if scene.is_some_and(|id| !project.scenes().iter().any(|scene| scene.id() == id)) {
            return Err(Invalid("console scene no longer exists"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scene {
    pub(crate) id: SceneId,
    pub(crate) name: String,
}
impl Scene {
    pub fn id(&self) -> SceneId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMidiClip {
    pub(crate) active: bool,
    pub(crate) playback_offset: BeatTime,
    pub(crate) flavor: crate::ClipFlavor,
    pub(crate) id: ClipId,
    pub(crate) name: String,
    pub(crate) source: Arc<MidiSource>,
    pub(crate) volume: crate::MidiClipVolume,
}

/// Positive half-open cycle in asset playback frames, independent of launch time.
/// A launch plays source_start..end, then repeats start..end. The retained
/// source_end may include a release tail outside the cycle; stopping a cell
/// follows the existing route-release contract rather than playing that tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionAudioLoop {
    start: u64,
    end: u64,
}
impl SessionAudioLoop {
    pub fn new(start: u64, end: u64) -> Result<Self, Invalid> {
        if start >= end {
            return Err(Invalid("console audio loop must have positive length"));
        }
        Ok(Self { start, end })
    }
    pub fn start(self) -> u64 {
        self.start
    }
    pub fn end(self) -> u64 {
        self.end
    }
    pub fn validate_source(self, source_start: u64, source_end: u64) -> Result<(), Invalid> {
        if source_start >= self.end || self.end > source_end {
            return Err(Invalid("console audio loop is outside its source window"));
        }
        Ok(())
    }
    /// Bounded constant-work lookup, including arbitrarily late launches.
    /// Returns None for a mismatched source start; never adds elapsed to an
    /// absolute source position before reducing it into the positive cycle.
    pub fn frame_at(self, source_start: u64, elapsed: u64) -> Option<u64> {
        let first = self.end.checked_sub(source_start).filter(|n| *n > 0)?;
        if elapsed < first {
            source_start.checked_add(elapsed)
        } else {
            Some(self.start + (elapsed - first) % (self.end - self.start))
        }
    }
}

/// Source frame audible `elapsed` playback frames after a launch: plays
/// `source_start..` and then repeats the loop cycle, or ends at `source_end`
/// without one. Constant work. The Session renderer and the clip-editor
/// playback line share this law so the cursor cannot drift from the audio.
pub fn session_audio_source_frame(
    source_start: u64,
    source_end: u64,
    loop_range: Option<SessionAudioLoop>,
    elapsed: u64,
) -> Option<u64> {
    match loop_range {
        Some(range) => range.frame_at(source_start, elapsed),
        None => source_start
            .checked_add(elapsed)
            .filter(|frame| *frame < source_end),
    }
}

/// Frame-anchored audio source assigned to one Session cell. A missing loop
/// preserves one-shot playback; an explicit loop retains source/intro/tail PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAudioClip {
    pub(crate) active: bool,
    pub(crate) flavor: crate::ClipFlavor,
    pub(crate) id: ClipId,
    pub(crate) name: String,
    pub(crate) asset: crate::AssetId,
    pub(crate) source_start: u64,
    pub(crate) source_end: u64,
    pub(crate) loop_range: Option<SessionAudioLoop>,
    pub(crate) gain: crate::ClipGain,
}
impl SessionAudioClip {
    /// Authored clip activation, independent of track mute or selection.
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn flavor(&self) -> crate::ClipFlavor {
        self.flavor
    }
    pub fn id(&self) -> ClipId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn asset(&self) -> crate::AssetId {
        self.asset
    }
    pub fn source_start(&self) -> u64 {
        self.source_start
    }
    pub fn gain(&self) -> crate::ClipGain {
        self.gain
    }
    pub fn loop_range(&self) -> Option<SessionAudioLoop> {
        self.loop_range
    }
    pub fn source_end(&self) -> u64 {
        self.source_end
    }
    /// Asset playback frame heard `elapsed` frames after this cell launched.
    pub fn source_frame_at(&self, elapsed: u64) -> Option<u64> {
        session_audio_source_frame(self.source_start, self.source_end, self.loop_range, elapsed)
    }
}
impl SessionMidiClip {
    /// Authored clip activation, independent of track mute or selection.
    pub fn active(&self) -> bool {
        self.active
    }
    /// Elapsed source-playback window origin in quarter-note beats.
    pub fn playback_offset(&self) -> BeatTime {
        self.playback_offset
    }
    pub fn volume(&self) -> &crate::MidiClipVolume {
        &self.volume
    }
    pub fn flavor(&self) -> crate::ClipFlavor {
        self.flavor
    }
    pub fn id(&self) -> ClipId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> &MidiSource {
        &self.source
    }
    pub fn source_identity(&self) -> crate::MidiSourceIdentity {
        crate::MidiSourceIdentity::of(&self.source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCellContent {
    Midi(SessionMidiClip),
    Audio(SessionAudioClip),
    /// An explicit stop cell. Missing cells are structurally empty.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCell {
    pub(crate) track: TrackId,
    pub(crate) scene: SceneId,
    pub(crate) content: SessionCellContent,
}
impl SessionCell {
    pub fn track(&self) -> TrackId {
        self.track
    }
    pub fn scene(&self) -> SceneId {
        self.scene
    }
    pub fn content(&self) -> &SessionCellContent {
        &self.content
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionGrid {
    pub(crate) scenes: Vec<Scene>,
    pub(crate) cells: Vec<SessionCell>,
}

pub(crate) fn apply(
    project: &mut crate::Project,
    command: crate::Command,
) -> Result<Created, Invalid> {
    use crate::Command;
    match command {
        Command::AddScene { name } => apply(project, Command::InsertScene { name, before: None }),
        Command::InsertScene { name, before } => {
            crate::edit::name_valid(&name)?;
            let index = match before {
                Some(anchor) => project
                    .session
                    .scenes
                    .iter()
                    .position(|scene| scene.id == anchor)
                    .ok_or(Invalid("scene insertion anchor no longer exists"))?,
                None => project.session.scenes.len(),
            };
            if project.session.scenes.len() >= MAX_SCENES {
                return Err(Invalid("scene limit reached"));
            }
            project
                .session
                .scenes
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve scene storage"))?;
            let id = SceneId(project.allocate_id()?);
            project.session.scenes.insert(index, Scene { id, name });
            Ok(Created::Scene(id))
        }
        Command::DuplicateScene(scene) => duplicate_scene(project, scene),
        Command::MoveScene { scene, before } => move_scene(project, scene, before),
        Command::RenameScene { scene, name } => {
            crate::edit::name_valid(&name)?;
            let target = project
                .session
                .scenes
                .iter_mut()
                .find(|item| item.id == scene)
                .ok_or(Invalid("scene no longer exists"))?;
            target.name = name;
            Ok(Created::None)
        }
        Command::RemoveScene(scene) => {
            let index = project
                .session
                .scenes
                .iter()
                .position(|item| item.id == scene)
                .ok_or(Invalid("scene no longer exists"))?;
            project.session.scenes.remove(index);
            project.session.cells.retain(|cell| cell.scene != scene);
            Ok(Created::None)
        }
        Command::CreateSessionMidiClip {
            track,
            scene,
            name,
            source_length,
        } => {
            crate::edit::name_valid(&name)?;
            require_known_cell_key(project, track, scene)?;
            ensure_empty(project, track, scene)?;
            crate::midi::validate_source(
                source_length,
                BeatTime::ZERO,
                BeatTime::ZERO,
                source_length,
                false,
            )?;
            ensure_clip_room(project, 1)?;
            reserve_cell(project)?;
            let id = ClipId(project.allocate_id()?);
            project.session.cells.push(SessionCell {
                track,
                scene,
                content: SessionCellContent::Midi(SessionMidiClip {
                    active: true,
                    playback_offset: crate::BeatTime::ZERO,
                    flavor: crate::ClipFlavor::default(),
                    id,
                    name,
                    volume: crate::MidiClipVolume::default(),
                    source: Arc::new(MidiSource::defaults(source_length)),
                }),
            });
            Ok(Created::SessionClip(id))
        }
        Command::CreateSessionAudioClip {
            track,
            scene,
            name,
            asset,
            source_start,
            source_end,
        } => {
            crate::edit::name_valid(&name)?;
            require_known_cell_key(project, track, scene)?;
            ensure_empty(project, track, scene)?;
            let media = project
                .asset(asset)
                .ok_or(Invalid("session audio asset no longer exists"))?;
            validate_audio_source(source_start, source_end, media.playback_frames())?;
            ensure_clip_room(project, 1)?;
            reserve_cell(project)?;
            let id = ClipId(project.allocate_id()?);
            project.session.cells.push(SessionCell {
                track,
                scene,
                content: SessionCellContent::Audio(SessionAudioClip {
                    active: true,
                    flavor: crate::ClipFlavor::default(),
                    id,
                    name,
                    asset,
                    source_start,
                    source_end,
                    loop_range: None,
                    gain: crate::ClipGain::UNITY,
                }),
            });
            Ok(Created::SessionClip(id))
        }
        Command::CopyArrangementAudioClipToSession { source, scene } => {
            copy_arrangement_audio_to_session(project, source, scene, None)
        }
        Command::CopyArrangementMidiToSession { source, scene } => {
            copy_arrangement_midi_to_session(project, source, scene, None)
        }
        Command::CopyArrangementAudioClipToNewSessionTrack {
            source,
            before_track,
            scene,
        } => copy_to_new_session_track(project, source, before_track, scene, false),
        Command::CopyArrangementMidiToNewSessionTrack {
            source,
            before_track,
            scene,
        } => copy_to_new_session_track(project, source, before_track, scene, true),
        Command::MoveSessionCell {
            from_track,
            from_scene,
            to_track,
            to_scene,
        } => {
            require_known_cell_key(project, from_track, from_scene)?;
            require_known_cell_key(project, to_track, to_scene)?;
            let index = project
                .session
                .cells
                .iter()
                .position(|cell| cell.track == from_track && cell.scene == from_scene)
                .ok_or(Invalid("session source cell is empty"))?;
            match &project.session.cells[index].content {
                SessionCellContent::Midi(_) => require_known_cell_key(project, to_track, to_scene)?,
                SessionCellContent::Audio(_) => {
                    require_known_cell_key(project, to_track, to_scene)?
                }
                SessionCellContent::Stop => require_known_cell_key(project, to_track, to_scene)?,
            }
            if from_track == to_track && from_scene == to_scene {
                return Ok(Created::None);
            }
            ensure_empty(project, to_track, to_scene)?;
            project.session.cells[index].track = to_track;
            project.session.cells[index].scene = to_scene;
            Ok(Created::None)
        }
        Command::DuplicateSessionMidiCell {
            track,
            from_scene,
            to_scene,
        } => {
            require_known_cell_key(project, track, from_scene)?;
            require_known_cell_key(project, track, to_scene)?;
            ensure_empty(project, track, to_scene)?;
            let source_note_count = match project
                .session
                .cells
                .iter()
                .find(|cell| cell.track == track && cell.scene == from_scene)
                .map(|cell| &cell.content)
            {
                Some(SessionCellContent::Midi(clip)) => clip.source.notes.len(),
                Some(SessionCellContent::Audio(_)) => {
                    return Err(Invalid("session cell contains audio"));
                }
                Some(SessionCellContent::Stop) => {
                    return Err(Invalid("cannot duplicate a session stop cell"));
                }
                None => return Err(Invalid("session cell is empty")),
            };
            ensure_clip_room(project, 1)?;
            ensure_note_room(project, source_note_count)?;
            reserve_cell(project)?;
            let mut duplicate = match project
                .session
                .cells
                .iter()
                .find(|cell| cell.track == track && cell.scene == from_scene)
                .map(|cell| &cell.content)
            {
                Some(SessionCellContent::Midi(clip)) => clip.clone(),
                _ => return Err(Invalid("session midi clip no longer exists")),
            };
            let id = ClipId(project.allocate_id()?);
            duplicate.id = id;
            for note in &mut Arc::make_mut(&mut duplicate.source).notes {
                note.id = NoteId(project.allocate_id()?);
            }
            project.session.cells.push(SessionCell {
                track,
                scene: to_scene,
                content: SessionCellContent::Midi(duplicate),
            });
            Ok(Created::SessionClip(id))
        }
        Command::DuplicateSessionAudioCell {
            track,
            from_scene,
            to_scene,
        } => {
            require_known_cell_key(project, track, from_scene)?;
            require_known_cell_key(project, track, to_scene)?;
            ensure_empty(project, track, to_scene)?;
            let mut duplicate = match project
                .session
                .cells
                .iter()
                .find(|cell| cell.track == track && cell.scene == from_scene)
                .map(|cell| &cell.content)
            {
                Some(SessionCellContent::Audio(clip)) => clip.clone(),
                Some(SessionCellContent::Midi(_)) => {
                    return Err(Invalid("session cell contains midi"));
                }
                Some(SessionCellContent::Stop) => {
                    return Err(Invalid("cannot duplicate a session stop cell"));
                }
                None => return Err(Invalid("session cell is empty")),
            };
            ensure_clip_room(project, 1)?;
            reserve_cell(project)?;
            duplicate.id = ClipId(project.allocate_id()?);
            let id = duplicate.id;
            project.session.cells.push(SessionCell {
                track,
                scene: to_scene,
                content: SessionCellContent::Audio(duplicate),
            });
            Ok(Created::SessionClip(id))
        }
        Command::SetSessionAudioClipLoop { clip, loop_range } => {
            let item = project
                .session
                .cells
                .iter_mut()
                .find_map(|cell| match &mut cell.content {
                    SessionCellContent::Audio(item) if item.id == clip => Some(item),
                    _ => None,
                })
                .ok_or(Invalid("session audio clip no longer exists"))?;
            if let Some(range) = loop_range {
                range.validate_source(item.source_start, item.source_end)?;
            }
            item.loop_range = loop_range;
            Ok(Created::None)
        }
        Command::SetSessionAudioClipSource {
            clip,
            asset,
            source_start,
            source_end,
        } => {
            if project.session_audio_clip(clip).is_none() {
                return Err(Invalid("session audio clip no longer exists"));
            }
            let media = project
                .asset(asset)
                .ok_or(Invalid("session audio asset no longer exists"))?;
            validate_audio_source(source_start, source_end, media.playback_frames())?;
            if let Some(range) = project.session_audio_clip(clip).and_then(|c| c.loop_range) {
                range.validate_source(source_start, source_end)?;
            }
            let target_cell = project.session.cells.iter_mut().find(|cell| matches!(&cell.content, SessionCellContent::Audio(item) if item.id == clip)).ok_or(Invalid("session audio clip no longer exists"))?;
            if let SessionCellContent::Audio(item) = &mut target_cell.content {
                item.asset = asset;
                item.source_start = source_start;
                item.source_end = source_end;
            } else {
                return Err(Invalid("session audio clip no longer exists"));
            }
            Ok(Created::None)
        }
        Command::ClearSessionCell { track, scene } => {
            require_known_cell_key(project, track, scene)?;
            project
                .session
                .cells
                .retain(|cell| !(cell.track == track && cell.scene == scene));
            Ok(Created::None)
        }
        Command::SetSessionStopCell { track, scene } => {
            require_known_cell_key(project, track, scene)?;
            if let Some(cell) = project
                .session
                .cells
                .iter_mut()
                .find(|cell| cell.track == track && cell.scene == scene)
            {
                cell.content = SessionCellContent::Stop;
            } else {
                reserve_cell(project)?;
                project.session.cells.push(SessionCell {
                    track,
                    scene,
                    content: SessionCellContent::Stop,
                });
            }
            Ok(Created::None)
        }
        Command::SetSessionMidiClipSource {
            clip,
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping,
        } => {
            crate::midi::validate_source(
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
            )?;
            let target = project
                .session_midi_clip(clip)
                .ok_or(Invalid("session midi clip no longer exists"))?;
            crate::midi::validate_source_boundary(
                source_length,
                source_start,
                loop_start,
                loop_end,
                looping,
                target.source.one_shot_boundary(),
            )?;
            target.volume.validate_source_length(source_length)?;
            for note in target.source.notes() {
                crate::midi::validate_note(
                    note.start,
                    note.duration,
                    source_length,
                    note.pitch,
                    note.channel,
                    note.velocity,
                )?;
            }
            let target = project
                .session
                .cells
                .iter_mut()
                .find_map(|cell| match &mut cell.content {
                    SessionCellContent::Midi(item) if item.id == clip => Some(item),
                    _ => None,
                })
                .ok_or(Invalid("session midi clip no longer exists"))?;
            let source = Arc::make_mut(&mut target.source);
            if source.source_length != source_length {
                source.source_length = source_length;
            }
            source.source_start = source_start;
            source.loop_start = loop_start;
            source.loop_end = loop_end;
            source.looping = looping;
            Ok(Created::None)
        }
        Command::RenameSessionMidiClip { clip, name } => {
            crate::edit::name_valid(&name)?;
            let target = project
                .session
                .cells
                .iter_mut()
                .find_map(|cell| match &mut cell.content {
                    SessionCellContent::Midi(item) if item.id == clip => Some(item),
                    _ => None,
                })
                .ok_or(Invalid("session midi clip no longer exists"))?;
            target.name = name;
            Ok(Created::None)
        }
        _ => Err(Invalid("not a session command")),
    }
}

fn duplicate_scene(project: &mut crate::Project, scene: SceneId) -> Result<Created, Invalid> {
    let source_index = project
        .session
        .scenes
        .iter()
        .position(|item| item.id == scene)
        .ok_or(Invalid("scene no longer exists"))?;
    if project.session.scenes.len() >= MAX_SCENES {
        return Err(Invalid("scene limit reached"));
    }

    let mut duplicate_name = String::new();
    duplicate_name
        .try_reserve_exact(project.session.scenes[source_index].name.len())
        .map_err(|_| Invalid("could not reserve scene name storage"))?;
    duplicate_name.push_str(&project.session.scenes[source_index].name);

    let mut source_cell_count = 0_usize;
    let mut midi_clip_count = 0_usize;
    let mut source_note_count = 0_usize;
    for cell in project
        .session
        .cells
        .iter()
        .filter(|cell| cell.scene == scene)
    {
        source_cell_count = source_cell_count
            .checked_add(1)
            .ok_or(Invalid("session cell count overflow"))?;
        match &cell.content {
            SessionCellContent::Midi(clip) => {
                midi_clip_count = midi_clip_count
                    .checked_add(1)
                    .ok_or(Invalid("session clip count overflow"))?;
                source_note_count = source_note_count
                    .checked_add(clip.source.notes.len())
                    .ok_or(Invalid("session note count overflow"))?;
            }
            SessionCellContent::Audio(_) => {
                midi_clip_count = midi_clip_count
                    .checked_add(1)
                    .ok_or(Invalid("session clip count overflow"))?
            }
            SessionCellContent::Stop => {}
        }
    }
    if project
        .session
        .cells
        .len()
        .checked_add(source_cell_count)
        .filter(|count| *count <= MAX_SESSION_CELLS)
        .is_none()
    {
        return Err(Invalid("session cell limit reached"));
    }
    ensure_clip_room(project, midi_clip_count)?;
    ensure_note_room(project, source_note_count)?;
    let id_count = 1_usize
        .checked_add(midi_clip_count)
        .and_then(|count| count.checked_add(source_note_count))
        .ok_or(Invalid("project id space exhausted"))?;
    let id_count_u64 =
        u64::try_from(id_count).map_err(|_| Invalid("project id space exhausted"))?;
    project
        .next_id
        .checked_add(id_count_u64)
        .ok_or(Invalid("project id space exhausted"))?;

    project
        .session
        .scenes
        .try_reserve(1)
        .map_err(|_| Invalid("could not reserve scene storage"))?;
    project
        .session
        .cells
        .try_reserve(source_cell_count)
        .map_err(|_| Invalid("could not reserve session cell storage"))?;
    let mut duplicated_cells = Vec::new();
    duplicated_cells
        .try_reserve_exact(source_cell_count)
        .map_err(|_| Invalid("could not reserve duplicated session cells"))?;
    for cell in project
        .session
        .cells
        .iter()
        .filter(|cell| cell.scene == scene)
    {
        let content = match &cell.content {
            SessionCellContent::Stop => SessionCellContent::Stop,
            SessionCellContent::Midi(clip) => {
                let mut name = String::new();
                name.try_reserve_exact(clip.name.len())
                    .map_err(|_| Invalid("could not reserve session clip name storage"))?;
                name.push_str(&clip.name);

                let source = clip.source.as_ref();
                let mut notes = Vec::new();
                notes
                    .try_reserve_exact(source.notes.len())
                    .map_err(|_| Invalid("could not reserve duplicated midi notes"))?;
                notes.extend(source.notes.iter().cloned());
                SessionCellContent::Midi(SessionMidiClip {
                    active: clip.active,
                    playback_offset: clip.playback_offset,
                    flavor: clip.flavor,
                    id: ClipId(0),
                    name,
                    volume: clip.volume.clone(),
                    source: Arc::new(MidiSource {
                        content: std::sync::Arc::new(crate::MidiContent {
                            source_length: source.source_length,
                            notes,
                        }),
                        one_shot_boundary: source.one_shot_boundary,
                        source_start: source.source_start,
                        loop_start: source.loop_start,
                        loop_end: source.loop_end,
                        looping: source.looping,
                    }),
                })
            }
            SessionCellContent::Audio(clip) => {
                let mut name = String::new();
                name.try_reserve_exact(clip.name.len())
                    .map_err(|_| Invalid("could not reserve session clip name storage"))?;
                name.push_str(&clip.name);
                SessionCellContent::Audio(SessionAudioClip {
                    active: clip.active,
                    flavor: clip.flavor,
                    id: ClipId(0),
                    name,
                    asset: clip.asset,
                    source_start: clip.source_start,
                    source_end: clip.source_end,
                    loop_range: clip.loop_range,
                    gain: clip.gain,
                })
            }
        };
        duplicated_cells.push(SessionCell {
            track: cell.track,
            scene: SceneId(0),
            content,
        });
    }

    let (first_id, _) = project.allocate_id_range(id_count)?;
    let mut next_id = first_id;
    let duplicate_id = SceneId(next_id);
    next_id = next_id
        .checked_add(1)
        .ok_or(Invalid("project id space exhausted"))?;
    for cell in &mut duplicated_cells {
        cell.scene = duplicate_id;
        if let SessionCellContent::Midi(clip) = &mut cell.content {
            clip.id = ClipId(next_id);
            next_id = next_id
                .checked_add(1)
                .ok_or(Invalid("project id space exhausted"))?;
            let source = Arc::get_mut(&mut clip.source)
                .ok_or(Invalid("duplicated midi source is shared"))?;
            for note in &mut source.notes {
                note.id = NoteId(next_id);
                next_id = next_id
                    .checked_add(1)
                    .ok_or(Invalid("project id space exhausted"))?;
            }
        } else if let SessionCellContent::Audio(clip) = &mut cell.content {
            clip.id = ClipId(next_id);
            next_id = next_id
                .checked_add(1)
                .ok_or(Invalid("project id space exhausted"))?;
        }
    }
    let duplicate_index = source_index + 1;
    project.session.scenes.insert(
        duplicate_index,
        Scene {
            id: duplicate_id,
            name: duplicate_name,
        },
    );
    project.session.cells.extend(duplicated_cells);
    Ok(Created::Scene(duplicate_id))
}

fn move_scene(
    project: &mut crate::Project,
    scene: SceneId,
    before: Option<SceneId>,
) -> Result<Created, Invalid> {
    let source_index = project
        .session
        .scenes
        .iter()
        .position(|item| item.id == scene)
        .ok_or(Invalid("scene no longer exists"))?;
    let target_index = match before {
        Some(target) if target == scene => return Ok(Created::None),
        Some(target) => Some(
            project
                .session
                .scenes
                .iter()
                .position(|item| item.id == target)
                .ok_or(Invalid("scene insertion target no longer exists"))?,
        ),
        None => None,
    };
    let moved = project.session.scenes.remove(source_index);
    let insertion_index = match target_index {
        Some(index) if source_index < index => index - 1,
        Some(index) => index,
        None => project.session.scenes.len(),
    };
    project.session.scenes.insert(insertion_index, moved);
    Ok(Created::None)
}

fn require_known_cell_key(
    project: &crate::Project,
    track: TrackId,
    scene: SceneId,
) -> Result<(), Invalid> {
    project
        .track(track)
        .ok_or(Invalid("track no longer exists"))?;
    if !project.session.scenes.iter().any(|item| item.id == scene) {
        return Err(Invalid("scene no longer exists"));
    }
    Ok(())
}
fn validate_audio_source(start: u64, end: u64, frames: u64) -> Result<(), Invalid> {
    if start >= end || end > frames {
        Err(Invalid("session audio source interval is outside media"))
    } else {
        Ok(())
    }
}
fn ensure_empty(project: &crate::Project, track: TrackId, scene: SceneId) -> Result<(), Invalid> {
    if project
        .session
        .cells
        .iter()
        .any(|cell| cell.track == track && cell.scene == scene)
    {
        Err(Invalid("session target cell is occupied"))
    } else {
        Ok(())
    }
}
fn reserve_cell(project: &mut crate::Project) -> Result<(), Invalid> {
    if project.session.cells.len() >= MAX_SESSION_CELLS {
        return Err(Invalid("session cell limit reached"));
    }
    project
        .session
        .cells
        .try_reserve(1)
        .map_err(|_| Invalid("could not reserve session cell storage"))
}
fn ensure_clip_room(project: &crate::Project, additional: usize) -> Result<(), Invalid> {
    let current = project
        .tracks
        .iter()
        .map(|track| track.clips().len() + track.midi_clips().len())
        .sum::<usize>()
        + project
            .session
            .cells
            .iter()
            .filter(|cell| {
                matches!(
                    cell.content,
                    SessionCellContent::Midi(_) | SessionCellContent::Audio(_)
                )
            })
            .count();
    if current
        .checked_add(additional)
        .filter(|count| *count <= MAX_CLIPS)
        .is_none()
    {
        Err(Invalid("clip limit reached"))
    } else {
        Ok(())
    }
}
fn ensure_note_room(project: &crate::Project, additional: usize) -> Result<(), Invalid> {
    let current = crate::midi::note_count(project);
    if current
        .checked_add(additional)
        .filter(|count| *count <= MAX_NOTES)
        .is_none()
    {
        Err(Invalid("note limit reached"))
    } else {
        Ok(())
    }
}

// Keep the copy law shared: source processing refusal, fresh identities and all
// source-owned metadata are independent of which normal track receives it.
fn copy_arrangement_audio_to_session(
    project: &mut crate::Project,
    source: ClipId,
    scene: SceneId,
    destination: Option<TrackId>,
) -> Result<Created, Invalid> {
    let (track, original) = project
        .tracks
        .iter()
        .find_map(|track| {
            track
                .clips()
                .iter()
                .find(|clip| clip.id() == source)
                .map(|clip| (track.id(), clip.clone()))
        })
        .ok_or(Invalid("arrangement audio clip no longer exists"))?;
    if original.audio_edit().is_some() {
        return Err(Invalid(
            "transformed or musically anchored arrangement audio cannot yet transfer to an unwarped session one-shot",
        ));
    }
    let track = destination.unwrap_or(track);
    require_known_cell_key(project, track, scene)?;
    ensure_empty(project, track, scene)?;
    ensure_clip_room(project, 1)?;
    reserve_cell(project)?;
    let id = ClipId(project.allocate_id()?);
    let name = original.name().to_owned();
    crate::edit::name_valid(&name)?;
    project.session.cells.push(SessionCell {
        track,
        scene,
        content: SessionCellContent::Audio(SessionAudioClip {
            active: original.active,
            flavor: original.flavor,
            id,
            name,
            loop_range: None,
            gain: original.gain(),
            asset: original.asset(),
            source_start: original.source_offset(),
            source_end: original
                .source_offset()
                .checked_add(original.source_length())
                .ok_or(Invalid("source end overflow"))?,
        }),
    });
    Ok(Created::SessionClip(id))
}
fn copy_arrangement_midi_to_session(
    project: &mut crate::Project,
    source: ClipId,
    scene: SceneId,
    destination: Option<TrackId>,
) -> Result<Created, Invalid> {
    require_forward_arrangement_midi(project, source)?;
    let (track, source_note_count) = project
        .tracks
        .iter()
        .find_map(|track| {
            track
                .midi_clips()
                .iter()
                .find(|clip| clip.id == source)
                .map(|clip| (track.id, clip.notes().len()))
        })
        .ok_or(Invalid("arrangement midi clip no longer exists"))?;
    let track = destination.unwrap_or(track);
    require_known_cell_key(project, track, scene)?;
    ensure_empty(project, track, scene)?;
    ensure_clip_room(project, 1)?;
    ensure_note_room(project, source_note_count)?;
    reserve_cell(project)?;
    let original = project
        .midi_clip(source)
        .ok_or(Invalid("arrangement midi clip no longer exists"))?
        .clone();
    let id = ClipId(project.allocate_id()?);
    let mut source = original.source.as_ref().clone();
    for note in &mut source.notes {
        note.id = NoteId(project.allocate_id()?);
    }
    project.session.cells.push(SessionCell {
        track,
        scene,
        content: SessionCellContent::Midi(SessionMidiClip {
            active: original.active,
            playback_offset: original.playback_offset,
            flavor: original.flavor,
            id,
            name: original.name.clone(),
            volume: original.volume.clone(),
            source: Arc::new(source),
        }),
    });
    Ok(Created::SessionClip(id))
}
fn require_forward_arrangement_midi(
    project: &crate::Project,
    source: ClipId,
) -> Result<(), Invalid> {
    if project
        .midi_clip(source)
        .is_some_and(|clip| clip.content_view().is_some())
    {
        return Err(Invalid(
            "directed arrangement midi cannot yet transfer to an ongoing session clip; use a forward clip",
        ));
    }
    Ok(())
}
fn copy_to_new_session_track(
    project: &mut crate::Project,
    source: ClipId,
    before_track: Option<TrackId>,
    scene: SessionSceneTarget,
    midi: bool,
) -> Result<Created, Invalid> {
    // Refuse before allocating a track, scene or fresh source identities.
    if midi {
        require_forward_arrangement_midi(project, source)?;
    }
    scene.validate(project)?;
    let name = if midi {
        project.midi_clip(source).map(|clip| clip.name().to_owned())
    } else {
        project.clip(source).map(|clip| clip.name().to_owned())
    }
    .ok_or(Invalid("arrangement source no longer exists"))?;
    let track = crate::edit::insert_track_before(project, name, before_track)?;
    let scene = match scene {
        SessionSceneTarget::Existing(scene) => scene,
        SessionSceneTarget::Insert { before } => {
            let name = format!("scene {}", project.scenes().len() + 1);
            let Created::Scene(scene) =
                apply(project, crate::Command::InsertScene { name, before })?
            else {
                return Err(Invalid("console insertion did not create a scene"));
            };
            scene
        }
    };
    if midi {
        copy_arrangement_midi_to_session(project, source, scene, Some(track))?;
    } else {
        copy_arrangement_audio_to_session(project, source, scene, Some(track))?;
    }
    Ok(Created::Track(track))
}

#[cfg(test)]
mod scene_command_tests {
    use super::*;
    use crate::{Command, Editor, Project};

    fn fixture(with_note: bool) -> (Project, TrackId, SceneId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "instrument".into(),
            })
            .expect("track")
        {
            Created::Track(id) => id,
            _ => panic!("track result"),
        };
        let scene = match editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        {
            Created::Scene(id) => id,
            _ => panic!("scene result"),
        };
        let clip = match editor
            .execute(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: BeatTime::from_quarters(4).expect("source length"),
            })
            .expect("session clip")
        {
            Created::SessionClip(id) => id,
            _ => panic!("session clip result"),
        };
        if with_note {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                })
                .expect("note");
        }
        (editor.project().clone(), track, scene)
    }

    fn assert_duplicate_refused_without_mutation(mut project: Project, scene: SceneId) {
        let before = project.clone();
        assert!(apply(&mut project, Command::DuplicateScene(scene)).is_err());
        assert_eq!(project, before);
    }

    #[test]
    fn duplicate_scene_preflights_all_model_limits_and_id_exhaustion() {
        let (base, _, scene) = fixture(false);

        let mut scenes_full = base.clone();
        let template = scenes_full.session.scenes[0].clone();
        scenes_full
            .session
            .scenes
            .resize_with(MAX_SCENES, || template.clone());
        assert_duplicate_refused_without_mutation(scenes_full, scene);

        let mut cells_full = base.clone();
        let template = cells_full.session.cells[0].clone();
        cells_full
            .session
            .cells
            .resize_with(MAX_SESSION_CELLS, || template.clone());
        assert_duplicate_refused_without_mutation(cells_full, scene);

        let mut clips_full = base.clone();
        let template = clips_full.session.cells[0].clone();
        clips_full
            .session
            .cells
            .resize_with(MAX_CLIPS, || template.clone());
        assert_duplicate_refused_without_mutation(clips_full, scene);

        let (mut notes_full, _, notes_scene) = fixture(true);
        if let SessionCellContent::Midi(clip) = &mut notes_full.session.cells[0].content {
            let template = clip.source.notes[0].clone();
            Arc::make_mut(&mut clip.source)
                .notes
                .resize(MAX_NOTES, template);
        }
        assert_duplicate_refused_without_mutation(notes_full, notes_scene);

        let mut ids_exhausted = base;
        ids_exhausted.next_id = u64::MAX - 1;
        assert_duplicate_refused_without_mutation(ids_exhausted, scene);
    }

    #[test]
    fn move_scene_preserves_cell_identity_and_does_not_allocate_ids() {
        let (mut project, _, scene) = fixture(true);
        let extra = Scene {
            id: SceneId(project.next_id),
            name: "second".into(),
        };
        project.next_id += 1;
        project.session.scenes.push(extra.clone());
        let last = Scene {
            id: SceneId(project.next_id),
            name: "third".into(),
        };
        project.next_id += 1;
        project.session.scenes.push(last.clone());
        let next_id = project.next_id;
        let cells = project.session.cells.clone();

        assert_eq!(
            apply(
                &mut project,
                Command::MoveScene {
                    scene: last.id,
                    before: Some(scene),
                },
            )
            .unwrap(),
            Created::None
        );
        assert_eq!(
            project
                .session
                .scenes
                .iter()
                .map(Scene::id)
                .collect::<Vec<_>>(),
            vec![last.id, scene, extra.id]
        );
        assert_eq!(project.session.cells, cells);
        assert_eq!(project.next_id, next_id);
    }
}
