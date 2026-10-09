//! Control-worker preparation cost, not UI or device deadline acceptance.
use hex_project::{
    BeatTime, Command, Created, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use hex_session::Session;
use std::{error::Error, hint::black_box, time::Instant};

fn fixture() -> Result<Project, hex_project::Invalid> {
    let mut next_id = 1;
    let mut tracks = Vec::new();
    for index in 0..100 {
        let track_id = next_id;
        next_id += 1;
        let clip_id = next_id;
        next_id += 1;
        let notes = (0..64)
            .map(|note| {
                let id = next_id;
                next_id += 1;
                MidiNoteSnapshot {
                    id,
                    start_raw: (note * 4) << 32,
                    duration_raw: 1 << 32,
                    pitch: 48 + (index % 36) as u8,
                    channel: 0,
                    velocity: 100,
                }
            })
            .collect();
        tracks.push(TrackSnapshot {
            id: track_id,
            name: format!("instrument {index:03}"),
            gain: 0.1,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                id: clip_id,
                start_raw: 0,
                length_raw: 256 << 32,
                source_length_raw: 256 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 256 << 32,
                looping: false,
                notes,
            }]),
        });
    }
    Project::from_snapshot(Snapshot {
        sample_rate: 48_000,
        next_id,
        assets: Vec::new(),
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let project = fixture()?;
    let track = project.tracks()[0].id();
    let clip = project.tracks()[0].midi_clips()[0].id();
    let mut session = Session::from_loaded(project, 32, 8_000_000, Vec::new())?;
    black_box(session.compile()?);
    // A real note transaction each time: 20 insert/remove pairs including history
    // and candidate compilation, retaining canonical model and bounded history.
    let mut times = Vec::new();
    for iteration in 0..20 {
        let started = Instant::now();
        let (created, compiled) = session.command_prepared(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(253)?,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })?;
        black_box(compiled);
        times.push(started.elapsed().as_micros());
        let Created::Note(note) = created else {
            return Err("benchmark note was not created".into());
        };
        let started = Instant::now();
        black_box(session.command_prepared(Command::RemoveMidiNote(note))?);
        times.push(started.elapsed().as_micros());
        black_box(iteration);
    }
    times.sort_unstable();
    println!(
        "100 tracks, 6400 notes, 32 history slots, 48000 hz, one warm-up compile, 40 insert/remove transactions: p50={}us p95={}us max={}us",
        times[19] + (times[20] - times[19]) / 2,
        times[37],
        times[39]
    );
    let started = Instant::now();
    black_box(session.command_prepared(Command::Gain { track, gain: 0.2 })?);
    println!(
        "track gain edit and candidate compilation: {}us",
        started.elapsed().as_micros()
    );
    Ok(())
}
