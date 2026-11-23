use super::super::audio_admission::{AudioAdmissionStatus, PausedRepublish, ProjectRevision};
use super::{ViewState, is_paused_republish_rejection, record_paused_republish_rejection};
use hex_engine::{PlaybackAction, PlaybackCommand, PlaybackHandoffRejection};
use std::sync::Arc;

#[test]
fn foreign_tail_source_paused_exhausts_paused_republish_and_clears_deferred_play() {
    assert!(is_paused_republish_rejection(
        PlaybackHandoffRejection::SessionRuntime
    ));
    assert!(is_paused_republish_rejection(
        PlaybackHandoffRejection::ForeignTailSourcePaused
    ));
    let project = ProjectRevision {
        generation: 2,
        revision: 110,
    };
    let state = ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::new(hex_project::Project::new(48_000).unwrap()),
        waveforms: Default::default(),
        source_overviews: Default::default(),
        audio_clip_waveforms: Default::default(),
        midi_preview: Default::default(),
        gain_preview: Ok(Default::default()),
        timeline_frames: 48_000,
        can_undo: false,
        can_redo: false,
        revision: project.revision,
        unsaved: false,
        document_path: None,
    };
    let mut admission = AudioAdmissionStatus::default();
    admission.canonical_revision(project);
    admission.published(project, 2506);
    admission.rejected(
        project,
        2506,
        PlaybackHandoffRejection::ForeignTailSourcePaused.message(),
    );

    for initial in [
        None,
        Some(PausedRepublish::Submitted {
            project,
            output: 2,
            rejected_audio_revision: 2505,
        }),
    ] {
        let mut paused_republish = initial;
        let mut deferred_play = Some((
            PlaybackAction::Transport(PlaybackCommand::Play),
            None,
            project.generation,
        ));
        let mut status = String::from("applying audio update at the paused cursor");
        let mut error = None;

        assert!(record_paused_republish_rejection(
            &mut paused_republish,
            &mut deferred_play,
            &mut status,
            &mut error,
            project.generation,
            Some(&state),
            &admission,
            project,
            Some(2),
            2506,
            PlaybackHandoffRejection::ForeignTailSourcePaused,
        ));
        assert_eq!(
            paused_republish,
            Some(PausedRepublish::Exhausted { project, output: 2 })
        );
        assert!(deferred_play.is_none());
        assert_eq!(status, "stop playback to apply edit");
        assert_eq!(
            error.as_deref(),
            Some(PlaybackHandoffRejection::ForeignTailSourcePaused.message())
        );
    }
}
