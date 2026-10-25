//! Complete control-side current-schema decode; generated metadata only, no files or audio I/O.
use hex_project::{AssetSnapshot, ClipSnapshot, Project, Snapshot, TrackSnapshot};
use std::{hint::black_box, time::Instant};
fn fixture(track_count: u64, clip_count: u64) -> Result<Project, Box<dyn std::error::Error>> {
    let mut next_id = 2;
    let mut tracks = Vec::new();
    for index in 0..track_count {
        let id = next_id;
        next_id += 1;
        let mut clips = Vec::new();
        for clip in 0..clip_count {
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
    let project = Project::from_snapshot(Snapshot {
        arrangement_sections: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        return_feeds: Vec::new(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
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
    Ok(project)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for (tracks, clips) in [(1, 16), (100, 100)] {
        let project = fixture(tracks, clips)?;
        let expected = project.snapshot();
        let bytes = hex_storage::encode(&project)?;
        let mut times = Vec::new();
        for iteration in 0..24 {
            let start = Instant::now();
            let actual = black_box(hex_storage::decode(black_box(&bytes))?);
            let elapsed = start.elapsed().as_secs_f64() * 1e6;
            if actual.snapshot() != expected {
                return Err("decoded complete snapshot differs".into());
            }
            if hex_storage::encode(&actual)? != bytes {
                return Err("re-encoded bytes differ".into());
            }
            if iteration >= 8 {
                times.push(elapsed);
            }
        }
        times.sort_by(f64::total_cmp);
        println!(
            "STORAGE_CSV,{tracks},{clips},{},{:.3},{:.3},{:.3}",
            bytes.len(),
            times[8],
            times[14],
            times[15]
        );
    }
    println!("storage profile: complete snapshots and exact bytes checked");
    Ok(())
}
