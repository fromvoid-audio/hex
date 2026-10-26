use hex_project::{Command, Created, Editor, Project};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let mut editor = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Imported {track, clip: first, asset} = editor.execute(Command::ImportAudioTrack {
        name: "retained source".into(), path: "Media/original.wav".into(),
        sample_rate: 48_000, frames: 4096,
    })? else {return Err("import did not create audio".into())};
    editor.execute(Command::SetClipFades {clip: first, fade_in: 127, fade_out: 251})?;
    editor.execute(Command::SetAudioClipGain {clip: first, millidb: -3141})?;
    let Created::Clip(second) = editor.execute(Command::PlaceClip {
        track, asset, start: 5000, duration: 1024, source_offset: 173,
        source_length: 1024, looping: false,
    })? else {return Err("placement did not create clip".into())};
    // The selected operation is substituted at build time; each historical
    // writer receives only the command supported by its own immutable source.
    HISTORICAL_OPERATION
    let encoded = hex_storage::encode(editor.project())?;
    assert_eq!(hex_storage::decode(&encoded)?.snapshot(), editor.project().snapshot());
    std::fs::write(&args[1], encoded)?;
    Ok(())
}
