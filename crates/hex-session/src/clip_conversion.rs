//! Finite source bounce preparation off UI/audio threads. Commit only after the
//! durable file, decoded media, waveform and replacement revision are prepared.
use super::*;
use hex_project::{ClipId, MidiClipSnapshot, SessionCellContent, TrackContentSnapshot};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BOUNCE_FRAMES: u64 = hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES;
static NEXT_BOUNCE: AtomicU64 = AtomicU64::new(0);

impl Session {
    /// Bounce only this clip's instrument, before retained track effects/mix
    /// and Main processing. Those live processors must apply exactly once.
    /// Arrangement length is preserved exactly; tails beyond the clip are cut.
    /// Session bounces one initial source pass, since audio cells are one-shots.
    pub fn bounce_midi_clip(
        &mut self,
        clip: ClipId,
        directory: &Path,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
        self.bounce_midi_clip_with_admission(clip, directory, previous, |_, _| Ok(()))
    }
    pub(crate) fn bounce_midi_clip_with_admission(
        &mut self,
        clip: ClipId,
        directory: &Path,
        previous: Option<&hex_engine::PreparedRevision>,
        storage: impl FnOnce(&Session, &PreparedImport) -> Result<(), SessionError>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
        self.bounce_midi_clip_using(
            clip,
            directory,
            |session, project| {
                hex_engine::PreparedRevision::prepare(
                    project,
                    &BTreeMap::new(),
                    session.sine_table(),
                    None,
                )
                .map(Arc::new)
                .map_err(|e| e.to_string())
            },
            |path, revision, start, frames, gain| {
                hex_export::render_bounce_new(path, revision, start, frames, gain)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            |session, prepared, track, start| {
                storage(session, &prepared).map_err(|e| e.to_string())?;
                session
                    .import_prepared_audio_clip_revision_mode(
                        prepared,
                        track,
                        start,
                        previous,
                        false,
                        Some(clip),
                    )
                    .map_err(|e| e.to_string())
            },
        )
    }
    /// The isolated render owner is moved into synchronous rendering and destroyed before
    /// decoding. The final importer retains the existing single ConvertMidiClipToAudio edit.
    pub(crate) fn bounce_midi_clip_using<R, T>(
        &mut self,
        clip: ClipId,
        directory: &Path,
        prepare: impl FnOnce(&Session, &Project) -> Result<R, String>,
        render: impl FnOnce(&Path, R, u64, u64, f32) -> Result<(), String>,
        commit: impl FnOnce(
            &mut Session,
            PreparedImport,
            hex_project::TrackId,
            u64,
        ) -> Result<(Created, T), String>,
    ) -> Result<(Created, T), String> {
        if self.audio_mode != DocumentAudioMode::Resident {
            return Err("clip bounce is unavailable for verified streaming".into());
        }
        let (start, frames) = self
            .project()
            .midi_bounce_range(clip)
            .map_err(|e| e.to_string())?;
        if frames > MAX_BOUNCE_FRAMES
            || self
                .resident_frames
                .checked_add(frames)
                .and_then(|frames| {
                    self.native_assets
                        .capacity_frames()
                        .ok()
                        .and_then(|originals| frames.checked_add(originals))
                })
                .is_none_or(|total| total > self.max_resident_frames)
            || self.media.len() >= MAX_ASSETS
        {
            return Err("clip bounce exceeds resident audio limits".into());
        }
        let (project, track, source_gain) = isolated_source(self.project(), clip)?;
        let revision = prepare(self, &project)?;
        fs::create_dir_all(directory)
            .map_err(|e| format!("could not create bounce folder: {e}"))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let directory = fs::canonicalize(directory)
            .map_err(|e| format!("could not resolve bounce folder: {e}"))?;
        let folder = directory.join(format!(
            "bounce-{}-{nonce}-{}",
            std::process::id(),
            NEXT_BOUNCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).map_err(|e| format!("could not reserve bounce folder: {e}"))?;
        let path = folder.join("audio.wav");
        if let Err(error) = render(&path, revision, start, frames, source_gain) {
            // Publication errors may leave a durable recovery file. Never remove it.
            let _ = fs::remove_dir(&folder);
            return Err(error.to_string());
        }
        let result = (|| {
            let decoded = hex_media::decode_wav_file(
                &path,
                hex_media::Limits::new(64 * 1024 * 1024 + 44, MAX_BOUNCE_FRAMES as usize),
            )
            .map_err(|e| e.to_string())?;
            let prepared = PreparedImport::new(decoded).map_err(|e| e.to_string())?;
            commit(self, prepared, track, start)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(&folder);
        }
        result
    }
}

fn isolated_source(
    project: &Project,
    clip: ClipId,
) -> Result<(Project, hex_project::TrackId, f32), String> {
    let track_id = project
        .tracks()
        .iter()
        .find(|track| track.midi_clips().iter().any(|item| item.id() == clip))
        .map(|track| track.id())
        .or_else(|| {
            project.session_cells().iter().find_map(|cell| {
                matches!(cell.content(), SessionCellContent::Midi(item) if item.id() == clip)
                    .then_some(cell.track())
            })
        })
        .ok_or("midi clip no longer exists")?;
    let owner = project
        .track(track_id)
        .ok_or("clip track no longer exists")?;
    if owner
        .devices()
        .iter()
        .take(owner.instrument_position().unwrap_or(0))
        .any(|device| !device.bypassed())
    {
        return Err("move effects after the instrument before bouncing this clip".into());
    }
    // Audio passes the instrument slot's host gains. Keep those controls live:
    // store raw synthesis divided by its input gain, then input/output apply once.
    let source_gain = owner
        .instrument()
        .filter(|device| !device.bypassed())
        .map_or(1.0, |device| device.state().input_gain_linear().recip());
    let mut snapshot = project.snapshot();
    let mut source = if let Some(item) = project.session_midi_clip(clip) {
        let source = item.source();
        let end = source.effective_end();
        MidiClipSnapshot {
            content_view: None,
            active: item.active(),
            playback_offset_raw: item.playback_offset().raw(),
            one_shot_boundary: item.source().one_shot_boundary(),
            volume: item.volume().clone(),
            name: item.name().to_owned(),
            flavor: hex_project::ClipFlavor::default(),
            id: clip.value(),
            start_raw: 0,
            length_raw: end.raw() - source.source_start().raw(),
            source_length_raw: source.source_length().raw(),
            source_start_raw: source.source_start().raw(),
            loop_start_raw: source.loop_start().raw(),
            loop_end_raw: source.loop_end().raw(),
            // The finite bounce length remains one original first-pass width.
            // Retain repeat law so elapsed windows project the same source phase.
            looping: source.looping(),
            notes: source
                .notes()
                .iter()
                .map(|note| hex_project::MidiNoteSnapshot {
                    id: note.id().value(),
                    active: note.active(),
                    start_raw: note.start().raw(),
                    duration_raw: note.duration().raw(),
                    pitch: note.pitch(),
                    velocity: note.velocity(),
                    channel: note.channel(),
                })
                .collect(),
        }
    } else {
        snapshot
            .tracks
            .iter()
            .find_map(|track| match &track.content {
                TrackContentSnapshot::Hybrid { midi_clips, .. }
                | TrackContentSnapshot::Instrument(midi_clips) => midi_clips
                    .iter()
                    .find(|item| item.id == clip.value())
                    .cloned(),
                _ => None,
            })
            .ok_or("midi clip no longer exists")?
    };
    // Conversion writes reusable source media. Authored inactivity belongs to
    // the replacement clip, never to the isolated synthesis pass.
    source.active = true;
    snapshot.tracks.retain(|track| track.id == track_id.value());
    snapshot.retain_automatic_track_names_after_projection();
    let track = snapshot
        .tracks
        .first_mut()
        .ok_or("clip track no longer exists")?;
    track.content = TrackContentSnapshot::Hybrid {
        audio_clips: Vec::new(),
        midi_clips: vec![source],
    };
    track.devices.clear();
    if let Some(instrument) = &mut track.instrument {
        instrument.state = instrument.state.clone().without_host_gains();
        instrument.comparison = instrument
            .comparison
            .take()
            .map(|comparison| comparison.with_global_trims_from(&instrument.state))
            .transpose()
            .map_err(|e| e.to_string())?;
    }
    // This is a disposable render extraction. Normalize its canonical source
    // and read projection together; the live retained definition stays untouched.
    snapshot
        .native_sources
        .retain(|source| Some(source.id) == owner.source_id().map(|id| id.value()));
    if let Some(instrument) = &track.instrument {
        let source = snapshot
            .native_sources
            .first_mut()
            .ok_or("clip source no longer exists")?;
        let definition = source
            .devices
            .get_mut(source.instrument_index)
            .ok_or("clip instrument no longer exists")?;
        *definition = instrument.clone();
    }
    track.instrument_position = track.instrument.as_ref().map(|_| 0);
    track.gain = 1.0;
    track.balance = 0.0;
    track.muted = false;
    track.soloed = false;
    track.gain_envelope.clear();
    track.gain_touch_spans.clear();
    track.sends.clear();
    // Current validation includes inactive A/B states. Keep exactly those source facts,
    // without decoding/carrying unrelated clip media into the isolated source graph.
    let required = owner
        .instrument()
        .into_iter()
        .flat_map(|device| device.authored_states())
        .flat_map(|state| state.asset_references())
        .map(|reference| reference.asset.value())
        .collect::<std::collections::BTreeSet<_>>();
    snapshot.assets.retain(|asset| required.contains(&asset.id));
    snapshot.session = Default::default();
    snapshot.return_tracks.clear();
    // The converted clip still passes through the canonical Main chain.
    // Exclude it only from this disposable instrument-source render.
    snapshot.main_devices.clear();
    snapshot.audio_feeds.clear();
    snapshot.midi_routes.clear();
    snapshot.return_feeds.clear();
    snapshot.loop_region = None;
    snapshot.master_gain = 1.0;
    Project::from_snapshot(snapshot)
        .map(|project| (project, track_id, source_gain))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod volume_tests {
    use super::*;
    use hex_project::{
        BeatTime, Command, Created, Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint,
    };
    #[test]
    fn isolated_console_and_arrangement_bounce_preserve_prepared_volume() {
        for console in [false, true] {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: "volume".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            let clip = if console {
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "scene".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                let Created::SessionClip(clip) = editor
                    .execute(Command::CreateSessionMidiClip {
                        track,
                        scene,
                        name: "clip".into(),
                        source_length: BeatTime::ONE,
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                clip
            } else {
                let Created::Clip(clip) = editor
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::ONE,
                    })
                    .unwrap()
                else {
                    panic!("clip")
                };
                clip
            };
            let volume = MidiClipVolume::new(
                vec![MidiClipVolumePoint::linear(
                    BeatTime::ZERO,
                    MidiClipGain::new(0.5).unwrap(),
                )],
                BeatTime::ONE,
            )
            .unwrap();
            editor
                .execute(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: volume.clone(),
                })
                .unwrap();
            editor
                .execute(Command::SetClipActive {
                    clip,
                    active: false,
                })
                .unwrap();
            let (isolated, owner, _) = isolated_source(editor.project(), clip).unwrap();
            assert!(!editor.project().clip_active(clip).unwrap());
            assert!(isolated.midi_clip(clip).unwrap().active());
            assert_eq!(owner, track);
            assert_eq!(isolated.midi_clip(clip).unwrap().volume(), &volume);
            assert!(hex_engine::compile::project(&isolated, &BTreeMap::new()).is_ok());
            let mut snapshot = editor.project().snapshot();
            if console {
                let hex_project::SessionCellContentSnapshot::Midi(cell) =
                    &mut snapshot.session.cells[0].content
                else {
                    panic!("midi")
                };
                cell.source.one_shot_boundary = hex_project::MidiOneShotBoundary::LoopEnd;
                cell.source.loop_end_raw = BeatTime::ONE.raw() / 2;
            } else {
                let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
                    &mut snapshot.tracks[0].content
                else {
                    panic!("hybrid")
                };
                midi_clips[0].one_shot_boundary = hex_project::MidiOneShotBoundary::LoopEnd;
                midi_clips[0].loop_end_raw = BeatTime::ONE.raw() / 2;
            }
            let retained = Project::from_snapshot(snapshot).unwrap();
            let (isolated, _, _) = isolated_source(&retained, clip).unwrap();
            assert_eq!(
                isolated
                    .midi_clip(clip)
                    .unwrap()
                    .source()
                    .one_shot_boundary(),
                hex_project::MidiOneShotBoundary::LoopEnd
            );
            assert_eq!(
                retained.midi_bounce_range(clip).unwrap(),
                (0, if console { 12000 } else { 24000 })
            );
            assert_eq!(
                isolated.midi_clip(clip).unwrap().length().raw(),
                if console {
                    BeatTime::ONE.raw() / 2
                } else {
                    BeatTime::ONE.raw()
                }
            );
            assert_eq!(isolated.midi_clip(clip).unwrap().volume(), &volume);
        }
    }
}
