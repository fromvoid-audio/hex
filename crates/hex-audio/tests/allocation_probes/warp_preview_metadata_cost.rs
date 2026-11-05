//! Control-side metadata cost only: no PCM/render, callback, OS audio or native UI.
use hex_project::{
    BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, TrimEdge, WarpMode,
    default_audio_warp, preview_audio_clip_stretch,
};
use std::time::Instant;

fn profile(markers: usize, cycles: u64) {
    let frames = 48_000;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "cost".into(),
            path: "cost.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 96_000,
            duration: frames * cycles,
            source_offset: 0,
            source_length: frames,
            looping: true,
        })
        .unwrap()
    else {
        panic!("place")
    };
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    let unit = settings.markers.last().unwrap().beat.raw();
    settings.markers = (0..markers)
        .map(|index| {
            let denominator = (markers - 1) as u128;
            hex_project::WarpMarker {
                source_frame: (u128::from(frames) * index as u128 / denominator) as u64,
                beat: BeatTime::from_raw((u128::from(unit) * index as u128 / denominator) as u64),
            }
        })
        .collect();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap();
    let tempo = editor.project().tempo_map();
    hex_engine::audio_transform::validate_clip_warp_preview(original, tempo).unwrap();
    let requested = original.start() + original.duration() * 5 / 4;
    let started = Instant::now();
    let info = allocation_counter::measure(|| {
        for _ in 0..32 {
            let preview =
                preview_audio_clip_stretch(original, TrimEdge::Right, requested, tempo, frames)
                    .unwrap();
            hex_engine::audio_transform::validate_clip_warp_preview(&preview, tempo).unwrap();
            std::hint::black_box(&preview);
        }
    });
    let elapsed = started.elapsed();
    assert_eq!(
        (info.count_current, info.bytes_current),
        (0, 0),
        "all temporary model/map allocations reclaimed"
    );
    eprintln!(
        "point-admission cost markers={markers} cycles={cycles} raw-boundaries={} probes=32 elapsed={elapsed:?} per-point={:?} allocations={info:?}",
        markers as u64 * cycles,
        elapsed / 32
    );
}

#[test]
fn representative_and_maximum_raw_boundary_capture_point_admission_cost() {
    profile(2, 1);
    profile(1_024, 8); // Actual raw8192 constructor cap, without dedup pretending lower cost.
}
