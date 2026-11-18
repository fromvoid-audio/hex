use super::{
    SourceEdge, SourceWindow, WAVEFORM_COLUMNS, adjusted_source_range, bin_range, source_step,
    source_window_is_current, waveform_bars,
};
use hex_project::{Command, Created, Editor, Project, SessionCellContent};
use hex_session::{CachedPeak, CachedWaveform};

#[test]
fn source_waveform_is_cropped_to_the_half_open_one_shot_window() {
    let waveform = CachedWaveform::from_peaks(
        9,
        (0..9)
            .map(|index| {
                let value = if (3..7).contains(&index) { 0.75 } else { 0.0 };
                [
                    CachedPeak {
                        min: -value,
                        max: value,
                    },
                    CachedPeak { min: 0.0, max: 0.0 },
                ]
            })
            .collect(),
    )
    .expect("valid waveform fixture");
    let bars = waveform_bars(&waveform, 3, 7);
    assert_eq!(bars.len(), WAVEFORM_COLUMNS);
    assert!(bars.iter().all(|value| *value == 0.75));
    assert_eq!(bin_range(&waveform, 3, 7), 3..7);
    assert!(
        waveform_bars(&waveform, 7, 7)
            .iter()
            .all(|value| *value == 0.0)
    );
}

#[test]
fn source_nudges_keep_the_half_open_window_inside_the_asset() {
    assert_eq!(
        adjusted_source_range(1, 10, 10, SourceEdge::StartEarlier, 1),
        (0, 10)
    );
    assert_eq!(
        adjusted_source_range(1, 10, 10, SourceEdge::StartLater, 1),
        (2, 10)
    );
    assert_eq!(
        adjusted_source_range(1, 10, 10, SourceEdge::EndEarlier, 1),
        (1, 9)
    );
    assert_eq!(
        adjusted_source_range(1, 10, 10, SourceEdge::EndLater, 1),
        (1, 10)
    );
    assert_eq!(source_step(48_000, false), 48);
    assert_eq!(source_step(44_100, false), 44);
    assert_eq!(source_step(192_000, true), 1);
    assert_eq!(source_step(1, false), 1);
    assert_eq!(
        adjusted_source_range(10, 20, 100, SourceEdge::StartLater, 48),
        (19, 20)
    );
    assert_eq!(
        adjusted_source_range(10, 20, 100, SourceEdge::EndEarlier, 48),
        (10, 11)
    );
    assert_eq!(
        adjusted_source_range(10, 20, 100, SourceEdge::EndLater, u64::MAX),
        (10, 100)
    );
}

#[test]
fn source_controls_reject_stale_generation_revision_or_selection() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("audio track")
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .expect("scene")
    {
        Created::Scene(id) => id,
        _ => panic!("scene"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "audio".into(),
            asset,
            source_start: 4,
            source_end: 20,
        })
        .expect("Session audio clip")
    {
        Created::SessionClip(id) => id,
        _ => panic!("Session clip"),
    };
    let window = SourceWindow {
        clip,
        asset,
        track,
        scene,
        start: 4,
        end: 20,
        asset_frames: 32,
        revision: 7,
        project_generation: 3,
    };
    let project = editor.project();
    assert!(source_window_is_current(
        window,
        3,
        Some((track, scene)),
        7,
        project
    ));
    assert!(!source_window_is_current(
        window,
        4,
        Some((track, scene)),
        7,
        project
    ));
    assert!(!source_window_is_current(
        window,
        3,
        Some((track, scene)),
        8,
        project
    ));
    assert!(!source_window_is_current(window, 3, None, 7, project));
    assert!(matches!(
        project.session_cell(track, scene),
        Some(SessionCellContent::Audio(item)) if item.id() == clip
    ));
}
