//! Metadata/history benchmark; excludes compilation, UI, disk and audio scheduling.
use hex_project::{
    AssetSnapshot, ClipSnapshot, Command, Created, Editor, Project, Snapshot, TrackSnapshot,
};
use std::{hint::black_box, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut next_id = 2;
    let mut tracks = Vec::new();
    for index in 0..100 {
        let id = next_id;
        next_id += 1;
        let mut clips = Vec::new();
        for clip in 0..100 {
            clips.push(ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: next_id,
                asset: 1,
                start: clip * 4096,
                duration: 64,
                source_offset: 0,
                source_length: 64,
                looping: false,
                source_phase: 0,
                fade: hex_project::ClipFade::neutral(64)?,
            });
            next_id += 1;
        }
        tracks.push(TrackSnapshot {
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id,
            name: format!("track {index}"),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: hex_project::TrackContentSnapshot::Audio(clips),
            sends: Vec::new(),
        });
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
        sample_rate: 48_000,
        next_id,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 1,
            path: "generated/shared.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 64,
            playback_frames: 64,
            conversion_version: 0,
        }],
        tracks,
        session: Default::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![hex_project::TempoPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Tempo::DEFAULT,
        )],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })?;
    let track = project.tracks()[0].id();
    let mut editor = Editor::new(project, 32)?;
    let mut timings = Vec::with_capacity(200);
    for index in 0..210 {
        let command = Command::Gain {
            track,
            gain: if index % 2 == 0 { 0.5 } else { 1.0 },
        };
        let start = Instant::now();
        let result = editor.execute(command)?;
        black_box(editor.project());
        if result != Created::None {
            return Err("unexpected edit result".into());
        }
        if index >= 10 {
            timings.push(start.elapsed().as_nanos());
        }
    }
    timings.sort_unstable();
    println!(
        "100 tracks × 100 clips; 32-step metadata history; 10 warmup / 200 edits; {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    println!(
        "gain transaction: p50={:.1}µs p95={:.1}µs p99={:.1}µs max={:.1}µs",
        timings[100] as f64 / 1000.0,
        timings[190] as f64 / 1000.0,
        timings[198] as f64 / 1000.0,
        timings[199] as f64 / 1000.0
    );
    Ok(())
}
