//! Control-worker preparation cost, not UI or device deadline acceptance.
//! `--recovery` and `--no-recovery` compare note edits on the real worker;
//! `--duplicate-recovery` and `--duplicate-no-recovery` profile whole-clip
//! duplication against the same saved-project fixture. `--paste-recovery`
//! and `--paste-no-recovery` measure two queued clip pastes. No argument retains
//! the direct-session profile.
use hex_project::{
    BeatTime, Command, Created, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot, Project,
    Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
};
use hex_session::Session;
use hex_session::service::{ArrangementEditViewKind, Event, Request, Service};
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
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
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
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
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
        Some(arg) if arg == "--duplicate-recovery" => return profile_duplicate_service(true),
        Some(arg) if arg == "--duplicate-no-recovery" => return profile_duplicate_service(false),
        Some(arg) if arg == "--paste-recovery" => return profile_paste_service(true),
        Some(arg) if arg == "--paste-no-recovery" => return profile_paste_service(false),
        Some(arg) if arg == "--switch-recovery" => return profile_switch_service(true),
        Some(arg) if arg == "--switch-no-recovery" => return profile_switch_service(false),
        Some(arg) if arg == "--time-copy-recovery" => return profile_time_copy_service(true),
        Some(arg) if arg == "--time-copy-no-recovery" => return profile_time_copy_service(false),
        Some(_) => {
            return Err(
                "expected --recovery, --no-recovery, --duplicate-recovery, --duplicate-no-recovery, --paste-recovery, --paste-no-recovery, --switch-recovery, --switch-no-recovery, --time-copy-recovery, or --time-copy-no-recovery".into(),
            );
        }
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

struct WorkerProfile {
    service: Service,
    root: std::path::PathBuf,
    document: std::path::PathBuf,
    document_bytes: usize,
    recovery: bool,
}

impl WorkerProfile {
    fn open(project: &Project, recovery: bool) -> Result<(Self, Event), Box<dyn Error>> {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hex-recovery-profile-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        eprintln!("profile files: {} (retained on error)", root.display());
        let document = root.join("fixture.hex");
        let document_bytes = hex_storage::encode(project)?;
        std::fs::write(&document, &document_bytes)?;
        let service = if recovery {
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
        Ok((
            Self {
                service,
                root,
                document,
                document_bytes: document_bytes.len(),
                recovery,
            },
            opened,
        ))
    }

    fn verify_journal(&self, last_project: &Project) -> Result<(), Box<dyn Error>> {
        let journal = hex_storage::journal_path(&self.document)?;
        if self.recovery {
            // Future asynchronous implementations may publish after the terminal.
            // Poll separately from the timed interval and verify the full latest state.
            let until = Instant::now() + Duration::from_secs(5);
            while !hex_storage::load_journal(&journal).is_ok_and(|saved| saved == *last_project) {
                if Instant::now() >= until {
                    return Err(
                        "latest recovery checkpoint did not appear within five seconds".into(),
                    );
                }
                thread::sleep(Duration::from_millis(1));
            }
        } else if journal.exists() {
            return Err("no-recovery control unexpectedly created a journal".into());
        }
        Ok(())
    }

    fn close(mut self) -> Result<(), Box<dyn Error>> {
        self.service.shutdown(Duration::from_secs(2))?;
        drop(self.service);
        std::fs::remove_dir_all(self.root)?;
        Ok(())
    }
}

/// Measure the public worker's request-to-terminal latency with either a real
/// strict recovery journal or a no-recovery control. This is not UI timing.
fn profile_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    let project = fixture()?;
    let clip = project.tracks()[0].midi_clips()[0].id();
    let (profile, opened) = WorkerProfile::open(&project, recovery)?;
    let service = &profile.service;
    let mut times = Vec::with_capacity(20);
    let mut last_project = opened.state.project.clone();
    let mut revision = opened.state.revision;
    // One insert/remove pair warms the same worker and journal. The next ten
    // pairs are timed individually, including request admission and polling.
    for iteration in 0..11 {
        let started = Instant::now();
        submit(
            service,
            Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(253)?,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }),
        )?;
        let added = next_terminal(service)?;
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
        submit(service, Request::Edit(Command::RemoveMidiNote(note)))?;
        let removed = next_terminal(service)?;
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
    profile.verify_journal(&last_project)?;
    println!(
        "service recovery={recovery}: 100 tracks, 6400 notes, {} document bytes, 48000 hz, one warmup pair, 20 edit terminals; p50={}us p95={}us max={}us; latest journal {}",
        profile.document_bytes,
        (times[9] + times[10]) / 2,
        times[18],
        times[19],
        if recovery { "verified" } else { "disabled" }
    );
    profile.close()
}

/// Measure source-bound whole-clip duplication on the public worker. The
/// committed display sample is a headless observation, not an audio callback
/// acknowledgment or an isolated checkpoint measurement.
fn profile_duplicate_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    let project = fixture()?;
    let clip = project.tracks()[0].midi_clips()[0].id();
    let (profile, opened) = WorkerProfile::open(&project, recovery)?;
    let service = &profile.service;
    let baseline = opened.state.project.snapshot();

    // Opening the fixture advances the default document generation from zero
    // to one. DuplicateClips requires this source identity on every submission.
    let project_generation = 1;
    let track = opened.state.project.tracks()[0].id();
    let mut revision = opened.state.revision;
    let mut last_project = opened.state.project.clone();
    let mut duplicate_times = Vec::with_capacity(10);
    let mut observation_times = Vec::with_capacity(10);

    // One complete duplicate/undo pair warms the worker and journal. Ten
    // further pairs provide bounded repeated measurements on the same fixture.
    for iteration in 0..11 {
        let started = Instant::now();
        submit(
            service,
            Request::DuplicateClips {
                project_generation,
                revision,
                clips: vec![clip],
            },
        )?;
        let (duplicated, elapsed, observed_at) =
            next_duplicate_terminal(service, project_generation, revision, track, started)?;
        revision = revision.checked_add(1).ok_or("revision exhausted")?;
        if duplicated.state.revision != revision {
            return Err("clip duplication acknowledged the wrong revision".into());
        }
        let Some(Created::Clips { first, last }) = duplicated.created else {
            return Err("clip duplication did not return created clips".into());
        };
        if first != last
            || duplicated.state.project.track(track).is_none_or(|track| {
                track.midi_clips().len() != 2
                    || !track
                        .midi_clips()
                        .iter()
                        .any(|candidate| candidate.id() == first)
            })
        {
            return Err("clip duplication did not produce exactly one duplicate".into());
        }
        if iteration > 0 {
            duplicate_times.push(elapsed);
            if let Some(observed_at) = observed_at {
                observation_times.push((observed_at, elapsed.saturating_sub(observed_at)));
            }
            println!(
                "duplicate recovery={recovery} iteration={} submit_to_terminal={}us{}",
                iteration,
                elapsed,
                observed_at.map_or_else(
                    || "; headless_display_observation=missed".to_owned(),
                    |observed| format!(
                        "; submit_to_headless_display_observation={observed}us; observation_to_terminal={}us",
                        elapsed.saturating_sub(observed)
                    )
                )
            );
        }

        submit(service, Request::Undo)?;
        let undone = next_terminal(service)?;
        revision = revision.checked_add(1).ok_or("revision exhausted")?;
        let mut restored = undone.state.project.snapshot();
        // Undo preserves the allocator high-water mark from the duplicate.
        restored.next_id = baseline.next_id;
        if undone.state.revision != revision || restored != baseline {
            return Err("undo did not restore the fixture project".into());
        }
        last_project = undone.state.project.clone();
    }

    duplicate_times.sort_unstable();
    profile.verify_journal(&last_project)?;
    println!(
        "duplicate summary recovery={recovery}: 100 tracks, 6400 notes, {} document bytes, 48000 hz, 1 warmup and 10 duplicate/undo trials; submit-to-terminal p50={}us p95={}us max={}us; latest journal {}",
        profile.document_bytes,
        (duplicate_times[4] + duplicate_times[5]) / 2,
        duplicate_times[9],
        duplicate_times[9],
        if recovery { "verified" } else { "absent" }
    );
    if !observation_times.is_empty() {
        let mut submit_to_observation: Vec<_> = observation_times
            .iter()
            .map(|(observed, _)| *observed)
            .collect();
        let mut observation_to_terminal: Vec<_> = observation_times
            .iter()
            .map(|(_, terminal)| *terminal)
            .collect();
        submit_to_observation.sort_unstable();
        observation_to_terminal.sort_unstable();
        let middle = submit_to_observation.len() / 2;
        println!(
            "headless display observation: received {}/10; submit-to-observation median={}us; observation-to-terminal median={}us (not audio callback or exclusive checkpoint timing)",
            observation_times.len(),
            submit_to_observation[middle],
            observation_to_terminal[middle]
        );
    }
    profile.close()
}

/// Two queued pastes through the real worker and optional durable journal.
/// Observation timing is headless, not native paint or audio callback timing.
fn profile_paste_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::sync::{Arc, Mutex};

    let project = fixture()?;
    let (profile, opened) = WorkerProfile::open(&project, recovery)?;
    let service = &profile.service;
    let track = opened.state.project.tracks()[0].id();
    let clip = opened.state.project.tracks()[0].midi_clips()[0].id();
    let baseline = opened.state.project.snapshot();
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    service.set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
        if let Some(request) = event
            .request
            .filter(|request| request.kind == "paste_time_range")
            && let Ok(mut records) = sink.lock()
        {
            records.push((
                request.request_id,
                event.stage,
                Instant::now(),
                event.details.clone(),
                event.details_truncated,
            ));
        }
    })))?;
    submit(service, Request::CopyClips(vec![clip]))?;
    next_terminal(service)?;
    let mut revision = opened.state.revision;
    let started_run = Instant::now();
    for iteration in 0..4 {
        records
            .lock()
            .map_err(|_| "diagnostic lock poisoned")?
            .clear();
        let started = Instant::now();
        submit(
            service,
            Request::PasteTimeRange {
                track,
                frame: 8_000_000,
            },
        )?;
        submit(
            service,
            Request::PasteTimeRange {
                track,
                frame: 16_000_000,
            },
        )?;
        let mut observed = [None, None];
        let mut terminals = Vec::new();
        let until = started + Duration::from_secs(5);
        while terminals.len() < 2 {
            if Instant::now() >= until || started_run.elapsed() >= Duration::from_secs(20) {
                return Err("queued paste measurement timed out".into());
            }
            loop {
                match service.try_arrangement_edit_view() {
                    Ok(view) => {
                        if view.project_generation == 1
                            && matches!(view.kind, ArrangementEditViewKind::Paste)
                            && let Some(state) = view.state.upgrade()
                        {
                            for (index, slot) in observed.iter_mut().enumerate() {
                                if view.previous_revision == revision + index as u64
                                    && state.revision == view.previous_revision + 1
                                {
                                    *slot = Some(started.elapsed().as_micros());
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            match service.try_event() {
                Ok(event) => {
                    verify_event(&event)?;
                    if event.request_complete {
                        terminals.push(event);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => thread::yield_now(),
                Err(error) => return Err(error.into()),
            }
        }
        if terminals[0].state.revision != revision + 1
            || terminals[1].state.revision != revision + 2
            || terminals[1]
                .state
                .project
                .track(track)
                .is_none_or(|t| t.midi_clips().len() != 3)
        {
            return Err("queued paste states or ordering differ".into());
        }
        let [first, second] = terminals.as_slice() else {
            return Err("queued paste terminal count differs".into());
        };
        let (
            Some(Created::Clips {
                first: first_clip,
                last: first_last,
            }),
            Some(Created::Clips {
                first: second_clip,
                last: second_last,
            }),
        ) = (first.created, second.created)
        else {
            return Err("queued pastes did not report clips".into());
        };
        if first_clip != first_last || second_clip != second_last || first_clip == second_clip {
            return Err("queued paste clip identities differ".into());
        }
        profile.verify_journal(&terminals[1].state.project)?;
        let rows = records.lock().map_err(|_| "diagnostic lock poisoned")?;
        let ids: Vec<_> = rows
            .iter()
            .filter_map(|(id, stage, _, _, _)| {
                matches!(stage, DiagnosticStage::SubmitAttempt).then_some(*id)
            })
            .collect();
        if ids.len() != 2 {
            return Err("paste diagnostic submissions differ".into());
        }
        for (index, id) in ids.iter().enumerate() {
            let find = |predicate: fn(&DiagnosticStage) -> bool| {
                rows.iter()
                    .find(|(candidate, stage, _, _, _)| candidate == id && predicate(stage))
            };
            let submit = find(|stage| matches!(stage, DiagnosticStage::SubmitAttempt))
                .ok_or("missing submit")?;
            let begin = find(|stage| matches!(stage, DiagnosticStage::WorkerBegin { .. }))
                .ok_or("missing worker begin")?;
            let outcome = find(|stage| {
                matches!(
                    stage,
                    DiagnosticStage::OutcomeReady {
                        request_complete: true,
                        error: false,
                        ..
                    }
                )
            })
            .ok_or("missing outcome")?;
            if outcome.4 {
                return Err("paste diagnostics were truncated".into());
            }
            let recovery_us = outcome
                .3
                .split_whitespace()
                .find_map(|field| field.strip_prefix("paste_recovery_us="));
            if recovery && recovery_us.is_none() {
                return Err("missing recovery timing".into());
            }
            println!(
                "paste recovery={recovery} iteration={iteration} request={} submit_to_worker_us={} worker_to_outcome_us={} paste_recovery_us={} pair_start_to_headless_view_us={}",
                index + 1,
                begin.2.duration_since(submit.2).as_micros(),
                outcome.2.duration_since(begin.2).as_micros(),
                recovery_us.unwrap_or("not-recorded"),
                observed[index].map_or_else(|| "not-observed".into(), |n| n.to_string())
            );
        }
        drop(rows);
        submit(service, Request::Undo)?;
        let first_undo = next_terminal(service)?;
        let clips = first_undo
            .state
            .project
            .track(track)
            .ok_or("track disappeared")?
            .midi_clips();
        if clips.len() != 2
            || !clips.iter().any(|clip| clip.id() == first_clip)
            || clips.iter().any(|clip| clip.id() == second_clip)
        {
            return Err("first Undo did not remove only the second paste".into());
        }
        submit(service, Request::Undo)?;
        let undone = next_terminal(service)?;
        revision += 4;
        profile.verify_journal(&undone.state.project)?;
        let mut restored = undone.state.project.snapshot();
        restored.next_id = baseline.next_id;
        if restored != baseline {
            return Err("paste Undo did not restore the project".into());
        }
    }
    println!(
        "paste workload: 100 tracks, 6400 notes, {} document bytes, 48000 hz, 1 warmup and 3 measured pairs; observer enabled; headless view is not native paint or audio",
        profile.document_bytes
    );
    profile.close()
}

fn next_duplicate_terminal(
    service: &Service,
    project_generation: u64,
    previous_revision: u64,
    track_id: hex_project::TrackId,
    started: Instant,
) -> Result<(Event, u128, Option<u128>), Box<dyn Error>> {
    let expected_revision = previous_revision
        .checked_add(1)
        .ok_or("revision exhausted")?;
    let until = started + Duration::from_secs(5);
    let mut observed = None;
    let receive_view = || -> Result<bool, Box<dyn Error>> {
        let observation = match service.try_arrangement_edit_view() {
            Ok(observation) => observation,
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let state = observation
            .state
            .upgrade()
            .ok_or("duplicate display observation state expired")?;
        if !matches!(
            observation.kind,
            ArrangementEditViewKind::Duplicate(Created::Clips { .. })
        ) || observation.project_generation != project_generation
            || observation.previous_revision != previous_revision
            || state.revision != expected_revision
            || state
                .project
                .track(track_id)
                .is_none_or(|track| track.midi_clips().len() != 2)
        {
            return Err(
                "duplicate display observation did not match its source and committed state".into(),
            );
        }
        Ok(true)
    };
    loop {
        if Instant::now() >= until {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "duplicate terminal timeout").into(),
            );
        }
        if observed.is_none() && receive_view()? {
            observed = Some(started.elapsed().as_micros());
        }
        match service.try_event() {
            Ok(event) => {
                let elapsed = started.elapsed().as_micros();
                verify_event(&event)?;
                if event.request_complete {
                    // Drain a view published between the two polls so it cannot
                    // be mistaken for the next duplicate. This sample has no
                    // separately observed display-to-terminal interval.
                    if observed.is_none() {
                        receive_view()?;
                    }
                    return Ok((event, elapsed, observed));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => thread::yield_now(),
            Err(error) => return Err(error.into()),
        }
    }
}

fn verify_event(event: &Event) -> Result<(), Box<dyn Error>> {
    if let Some(error) = &event.error {
        return Err(error.clone().into());
    }
    if let Some(failure) = &event.recovery_failure {
        return Err(format!("automatic recovery failed: {}", failure.explanation).into());
    }
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
        verify_event(&event)?;
        if event.request_complete {
            return Ok(event);
        }
    }
}

/// Measure queued switches on the actual control worker; this is not native UI or audio timing.
fn profile_switch_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::sync::{Arc, Mutex};
    let project = fixture()?;
    let (profile, opened) = WorkerProfile::open(&project, recovery)?;
    let service = &profile.service;
    let track = opened.state.project.tracks()[0].id();
    let mut revision = opened.state.revision;
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    service.set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
        if let Some(request) = event
            .request
            .filter(|request| request.operation == Some("toggle_mute"))
            && let Ok(mut records) = sink.lock()
        {
            records.push((
                request.request_id,
                event.stage,
                Instant::now(),
                event.details.clone(),
                event.details_truncated,
            ));
        }
    })))?;
    let started_run = Instant::now();
    for iteration in 0..4 {
        records
            .lock()
            .map_err(|_| "diagnostic lock poisoned")?
            .clear();
        let started = Instant::now();
        submit(service, Request::Edit(Command::ToggleMute(track)))?;
        submit(service, Request::Edit(Command::ToggleMute(track)))?;
        let mut observed = [None, None];
        let mut terminals = Vec::new();
        let until = started + Duration::from_secs(5);
        while terminals.len() < 2 {
            if Instant::now() >= until || started_run.elapsed() >= Duration::from_secs(20) {
                return Err("queued switch measurement timed out".into());
            }
            loop {
                match service.try_track_switch_view() {
                    Ok(view) => {
                        if view.project_generation == 1 {
                            for (index, slot) in observed.iter_mut().enumerate() {
                                if view.previous_revision == revision + index as u64
                                    && view.revision == view.previous_revision + 1
                                {
                                    *slot = Some(started.elapsed().as_micros());
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            match service.try_event() {
                Ok(event) => {
                    verify_event(&event)?;
                    if event.request_complete {
                        terminals.push(event);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => thread::yield_now(),
                Err(error) => return Err(error.into()),
            }
        }
        if terminals[0].state.revision != revision + 1
            || terminals[1].state.revision != revision + 2
            || !terminals[0]
                .state
                .project
                .track(track)
                .ok_or("track missing")?
                .muted()
            || terminals[1]
                .state
                .project
                .track(track)
                .ok_or("track missing")?
                .muted()
        {
            return Err("switch states or ordering differ".into());
        }
        profile.verify_journal(&terminals[1].state.project)?;
        let rows = records.lock().map_err(|_| "diagnostic lock poisoned")?;
        let ids: Vec<_> = rows
            .iter()
            .filter_map(|(id, stage, _, _, _)| {
                matches!(stage, DiagnosticStage::SubmitAttempt).then_some(*id)
            })
            .collect();
        if ids.len() != 2 {
            return Err("switch diagnostic submissions differ".into());
        }
        for (index, id) in ids.iter().enumerate() {
            let find = |predicate: fn(&DiagnosticStage) -> bool| {
                rows.iter()
                    .find(|(candidate, stage, _, _, _)| candidate == id && predicate(stage))
            };
            let submit = find(|stage| matches!(stage, DiagnosticStage::SubmitAttempt))
                .ok_or("missing submit")?;
            let begin = find(|stage| matches!(stage, DiagnosticStage::WorkerBegin { .. }))
                .ok_or("missing worker begin")?;
            let outcome = find(|stage| {
                matches!(
                    stage,
                    DiagnosticStage::OutcomeReady {
                        request_complete: true,
                        error: false,
                        ..
                    }
                )
            })
            .ok_or("missing outcome")?;
            if outcome.4 {
                return Err("switch diagnostics were truncated".into());
            }
            let recovery_us = outcome
                .3
                .split_whitespace()
                .find_map(|field| field.strip_prefix("track_switch_recovery_us="));
            if recovery && recovery_us.is_none() {
                return Err("missing recovery timing".into());
            }
            println!(
                "switch recovery={recovery} iteration={iteration} request={} submit_to_worker_us={} worker_to_outcome_us={} track_switch_recovery_us={} pair_start_to_headless_view_us={}",
                index + 1,
                begin.2.duration_since(submit.2).as_micros(),
                outcome.2.duration_since(begin.2).as_micros(),
                recovery_us.unwrap_or("not-recorded"),
                observed[index].map_or_else(|| "not-observed".into(), |n| n.to_string())
            );
        }
        drop(rows);
        submit(service, Request::Undo)?;
        if !next_terminal(service)?
            .state
            .project
            .track(track)
            .ok_or("track missing")?
            .muted()
        {
            return Err("first Undo did not restore mute".into());
        }
        submit(service, Request::Undo)?;
        let undone = next_terminal(service)?;
        if undone
            .state
            .project
            .track(track)
            .ok_or("track missing")?
            .muted()
        {
            return Err("second Undo did not restore unmuted".into());
        }
        revision += 4;
        profile.verify_journal(&undone.state.project)?;
    }
    println!(
        "switch workload: 100 tracks, 6400 notes, {} document bytes, 48000 hz, 1 warmup and 3 measured pairs; observer enabled; headless view is not native paint or audio",
        profile.document_bytes
    );
    profile.close()
}

/// Measure queued range copies on the actual control worker; not native UI or audio timing.
fn profile_time_copy_service(recovery: bool) -> Result<(), Box<dyn Error>> {
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::sync::{Arc, Mutex};
    let project = fixture()?;
    let (profile, opened) = WorkerProfile::open(&project, recovery)?;
    let service = &profile.service;
    let track = opened.state.project.tracks()[0].id();
    let baseline = opened.state.project.snapshot();
    let mut revision = opened.state.revision;
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    service.set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
        if let Some(request) = event
            .request
            .filter(|request| request.operation == Some("copy_time_range_to"))
            && let Ok(mut records) = sink.lock()
        {
            records.push((
                request.request_id,
                event.stage,
                Instant::now(),
                event.details.clone(),
                event.details_truncated,
            ));
        }
    })))?;
    for iteration in 0..4 {
        records
            .lock()
            .map_err(|_| "diagnostic lock poisoned")?
            .clear();
        for start in [0, 24_000] {
            let range = hex_project::TimeRange::new(vec![track], start, start + 24_000)?;
            submit(
                service,
                Request::Edit(Command::CopyTimeRangeTo {
                    range,
                    track,
                    frame: start + 24_000,
                }),
            )?;
        }
        let first = next_terminal(service)?;
        let second = next_terminal(service)?;
        if first.state.revision != revision + 1 || second.state.revision != revision + 2 {
            return Err("copy revisions or ordering differ".into());
        }
        profile.verify_journal(&second.state.project)?;
        let rows = records.lock().map_err(|_| "diagnostic lock poisoned")?;
        let ids: Vec<_> = rows
            .iter()
            .filter_map(|(id, stage, _, _, _)| {
                matches!(stage, DiagnosticStage::SubmitAttempt).then_some(*id)
            })
            .collect();
        if ids.len() != 2 {
            return Err("copy diagnostic submissions differ".into());
        }
        for (index, id) in ids.iter().enumerate() {
            let find = |predicate: fn(&DiagnosticStage) -> bool| {
                rows.iter()
                    .find(|(candidate, stage, _, _, _)| candidate == id && predicate(stage))
            };
            let submit = find(|stage| matches!(stage, DiagnosticStage::SubmitAttempt))
                .ok_or("missing submit")?;
            let begin = find(|stage| matches!(stage, DiagnosticStage::WorkerBegin { .. }))
                .ok_or("missing worker begin")?;
            let outcome = find(|stage| {
                matches!(
                    stage,
                    DiagnosticStage::OutcomeReady {
                        request_complete: true,
                        error: false,
                        ..
                    }
                )
            })
            .ok_or("missing outcome")?;
            if outcome.4 {
                return Err("copy diagnostics truncated".into());
            }
            let recovery_us = outcome
                .3
                .split_whitespace()
                .find_map(|field| field.strip_prefix("duplicate_recovery_us="));
            if recovery && recovery_us.is_none() {
                return Err("missing recovery timing".into());
            }
            println!(
                "time-copy recovery={recovery} iteration={iteration} request={} submit_to_worker_us={} worker_to_outcome_us={} duplicate_recovery_us={}",
                index + 1,
                begin.2.duration_since(submit.2).as_micros(),
                outcome.2.duration_since(begin.2).as_micros(),
                recovery_us.unwrap_or("not-recorded")
            );
        }
        drop(rows);
        submit(service, Request::Undo)?;
        let first_undo = next_terminal(service)?;
        let mut expected_first = first.state.project.snapshot();
        let first_undo_snapshot = first_undo.state.project.snapshot();
        expected_first.next_id = first_undo_snapshot.next_id;
        if expected_first != first_undo_snapshot {
            return Err("first Undo did not restore first copy".into());
        }
        submit(service, Request::Undo)?;
        let undone = next_terminal(service)?;
        let mut expected_original = baseline.clone();
        let restored = undone.state.project.snapshot();
        expected_original.next_id = restored.next_id;
        if expected_original != restored {
            return Err("second Undo did not restore project".into());
        }
        revision += 4;
        profile.verify_journal(&undone.state.project)?;
    }
    println!(
        "time-copy workload: 100 tracks, 6400 notes, {} document bytes, 48000 hz, 1 warmup and 3 measured pairs; headless worker, not native paint or audio",
        profile.document_bytes
    );
    profile.close()
}
