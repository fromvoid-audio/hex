//! Capture-local point admission. Invalid requested points never become display
//! or commands; this deliberately makes no monotonic/nearest-boundary claim.
use super::{TrimDrag, TrimPreviewState, edge_frame, preview_clip};
use hex_project::Project;
use std::time::{Duration, Instant};

// Precision only: never limits whether a requested endpoint is admitted.
const OPTIONAL_PRECISION_TIME: Duration = Duration::from_millis(1);

#[derive(Default, Debug)]
pub(super) struct Resolution {
    requested: Option<(u64, bool)>,
    accepted: Option<TrimPreviewState>,
    #[cfg(test)]
    pub(super) point_checks: usize,
}

pub(super) fn resolve(drag: &TrimDrag, project: &Project, frame: u64) -> Option<TrimPreviewState> {
    let stretch = drag.stretch_modifier.get();
    let mut resolution = drag.resolution.borrow_mut();
    if resolution.requested == Some((frame, stretch)) {
        return resolution.accepted.clone();
    }
    let started = Instant::now();
    #[cfg(test)]
    {
        resolution.point_checks += 1;
    }
    if let Some(candidate) = admitted(drag, project, frame) {
        resolution.requested = Some((frame, stretch));
        resolution.accepted = Some(candidate.clone());
        return Some(candidate);
    }
    if !stretch {
        resolution.requested = Some((frame, stretch));
        resolution.accepted = None;
        return None;
    }
    // A modifier change must not reuse an ordinary-trim candidate as a stretch
    // command. The captured original is an exact no-op fallback in the new mode.
    let mut accepted = resolution
        .accepted
        .clone()
        .filter(|old| old.stretch == stretch);
    if accepted.is_none() {
        accepted = admitted(drag, project, edge_frame(&drag.clip, drag.edge)?);
        #[cfg(test)]
        {
            resolution.point_checks += 1;
        }
    }
    if let Some(candidate) = &mut accepted {
        let mut invalid = frame;
        // Precision only for an already unsupported target. Every retained
        // point is canonically validated; valid far targets above bypass this.
        for _ in 0..8 {
            if started.elapsed() >= OPTIONAL_PRECISION_TIME {
                break;
            }
            let valid = candidate.command_frame;
            if valid.abs_diff(invalid) <= 1 {
                break;
            }
            let middle = valid.min(invalid) + valid.abs_diff(invalid) / 2;
            #[cfg(test)]
            {
                resolution.point_checks += 1;
            }
            if let Some(next) = admitted(drag, project, middle) {
                *candidate = next;
            } else {
                invalid = middle;
            }
        }
    }
    resolution.requested = Some((frame, stretch));
    resolution.accepted = accepted.clone();
    accepted
}

fn admitted(drag: &TrimDrag, project: &Project, frame: u64) -> Option<TrimPreviewState> {
    let clip = preview_clip(drag, project, frame).ok()?;
    let stretch = drag.stretch_modifier.get();
    if stretch {
        hex_engine::audio_transform::validate_clip_warp_preview(&clip, project.tempo_map()).ok()?;
    }
    Some(TrimPreviewState {
        clip,
        command_frame: frame,
        stretch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{
        BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, Tempo, TrimEdge,
        WarpMode, default_audio_warp,
    };

    #[test]
    fn distant_valid_island_is_accepted_and_identical_requests_do_no_metadata_work() {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, track, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "islands".into(),
                path: "islands.wav".into(),
                sample_rate: 16_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("import")
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
        settings.markers.insert(
            1,
            hex_project::WarpMarker {
                source_frame: 7_600,
                beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            },
        );
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(30_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ONE,
                tempo: Tempo::new(480_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::StretchAudioClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: 3_200,
            })
            .unwrap();
        let mut drag = super::super::tests::trim_drag(
            track,
            editor.project().clip(clip).unwrap().clone(),
            8_000,
        );
        drag.edge = TrimEdge::Right;
        drag.stretch_modifier.set(true);
        let project = editor.project();
        assert!(admitted(&drag, project, 32_000).is_none());
        assert!(admitted(&drag, project, 34_200).is_some());
        let held = resolve(&drag, project, 32_000).unwrap();
        assert_ne!(held.command_frame, 32_000);
        assert!(
            hex_engine::audio_transform::validate_clip_warp_preview(
                &held.clip,
                project.tempo_map()
            )
            .is_ok()
        );
        let before = drag.resolution.borrow().point_checks;
        assert_eq!(resolve(&drag, project, 32_000).unwrap().clip, held.clip);
        assert_eq!(drag.resolution.borrow().point_checks, before);
        let distant = resolve(&drag, project, 34_200).unwrap();
        assert_eq!(
            distant.command_frame, 34_200,
            "no connected-component restriction"
        );
        assert_eq!(drag.resolution.borrow().point_checks, before + 1);
        let reversed = resolve(&drag, project, 3_200).unwrap();
        assert_eq!(reversed.command_frame, 3_200);
        assert_eq!(&reversed.clip, &drag.clip);
        assert!(
            drag.resolution.borrow().point_checks <= 12,
            "one invalid target has at most8 precision probes plus initial fallback"
        );
    }
}
