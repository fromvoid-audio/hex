//! Create-new mixed audio/MIDI fixtures for the native time-selection profiler.
//! Alternating 0.75s audio/MIDI placements retain 100 clips and 400 notes per
//! track across 75s. Historical overlapping 50s fixtures are not equivalent
//! performance inputs; generate new fixtures without rewriting existing files.
//! Optional --audio-only preserves the same audio placements without MIDI clips.
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipGain, ClipSnapshot, Meter, MeterPoint, MidiClipSnapshot,
    MidiNoteSnapshot, Project, Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

fn project(count: usize, wav: &Path, audio_only: bool) -> Result<Project, Box<dyn Error>> {
    if ![1, 100].contains(&count) {
        return Err("expected one or 100 tracks".into());
    }
    let mut next_id = 2_u64;
    let mut id = || {
        let value = next_id;
        next_id += 1;
        value
    };
    let length = BeatTime::from_ratio(3, 2)?.raw();
    let mut tracks = Vec::new();
    for index in 0..count {
        let track = id();
        let mut audio_clips = Vec::new();
        let mut midi_clips = Vec::new();
        for position in 0..50_u64 {
            audio_clips.push(ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: id(),
                asset: 1,
                start: position * 72_000,
                duration: 36_000,
                source_offset: 0,
                source_length: 48_000,
                looping: false,
                source_phase: 0,
                edit: None,
                fade: ClipFade::neutral(36_000)?,
                gain: ClipGain::UNITY,
            });
            let clip = id();
            let mut notes = Vec::new();
            for note in 0..8_u64 {
                notes.push(MidiNoteSnapshot {
                    active: true,
                    id: id(),
                    start_raw: BeatTime::from_ratio(note * 3, 16)?.raw(),
                    duration_raw: BeatTime::from_ratio(1, 8)?.raw(),
                    pitch: 48 + (note % 12) as u8,
                    channel: 0,
                    velocity: 100,
                });
            }
            midi_clips.push(MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip,
                start_raw: BeatTime::from_ratio(position * 6 + 3, 2)?.raw(),
                length_raw: length,
                source_length_raw: length,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: length,
                looping: false,
                notes,
            });
        }
        tracks.push(TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            id: track,
            name: format!("profile {index:03}"),
            gain: 0.1,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            sends: Vec::new(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            content: if audio_only {
                TrackContentSnapshot::Audio(audio_clips)
            } else {
                TrackContentSnapshot::Hybrid {
                    audio_clips,
                    midi_clips,
                }
            },
        });
    }
    Ok(Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: wav.to_path_buf(),
            source_sample_rate: 48_000,
            source_frames: 48_000,
            playback_frames: 48_000,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks,
        return_tracks: Vec::new(),
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
    })?)
}
fn wav(path: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = 48_000_u32 * 4;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&48_000_u32.to_le_bytes())?;
    file.write_all(&192_000_u32.to_le_bytes())?;
    file.write_all(&4_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    for frame in 0..48_000 {
        let sample = ((frame % 100) as i16 - 50) * 100;
        file.write_all(&sample.to_le_bytes())?;
        file.write_all(&sample.to_le_bytes())?;
    }
    file.sync_all()?;
    Ok(())
}
fn generate(directory: &Path, audio_only: bool) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let directory = fs::canonicalize(directory)?;
    for name in ["shared.wav", "1.hex", "100.hex"] {
        if directory.join(name).exists() {
            return Err(format!("fixture already exists: {name}").into());
        }
    }
    let media = directory.join("shared.wav");
    // Validate both complete snapshots before creating files.
    let small = project(1, &media, audio_only)?;
    let dense = project(100, &media, audio_only)?;
    wav(&media)?;
    hex_storage::save_new(directory.join("1.hex"), &small)?;
    hex_storage::save_new(directory.join("100.hex"), &dense)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .ok_or("usage: time_selection_fixture directory [--audio-only]")?;
    let audio_only = match args.next() {
        None => false,
        Some(flag) if flag == "--audio-only" => true,
        Some(_) => return Err("expected --audio-only or no second argument".into()),
    };
    if args.next().is_some() {
        return Err("expected exactly one fixture directory".into());
    }
    generate(Path::new(&directory), audio_only)?;
    writeln!(
        std::io::stdout().lock(),
        "created 1/100-track fixtures; audio_only={audio_only}; shared 48000 hz stereo wav"
    )?;

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_sizes_are_hybrid_with_identical_density() -> Result<(), Box<dyn Error>> {
        for count in [1, 100] {
            let project = project(count, Path::new("shared.wav"), false)?;
            let audio_only = super::project(count, Path::new("shared.wav"), true)?;
            assert_eq!(project.tracks().len(), count);
            assert_eq!(audio_only.snapshot().assets, project.snapshot().assets);
            for (mixed, audio) in project.tracks().iter().zip(audio_only.tracks()) {
                assert_eq!(mixed.clips(), audio.clips());
                assert!(audio.midi_clips().is_empty());
            }
            for track in project.tracks() {
                assert_eq!(track.clips().len(), 50);
                assert_eq!(track.midi_clips().len(), 50);
                assert_eq!(track.clips().last().unwrap().start(), 49 * 72_000);
                let last = track.midi_clips().last().unwrap();
                assert_eq!(
                    project.tempo_map().beat_to_frame(BeatTime::from_raw(
                        last.start().raw() + last.length().raw()
                    ))?,
                    3_600_000
                );
                assert!(track.midi_clips().iter().all(|clip| {
                    clip.notes().iter().all(|note| {
                        note.start().raw() + note.duration().raw() <= clip.length().raw()
                    })
                }));
                assert!(
                    track
                        .midi_clips()
                        .iter()
                        .all(|clip| clip.notes().len() == 8)
                );
            }
        }
        Ok(())
    }
}
