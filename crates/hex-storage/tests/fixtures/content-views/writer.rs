use hex_project::{BeatTime, SessionCellContentSnapshot, TrackContentSnapshot};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let input = std::fs::read(&args[1])?;
    let project = hex_storage::decode(&input)?;
    let mut snapshot = project.snapshot();
    for track in &mut snapshot.tracks {
        match &mut track.content {
            TrackContentSnapshot::Hybrid { audio_clips, midi_clips } => {
                for clip in audio_clips { clip.active = false; }
                for clip in midi_clips { clip.active = false; clip.playback_offset_raw = BeatTime::from_quarters(16)?.raw(); }
            }
            TrackContentSnapshot::Audio(clips) => for clip in clips { clip.active = false; },
            TrackContentSnapshot::Instrument(clips) => for clip in clips { clip.active = false; clip.playback_offset_raw = BeatTime::from_quarters(16)?.raw(); },
        }
    }
    for cell in &mut snapshot.session.cells {
        match &mut cell.content {
            SessionCellContentSnapshot::Audio(clip) => clip.active = false,
            SessionCellContentSnapshot::Midi(clip) => { clip.active = false; clip.playback_offset_raw = BeatTime::from_quarters(16)?.raw(); },
            SessionCellContentSnapshot::Stop => (),
        }
    }
    let project = hex_project::Project::from_snapshot(snapshot)?;
    let bytes = hex_storage::encode(&project)?;
    assert_eq!(hex_storage::decode(&bytes)?.snapshot(), project.snapshot());
    std::fs::write(&args[2], bytes)?;
    Ok(())
}
