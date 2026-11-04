//! Canonical Session control preparation; no audio callback or UI is exercised.
use hex_engine::{PreparedRevision, PreparedSessionInventory};
use hex_project::{
    BeatTime, Command, Created, Editor, Meter, MeterPoint, MidiNoteSnapshot, MidiSourceSnapshot,
    Project, SceneSnapshot, SessionCellContentSnapshot, SessionCellSnapshot,
    SessionMidiClipSnapshot, SessionSnapshot, Snapshot, Tempo, TempoPoint, TrackContentSnapshot,
    TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const TRACKS: usize = 100;
const RUNS: usize = 100;
const PREVIOUS_INLINE_SAMPLER_BYTES: usize = 40;

fn fixture() -> Result<Project, Box<dyn Error>> {
    let scenes = (1..=3)
        .map(|id| SceneSnapshot {
            id,
            name: format!("scene {id}"),
        })
        .collect();
    let mut next_id = 4_u64;
    let mut tracks = Vec::new();
    let mut cells = Vec::new();
    for index in 0..TRACKS {
        let track = next_id;
        next_id += 1;
        let instrument_id = next_id;
        next_id += 1;
        tracks.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track,
            name: format!("instrument {index:03}"),
            gain: 0.1,
            balance: (index % 5) as f32 * 0.2 - 0.4,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(hex_project::DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: instrument_id,
                bypassed: false,
                state: hex_project::default_instrument_state()?,
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Instrument(Vec::new()),
            sends: Vec::new(),
        });
        for scene in 1..=2 {
            let clip = next_id;
            next_id += 1;
            let voices = 4 * (1 + (index + scene as usize) % 4);
            let mut notes = Vec::new();
            for voice in 0..voices {
                let id = next_id;
                next_id += 1;
                notes.push(MidiNoteSnapshot {
                    id,
                    active: true,
                    start_raw: 0,
                    duration_raw: 2 << 32,
                    pitch: 48 + ((index + voice) % 36) as u8,
                    channel: 0,
                    velocity: 80 + (index % 21) as u8,
                });
            }
            cells.push(SessionCellSnapshot {
                track,
                scene,
                content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                    active: true,
                    playback_offset_raw: 0,
                    volume: hex_project::MidiClipVolume::default(),
                    flavor: hex_project::ClipFlavor::default(),
                    id: clip,
                    name: format!("pattern {index:03}.{scene}"),
                    source: MidiSourceSnapshot {
                        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                        source_length_raw: 4 << 32,
                        source_start_raw: 0,
                        loop_start_raw: 0,
                        loop_end_raw: 4 << 32,
                        looping: true,
                        notes,
                    },
                }),
            });
        }
        if index % 2 == 0 {
            cells.push(SessionCellSnapshot {
                track,
                scene: 3,
                content: SessionCellContentSnapshot::Stop,
            });
        }
    }
    Ok(Project::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        next_id,
        assets: Vec::new(),
        tracks,
        session: SessionSnapshot { scenes, cells },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?)
}

fn report(name: &str, times: &mut [u128]) {
    times.sort_unstable();
    println!(
        "{name}: runs={} p50={}ns p95={}ns p99={}ns max={}ns",
        times.len(),
        times[times.len() / 2],
        times[(times.len() * 95).div_ceil(100) - 1],
        times[(times.len() * 99).div_ceil(100) - 1],
        times[times.len() - 1]
    );
}

fn reused(old: &PreparedSessionInventory, new: &PreparedSessionInventory) -> usize {
    new.midi_clips()
        .iter()
        .filter(|clip| {
            old.midi_clip(clip.id).is_some_and(|before| {
                std::ptr::eq(before.source_template(), clip.source_template())
            })
        })
        .count()
}

fn main() -> Result<(), Box<dyn Error>> {
    let project = fixture()?;
    let notes: usize = project
        .session_cells()
        .iter()
        .filter_map(|cell| match cell.content() {
            hex_project::SessionCellContent::Midi(clip) => Some(clip.source().notes().len()),
            hex_project::SessionCellContent::Audio(_) => None,
            hex_project::SessionCellContent::Stop => None,
        })
        .sum();
    let track = project.tracks().first().ok_or("missing track")?.id();
    let table = Arc::new(hex_devices::SineTable::new());
    let mut previous = PreparedSessionInventory::prepare(&project, Arc::clone(&table), None)?;
    let clip = previous.midi_clips().first().ok_or("missing midi clip")?.id;
    println!(
        "canonical fixture: tracks={} scenes={} midi_cells={} stop_cells=50 absent_cells=50 notes={} rate=48000 history_slots=8; preparation and canonical edits only",
        project.tracks().len(),
        project.scenes().len(),
        previous.midi_clips().len(),
        notes
    );
    println!("structural reachable envelope: {:?}", previous.work());
    let sampler_references = previous.midi_note_bank_reference_count();
    let old_sampler_bytes = sampler_references
        .checked_mul(PREVIOUS_INLINE_SAMPLER_BYTES)
        .ok_or("old sampler byte estimate overflow")?;
    let indexed_bytes = previous
        .midi_note_bank_storage_bytes()
        .checked_add(
            sampler_references
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or("indexed sampler reference byte count overflow")?,
        )
        .ok_or("indexed sampler byte estimate overflow")?;
    println!(
        "note storage estimate: refs={} indexed_bytes={} previous_inline_sampler_bytes={} (bank entries include keys, owner and per-template handles)",
        sampler_references, indexed_bytes, old_sampler_bytes
    );
    let mut times = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let prepared =
            PreparedSessionInventory::prepare(black_box(&project), Arc::clone(&table), None)?;
        black_box(&prepared);
        times.push(started.elapsed().as_nanos());
    }
    report(
        "cold inventory (one excluded warmup; destruction outside timer)",
        &mut times,
    );
    let mut editor = Editor::new(project, 8)?;
    times.clear();
    for index in 0..RUNS {
        let started = Instant::now();
        editor.execute(Command::Gain {
            track,
            gain: if index % 2 == 0 { 0.2 } else { 0.1 },
        })?;
        let prepared = PreparedSessionInventory::prepare(
            editor.project(),
            Arc::clone(&table),
            Some(&previous),
        )?;
        black_box(&prepared);
        times.push(started.elapsed().as_nanos());
        if reused(&previous, &prepared) != 200 {
            return Err("gain edit did not retain all 200 source templates".into());
        }
        previous = prepared;
    }
    report("gain transaction plus reused inventory", &mut times);
    times.clear();
    for _ in 0..RUNS / 2 {
        let started = Instant::now();
        let created = editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(3)?,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })?;
        let prepared = PreparedSessionInventory::prepare(
            editor.project(),
            Arc::clone(&table),
            Some(&previous),
        )?;
        black_box(&prepared);
        times.push(started.elapsed().as_nanos());
        if reused(&previous, &prepared) != 199 {
            return Err("note insertion did not rebuild exactly one source template".into());
        }
        previous = prepared;
        let Created::Note(note) = created else {
            return Err("expected created note".into());
        };
        let started = Instant::now();
        editor.execute(Command::RemoveMidiNote(note))?;
        let prepared = PreparedSessionInventory::prepare(
            editor.project(),
            Arc::clone(&table),
            Some(&previous),
        )?;
        black_box(&prepared);
        times.push(started.elapsed().as_nanos());
        if reused(&previous, &prepared) != 199 {
            return Err("note removal did not rebuild exactly one source template".into());
        }
        previous = prepared;
    }
    report("note insert/remove transaction plus inventory", &mut times);
    println!("inventory-only reuse checks outside timer: gain 200/200, note edit 199/200");
    let media = BTreeMap::new();
    let mut revision = PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::clone(&table),
        Some(&previous),
    )?;
    times.clear();
    for index in 0..RUNS {
        let started = Instant::now();
        let (_, prepared) = editor.execute_prepared(
            Command::Gain {
                track,
                gain: if index % 2 == 0 { 0.2 } else { 0.1 },
            },
            |candidate| {
                PreparedRevision::prepare(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    Some(revision.session()),
                )
                .map_err(|error| hex_project::Invalid(error.0))
            },
        )?;
        black_box(&prepared);
        times.push(started.elapsed().as_nanos());
        if reused(revision.session(), prepared.session()) != 200 {
            return Err("full revision gain edit did not retain all source templates".into());
        }
        revision = prepared;
    }
    report("gain transaction plus full prepared revision", &mut times);
    println!(
        "full revision compiles 100 empty Arrangement tracks plus Session inventory and preview inside the candidate transaction; no worker queues, native UI, DSP or device deadline acceptance"
    );
    Ok(())
}
