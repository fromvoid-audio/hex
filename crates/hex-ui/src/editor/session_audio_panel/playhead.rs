//! Session audio playhead position within the displayed source window.
use hex_project::SessionAudioClip;

/// Fraction of the displayed `source_start..source_end` window heard at
/// `performance_frame` for a cell launched at `launch_frame`, both on the
/// performance clock in project frames. None before the launch, after a
/// one-shot ends, or where a loop cycle starts before the displayed window.
pub(super) fn playhead_fraction(
    clip: &SessionAudioClip,
    launch_frame: u64,
    performance_frame: u64,
) -> Option<f32> {
    let elapsed = performance_frame.checked_sub(launch_frame)?;
    let frame = clip.source_frame_at(elapsed)?;
    let span = clip
        .source_end()
        .checked_sub(clip.source_start())
        .filter(|span| *span > 0)?;
    let offset = frame
        .checked_sub(clip.source_start())
        .filter(|offset| *offset < span)?;
    Some((offset as f64 / span as f64) as f32)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn session_audio_cell(source_start: u64, source_end: u64) -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Ok(Created::Track(track)) = editor.execute(Command::AddTrack {
            name: "audio".into(),
        }) else {
            panic!("track")
        };
        let Ok(Created::Scene(scene)) = editor.execute(Command::AddScene {
            name: "scene".into(),
        }) else {
            panic!("scene")
        };
        let Ok(Created::Asset(asset)) = editor.execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 1_000,
        }) else {
            panic!("asset")
        };
        let Ok(Created::SessionClip(clip)) = editor.execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "audio".into(),
            asset,
            source_start,
            source_end,
        }) else {
            panic!("Session clip")
        };
        (editor, clip)
    }

    fn cell(editor: &Editor, clip: hex_project::ClipId) -> &hex_project::SessionAudioClip {
        editor
            .project()
            .session_audio_clip(clip)
            .expect("Session audio cell")
    }

    #[test]
    fn playhead_follows_source_offset_from_the_accepted_launch() {
        let (editor, clip) = session_audio_cell(100, 300);
        let clip = cell(&editor, clip);
        // Launched at performance frame 5_000: elapsed 0 is the offset start.
        assert_eq!(playhead_fraction(clip, 5_000, 5_000), Some(0.0));
        assert_eq!(playhead_fraction(clip, 5_000, 5_050), Some(0.25));
        assert_eq!(
            playhead_fraction(clip, 5_000, 5_199),
            Some((199.0_f64 / 200.0) as f32)
        );
    }

    #[test]
    fn playhead_hides_before_launch_and_after_a_one_shot_ends() {
        let (editor, clip) = session_audio_cell(100, 300);
        let clip = cell(&editor, clip);
        assert_eq!(playhead_fraction(clip, 5_000, 4_999), None);
        assert_eq!(playhead_fraction(clip, 5_000, 5_200), None);
        assert_eq!(playhead_fraction(clip, 5_000, u64::MAX), None);
    }

    #[test]
    fn looped_playhead_wraps_inside_the_cycle_and_hides_outside_the_window() {
        let (mut editor, id) = session_audio_cell(100, 300);
        editor
            .execute(Command::SetSessionAudioClipLoop {
                clip: id,
                loop_range: Some(hex_project::SessionAudioLoop::new(150, 250).expect("loop")),
            })
            .expect("loop");
        let clip = cell(&editor, id);
        // Intro 100..250 (150 frames), then the 150..250 cycle repeats.
        assert_eq!(playhead_fraction(clip, 0, 100), Some(0.5));
        assert_eq!(playhead_fraction(clip, 0, 150), Some(0.25));
        assert_eq!(playhead_fraction(clip, 0, 250), Some(0.25));
        assert_eq!(
            playhead_fraction(clip, 0, 1_000_075),
            playhead_fraction(clip, 0, 175)
        );
        // A cycle that starts before the displayed source start has no line there.
        editor
            .execute(Command::SetSessionAudioClipLoop {
                clip: id,
                loop_range: Some(hex_project::SessionAudioLoop::new(50, 250).expect("loop")),
            })
            .expect("loop");
        let clip = cell(&editor, id);
        assert_eq!(playhead_fraction(clip, 0, 150), None);
        assert_eq!(playhead_fraction(clip, 0, 200), Some(0.0));
    }
}
