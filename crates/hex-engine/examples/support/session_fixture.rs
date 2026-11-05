//! Original canonical workload shared by offline Session benchmarks.
use hex_engine::{LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand};
use hex_project::{
    BeatTime, LoopRegion, Meter, MeterPoint, MidiClipSnapshot, MidiNoteSnapshot,
    MidiSourceSnapshot, Project, SceneSnapshot, SessionCellContentSnapshot, SessionCellSnapshot,
    SessionMidiClipSnapshot, SessionSnapshot, Snapshot, Tempo, TempoPoint, TrackContentSnapshot,
    TrackSnapshot,
};
use std::{collections::BTreeMap, error::Error, sync::Arc};

pub const RATE: u32 = 48_000;
const TRACKS: usize = 100;

fn notes(next: &mut u64, track: usize, transposition: u8) -> Vec<MidiNoteSnapshot> {
    (0..4 * (1 + track % 2))
        .map(|voice| {
            let id = *next;
            *next += 1;
            MidiNoteSnapshot {
                active: true,
                id,
                start_raw: 0,
                duration_raw: 2 << 32,
                pitch: 36 + ((track + voice) % 24) as u8 + transposition,
                channel: 0,
                velocity: 80 + (track % 21) as u8,
            }
        })
        .collect()
}

pub fn project(loop_tempo: bool) -> Result<Project, Box<dyn Error>> {
    let mut next = 2;
    let mut tracks = Vec::with_capacity(TRACKS);
    let mut cells = Vec::with_capacity(TRACKS / 2);
    for index in 0..TRACKS {
        let track = next;
        let clip = next + 1;
        let instrument = next + 2;
        next += 3;
        let arrangement_notes = notes(&mut next, index, 0);
        tracks.push(TrackSnapshot {
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track,
            name: format!("track {index:03}"),
            gain: 0.3 + (index % 7) as f32 * 0.05,
            balance: (index % 9) as f32 * 0.1 - 0.4,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(hex_project::DeviceSnapshot {
                automation: Vec::new(),
                comparison: None,
                id: instrument,
                bypassed: false,
                state: hex_project::default_instrument_state()?,
            }),
            instrument_position: Some(0),
            content: TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: 4 << 32,
                source_start_raw: 0,
                loop_start_raw: 0,
                loop_end_raw: 4 << 32,
                looping: true,
                notes: arrangement_notes,
            }]),
            sends: Vec::new(),
        });
        if index % 2 == 0 {
            let clip = next;
            next += 1;
            cells.push(SessionCellSnapshot {
                track,
                scene: 1,
                content: SessionCellContentSnapshot::Midi(SessionMidiClipSnapshot {
                    active: true,
                    playback_offset_raw: 0,
                    volume: hex_project::MidiClipVolume::default(),
                    flavor: hex_project::ClipFlavor::default(),
                    id: clip,
                    name: format!("session {index:03}"),
                    source: MidiSourceSnapshot {
                        one_shot_boundary: hex_project::MidiOneShotBoundary::SourceEnd,
                        source_length_raw: 4 << 32,
                        source_start_raw: 0,
                        loop_start_raw: 0,
                        loop_end_raw: 4 << 32,
                        looping: true,
                        notes: notes(&mut next, index, 12),
                    },
                }),
            });
        }
    }
    let project = Project::from_legacy_snapshot(Snapshot {
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
        sidechains: Vec::new(),
        import_provenance: None,
        sample_rate: RATE,
        next_id: next,
        assets: Vec::new(),
        tracks,
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 1,
                name: "mixed".into(),
            }],
            cells,
        },
        loop_region: Some(LoopRegion::new(
            0,
            if loop_tempo { 120_000 } else { 72_000 },
            true,
        )?),
        master_gain: 0.73,
        tempo_points: if loop_tempo {
            vec![
                TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
                TempoPoint::new(BeatTime::ONE, Tempo::new(60_000_000)?),
                // This point is beyond the repeating three-beat segment.
                TempoPoint::new(BeatTime::from_quarters(5)?, Tempo::new(240_000_000)?),
            ]
        } else {
            vec![TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT)]
        },
        meter_points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
        return_tracks: Vec::new(),
    })?;
    Ok(project)
}

pub fn finite_oracle(
    project: &Project,
    loop_tempo: bool,
) -> Result<hex_engine::Project, Box<dyn Error>> {
    // Independent finite compiler oracle: replace selected Session tracks with
    // their canonical sources and express Arrangement wraps as three-beat
    // source periods. Expand the repeating tempo pattern into ordinary points.
    // This oracle never calls the ongoing query or the Session clock.
    let mut oracle = project.snapshot();
    let cells = std::mem::take(&mut oracle.session.cells);
    oracle.session.scenes.clear();
    oracle.loop_region = None;
    for (index, track) in oracle.tracks.iter_mut().enumerate() {
        if index % 2 == 0 {
            let cell = cells
                .iter()
                .find(|cell| cell.track == track.id)
                .ok_or("missing oracle Session source")?;
            let SessionCellContentSnapshot::Midi(clip) = &cell.content else {
                return Err("oracle source is not midi".into());
            };
            track.content = TrackContentSnapshot::Instrument(vec![MidiClipSnapshot {
                content_view: None,
                active: true,
                playback_offset_raw: 0,
                one_shot_boundary: clip.source.one_shot_boundary,
                volume: hex_project::MidiClipVolume::default(),
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                id: clip.id,
                start_raw: 0,
                length_raw: 16 << 32,
                source_length_raw: clip.source.source_length_raw,
                source_start_raw: clip.source.source_start_raw,
                loop_start_raw: clip.source.loop_start_raw,
                loop_end_raw: clip.source.loop_end_raw,
                looping: clip.source.looping,
                notes: clip.source.notes.clone(),
            }]);
        } else if let TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } = &mut track.content
        {
            for clip in clips {
                clip.source_length_raw = 3 << 32;
                clip.loop_end_raw = 3 << 32;
            }
        }
    }
    if loop_tempo {
        oracle.tempo_points.clear();
        for cycle in 0..6 {
            oracle.tempo_points.push(TempoPoint::new(
                BeatTime::from_quarters(cycle * 3)?,
                Tempo::DEFAULT,
            ));
            oracle.tempo_points.push(TempoPoint::new(
                BeatTime::from_quarters(cycle * 3 + 1)?,
                Tempo::new(60_000_000)?,
            ));
        }
    }
    Ok(hex_engine::compile::project(
        &Project::from_snapshot(oracle)?,
        &BTreeMap::new(),
    )?)
}

pub fn kernel(revision: Arc<PreparedRevision>) -> Result<SessionPlayback, Box<dyn Error>> {
    let scene = revision
        .session()
        .scenes()
        .first()
        .ok_or("missing fixture scene")?
        .id;
    let mut playback = SessionPlayback::new(revision)?;
    playback.apply(SessionPlaybackCommand::LaunchScene {
        scene,
        quantization: LaunchQuantization::Immediate,
    })?;
    playback.apply(SessionPlaybackCommand::Play)?;
    Ok(playback)
}
