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

const MAX_BOUNCE_FRAMES: u64 = 8_000_000;
static NEXT_BOUNCE: AtomicU64 = AtomicU64::new(0);

impl Session {
    /// Bounce only this clip's instrument, before retained track effects/mix.
    /// Arrangement length is preserved exactly; tails beyond the clip are cut.
    /// Session bounces one initial source pass, since audio cells are one-shots.
    pub fn bounce_midi_clip(
        &mut self,
        clip: ClipId,
        directory: &Path,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), String> {
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
                .is_none_or(|total| total > self.max_resident_frames)
            || self.media.len() >= MAX_ASSETS
        {
            return Err("clip bounce exceeds resident audio limits".into());
        }
        let (project, track, source_gain) = isolated_source(self.project(), clip)?;
        let revision = hex_engine::PreparedRevision::prepare(
            &project,
            &BTreeMap::new(),
            self.sine_table(),
            None,
        )
        .map(Arc::new)
        .map_err(|e| e.to_string())?;
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
        if let Err(error) =
            hex_export::render_bounce_new(&path, revision, start, frames, source_gain)
        {
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
            self.import_prepared_audio_clip_revision_mode(
                prepared,
                track,
                start,
                previous,
                false,
                Some(clip),
            )
            .map_err(|e| e.to_string())
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
    let source = if let Some(item) = project.session_midi_clip(clip) {
        let source = item.source();
        let end = source.effective_end();
        MidiClipSnapshot {
            name: item.name().to_owned(),
            flavor: hex_project::ClipFlavor::default(),
            id: clip.value(),
            start_raw: 0,
            length_raw: end
                .raw()
                .checked_sub(source.source_start().raw())
                .filter(|length| *length > 0)
                .ok_or("no remaining one-shot material")?,
            source_length_raw: source.source_length().raw(),
            source_start_raw: source.source_start().raw(),
            loop_start_raw: source.loop_start().raw(),
            loop_end_raw: source.loop_end().raw(),
            looping: source.looping(),
            one_shot_boundary: source.one_shot_boundary(),
            notes: source
                .notes()
                .iter()
                .map(|note| hex_project::MidiNoteSnapshot {
                    id: note.id().value(),
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
    snapshot.tracks.retain(|track| track.id == track_id.value());
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
    }
    track.instrument_position = track.instrument.as_ref().map(|_| 0);
    track.gain = 1.0;
    track.balance = 0.0;
    track.muted = false;
    track.soloed = false;
    track.gain_envelope.clear();
    track.gain_touch_spans.clear();
    track.sends.clear();
    snapshot.assets.clear();
    snapshot.session = Default::default();
    snapshot.return_tracks.clear();
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
