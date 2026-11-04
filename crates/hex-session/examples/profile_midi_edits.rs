//! Control-worker preparation cost, not UI or device deadline acceptance.
//! `--recovery` and `--no-recovery` compare the real worker with the same
//! saved-project fixture; no argument retains the direct-session profile.
use hex_project::{
    BeatTime, Command, Created, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use hex_session::Session;
use hex_session::service::{Event, Request, Service};
use std::{
    error::Error,
    hint::black_box,
    io, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
                    active: true,
                    start_raw: (note * 4) << 32,
                    duration_raw: 1 << 32,
                    pitch: 48 + (index % 36) as u8,
                    channel: 0,
                    velocity: 100,
                }
            })
            .collect();
        tracks.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: format!("instrument {index:03}"),
            gain: 0.1,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
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
            sends: Vec::new(),
        });
    }
    Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id,
        assets: Vec::new(),
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args_os().nth(1) {
        Some(arg) if arg == "--recovery" => return profile_service(true),
        Some(arg) if arg == "--no-recovery" => return profile_service(false),
        Some(_) => return Err("expected --recovery or --no-recovery".into()),
        None => {}
    }
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

/// Measure the public worker's request-to-terminal latency with either a real
/// strict recovery journal or a no-recovery control. This is not UI timing.
fn profile_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    let project = fixture()?;
    let clip = project.tracks()[0].midi_clips()[0].id();
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-recovery-profile-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir(&root)?;
    eprintln!("profile files: {} (retained on error)", root.display());
    let document = root.join("fixture.hex");
    let document_bytes = hex_storage::encode(&project)?;
    std::fs::write(&document, &document_bytes)?;
    let mut service = if recovery {
        Service::with_recovery(48_000, 32, 8_000_000, root.join("untitled.hex-journal"))?
    } else {
        Service::new(48_000, 32, 8_000_000)?
    };
    let initial = next_event(&service, Instant::now() + Duration::from_secs(5))?;
    if let Some(error) = initial.error {
        return Err(error.into());
    }
    submit(&service, Request::Open(document.clone()))?;
    let opened = next_terminal(&service)?;
    if !opened.open_complete || !opened.project_replaced {
        return Err("fixture open did not replace the project".into());
    }
    let mut times = Vec::with_capacity(20);
    let mut last_project = opened.state.project.clone();
    let mut revision = opened.state.revision;
    // One insert/remove pair warms the same worker and journal. The next ten
    // pairs are timed individually, including request admission and polling.
    for iteration in 0..11 {
        let started = Instant::now();
        submit(
            &service,
            Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(253)?,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }),
        )?;
        let added = next_terminal(&service)?;
        let elapsed = started.elapsed();
        revision = revision.checked_add(1).ok_or("revision exhausted")?;
        if added.state.revision != revision {
            return Err("note insertion acknowledged the wrong revision".into());
        }
        let Some(Created::Note(note)) = added.created else {
            return Err("note insertion was not acknowledged".into());
        };
        if iteration > 0 {
            times.push(elapsed.as_micros());
        }
        let started = Instant::now();
        submit(&service, Request::Edit(Command::RemoveMidiNote(note)))?;
        let removed = next_terminal(&service)?;
        let elapsed = started.elapsed();
        revision = revision.checked_add(1).ok_or("revision exhausted")?;
        if removed.state.revision != revision {
            return Err("note removal acknowledged the wrong revision".into());
        }
        if iteration > 0 {
            times.push(elapsed.as_micros());
        }
        last_project = removed.state.project.clone();
    }
    times.sort_unstable();
    let journal = hex_storage::journal_path(&document)?;
    if recovery {
        // Future asynchronous implementations may publish after the terminal.
        // Poll separately from the timed interval and verify the full latest state.
        let until = Instant::now() + Duration::from_secs(5);
        while !hex_storage::load_journal(&journal).is_ok_and(|saved| saved == *last_project) {
            if Instant::now() >= until {
                return Err("latest recovery checkpoint did not appear within five seconds".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
    } else if journal.exists() {
        return Err("no-recovery control unexpectedly created a journal".into());
    }
    println!(
        "service recovery={recovery}: 100 tracks, 6400 notes, {} document bytes, 48000 hz, one warmup pair, 20 edit terminals; p50={}us p95={}us max={}us; latest journal {}",
        document_bytes.len(),
        (times[9] + times[10]) / 2,
        times[18],
        times[19],
        if recovery { "verified" } else { "disabled" }
    );
    service.shutdown(Duration::from_secs(2))?;
    drop(service);
    std::fs::remove_dir_all(root)?;
    Ok(())
}

fn submit(service: &Service, request: Request) -> Result<(), Box<dyn Error>> {
    service
        .submit(request)
        .map_err(|error| io::Error::other(format!("request admission failed: {error:?}")).into())
}

fn next_event(service: &Service, until: Instant) -> Result<Event, Box<dyn Error>> {
    loop {
        match service.try_event() {
            Ok(event) => return Ok(event),
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < until => {
                thread::yield_now()
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                return Err(
                    io::Error::new(io::ErrorKind::TimedOut, "session event timeout").into(),
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn next_terminal(service: &Service) -> Result<Event, Box<dyn Error>> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() >= until {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "session terminal timeout").into());
        }
        let event = next_event(service, until)?;
        if let Some(error) = &event.error {
            return Err(error.clone().into());
        }
        if let Some(failure) = &event.recovery_failure {
            return Err(format!("automatic recovery failed: {}", failure.explanation).into());
        }
        if event.request_complete {
            return Ok(event);
        }
    }
}
