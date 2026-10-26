//! Execute only against the recorded schema43 source; no media is read or written.
//! Paths and opaque fixture devices are metadata-only test inputs, not playable assets.
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, Project, RoutingOwner};
use std::{error::Error, io::Write};
fn state(value: u8) -> Result<NativeDeviceState, Box<dyn Error>> {
    Ok(NativeDeviceState::new(DeviceTypeId::from_static("fixture.group-track.effect"), 1, vec![value, 7, 11])?)
}
fn main() -> Result<(), Box<dyn Error>> {
    if hex_storage::CURRENT_SCHEMA_VERSION != 43 {
        return Err("historical fixture requires its recorded schema43 writer source".into());
    }
    let output = std::env::args_os().nth(1).ok_or("missing create-new output path")?;
    let mut e = Editor::new(Project::new(48_000)?, 16)?;
    let Created::Imported {track: audio, clip: first, asset} = e.execute(Command::ImportAudioTrack {
        name: "audio fixture".into(), path: "Media/metadata-only.wav".into(),
        sample_rate: 48_000, frames: 4096,
    })? else {return Err("audio import did not return identities".into())};
    e.execute(Command::SetClipFades {clip: first, fade_in: 127, fade_out: 251})?;
    e.execute(Command::SetAudioClipGain {clip: first, millidb: -3141})?;
    e.execute(Command::ReverseClips(vec![first]))?;
    let Created::Clip(second) = e.execute(Command::PlaceClip {
        track: audio, asset, start: 5000, duration: 1024, source_offset: 173,
        source_length: 1024, looping: false,
    })? else {return Err("clip placement did not return identity".into())};
    e.execute(Command::GroupClips(vec![first, second]))?;
    e.execute(Command::InsertDevice {track: audio, before: None, state: state(23)?})?;
    let Created::Track(notes) = e.execute(Command::AddMidiTrack {name: "midi fixture".into()})?
        else {return Err("midi track did not return identity".into())};
    e.execute(Command::InsertInstrument {track: notes, state: NativeDeviceState::new(
        DeviceTypeId::from_static("fixture.group-track.instrument"), 1, vec![31, 37],
    )?})?;
    let Created::Clip(midi) = e.execute(Command::PlaceMidiClip {
        track: notes, start: BeatTime::from_quarters(4)?, length: BeatTime::from_quarters(4)?,
    })? else {return Err("midi clip did not return identity".into())};
    e.execute(Command::AddMidiNote {clip: midi, start: BeatTime::ONE, duration: BeatTime::ONE,
        pitch: 67, channel: 2, velocity: 101})?;
    e.execute(Command::ReplaceMidiClipVolume {clip: midi, volume: MidiClipVolume::new(vec![
        MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.5)?),
        MidiClipVolumePoint::linear(BeatTime::ONE, MidiClipGain::new(1.5)?),
    ], BeatTime::from_quarters(4)?)?})?;
    e.execute(Command::ReplaceTrackGainEnvelope {track: audio, points: vec![
        GainPoint {incoming_gain: None, beat: BeatTime::ZERO, gain: 0.75, outgoing: GainInterpolation::Linear},
        GainPoint {incoming_gain: Some(0.625), beat: BeatTime::from_quarters(3)?, gain: 0.5, outgoing: GainInterpolation::Step},
    ]})?;
    let Created::Track(empty) = e.execute(Command::AddTrack {name: "empty fixture".into()})?
        else {return Err("empty track did not return identity".into())};
    let Created::Scene(scene) = e.execute(Command::AddScene {name: "scene fixture".into()})?
        else {return Err("scene did not return identity".into())};
    e.execute(Command::CreateSessionAudioClip {track: audio, scene, name: "session audio".into(),
        asset, source_start: 5, source_end: 100})?;
    e.execute(Command::CreateSessionMidiClip {track: notes, scene, name: "session midi".into(),
        source_length: BeatTime::ONE})?;
    e.execute(Command::SetSessionStopCell {track: empty, scene})?;
    let Created::ReturnTrack(a) = e.execute(Command::AddReturnTrack {name: "return a".into()})?
        else {return Err("return did not return identity".into())};
    let Created::ReturnTrack(b) = e.execute(Command::AddReturnTrack {name: "return b".into()})?
        else {return Err("return did not return identity".into())};
    e.execute(Command::SetReturnSend {track: audio, return_track: a, gain: 0.375})?;
    e.execute(Command::SetReturnFeed {source: a, destination: b, gain: 0.25})?;
    e.execute(Command::ReturnGain {return_track: a, gain: 0.625})?;
    e.execute(Command::InsertReturnDevice {return_track: b, before: None, state: state(41)?})?;
    e.execute(Command::SetAudioFeed {source: RoutingOwner::Track(audio), destination: notes, gain: 0.125})?;
    e.execute(Command::SetMidiSource {destination: notes, source: Some(audio)})?;
    e.execute(Command::InsertMainDevice {before: None, state: state(47)?})?;
    e.execute(Command::MasterGain {gain: 0.875})?;
    let encoded = hex_storage::encode(e.project())?;
    if hex_storage::decode(&encoded)?.snapshot() != e.project().snapshot() {
        return Err("historical writer round-trip differs".into());
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(output)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(())
}
