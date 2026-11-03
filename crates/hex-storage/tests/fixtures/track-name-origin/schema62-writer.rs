// Run against 6de4bd2d40fa248370c80311553de0c71f21cece, the schema62 writer.
use hex_project::{Command, Created, Editor, Project, TrackDeviceSlot};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 62 { return Err("requires schema62 writer".into()); }
    let path = std::env::args_os().nth(1).ok_or("output path missing")?;
    let mut e = Editor::new(Project::new(48_000)?, 16)?;
    // Historical UI default names; schema62 cannot tell them from user names.
    e.execute(Command::InsertTrack { name: "track 1".into(), before_track: None })?;
    e.execute(Command::InsertTrack { name: "audio 2".into(), before_track: None })?;
    e.execute(Command::AddTrack { name: "lead".into() })?;
    let Created::Track(_) = e.execute(Command::InsertTrackWithDevice { name: "new track".into(), before_track: None, slot: TrackDeviceSlot::Instrument, state: hex_project::default_instrument_state()? })? else { return Err("instrument".into()) };
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() { return Err("round trip".into()); }
    std::fs::write(path, encoded)?;
    Ok(())
}
