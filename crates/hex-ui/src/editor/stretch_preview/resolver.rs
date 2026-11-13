//! Pure, capture-local request caching and bounded optional precision search.
use hex_project::Clip;
use std::time::{Duration, Instant};

// Precision only: never limits whether a requested endpoint is admitted.
const OPTIONAL_PRECISION_TIME: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug)]
pub(super) struct ResolutionRequest {
    pub(super) frame: u64,
    pub(super) stretch: bool,
}

#[derive(Clone, Debug)]
pub(super) struct AcceptedPreview {
    pub(super) clip: Clip,
    pub(super) command_frame: u64,
    pub(super) stretch: bool,
}

#[derive(Default, Debug)]
pub(in crate::editor) struct Resolution {
    requested: Option<(u64, bool)>,
    accepted: Option<AcceptedPreview>,
    #[cfg(test)]
    pub(in crate::editor) point_checks: usize,
}

pub(super) fn resolve(
    resolution: &mut Resolution,
    request: ResolutionRequest,
    original_edge_frame: impl FnOnce() -> Option<u64>,
    mut admit: impl FnMut(u64) -> Option<Clip>,
) -> Option<AcceptedPreview> {
    if resolution.requested == Some((request.frame, request.stretch)) {
        return resolution.accepted.clone();
    }
    let started = Instant::now();
    #[cfg(test)]
    {
        resolution.point_checks += 1;
    }
    if let Some(candidate) = admitted(&mut admit, request.frame, request.stretch) {
        resolution.requested = Some((request.frame, request.stretch));
        resolution.accepted = Some(candidate.clone());
        return Some(candidate);
    }
    if !request.stretch {
        resolution.requested = Some((request.frame, request.stretch));
        resolution.accepted = None;
        return None;
    }
    // A modifier change must not reuse an ordinary-trim candidate as a stretch
    // command. The captured original is an exact no-op fallback in the new mode.
    let mut accepted = resolution
        .accepted
        .clone()
        .filter(|old| old.stretch == request.stretch);
    if accepted.is_none() {
        let original_edge_frame = original_edge_frame()?;
        accepted = admitted(&mut admit, original_edge_frame, request.stretch);
        #[cfg(test)]
        {
            resolution.point_checks += 1;
        }
    }
    if let Some(candidate) = &mut accepted {
        let mut invalid = request.frame;
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
            if let Some(next) = admitted(&mut admit, middle, request.stretch) {
                *candidate = next;
            } else {
                invalid = middle;
            }
        }
    }
    resolution.requested = Some((request.frame, request.stretch));
    resolution.accepted = accepted.clone();
    accepted
}

fn admitted(
    admit: &mut impl FnMut(u64) -> Option<Clip>,
    frame: u64,
    stretch: bool,
) -> Option<AcceptedPreview> {
    Some(AcceptedPreview {
        clip: admit(frame)?,
        command_frame: frame,
        stretch,
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{
        BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, Tempo, TrimEdge,
        WarpMode, default_audio_warp, preview_audio_clip_stretch,
    };

    fn admit(project: &Project, original: &Clip, frame: u64) -> Option<Clip> {
        let clip = preview_audio_clip_stretch(
            original,
            TrimEdge::Right,
            frame,
            project.tempo_map(),
            8_000,
        )
        .ok()?;
        hex_engine::audio_transform::validate_clip_warp_preview(&clip, project.tempo_map()).ok()?;
        Some(clip)
    }

    #[test]
    fn distant_valid_island_is_accepted_and_identical_requests_do_no_metadata_work() {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
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
        let original = editor.project().clip(clip).unwrap().clone();
        let project = editor.project();
        assert!(admit(project, &original, 32_000).is_none());
        assert!(admit(project, &original, 34_200).is_some());

        let mut resolution = Resolution::default();
        let request = |frame| ResolutionRequest {
            frame,
            stretch: true,
        };
        let held = resolve(
            &mut resolution,
            request(32_000),
            || original.start().checked_add(original.duration()),
            |frame| admit(project, &original, frame),
        )
        .unwrap();
        assert_ne!(held.command_frame, 32_000);
        assert!(
            hex_engine::audio_transform::validate_clip_warp_preview(
                &held.clip,
                project.tempo_map()
            )
            .is_ok()
        );
        let before = resolution.point_checks;
        assert_eq!(
            resolve(
                &mut resolution,
                request(32_000),
                || original.start().checked_add(original.duration()),
                |frame| admit(project, &original, frame),
            )
            .unwrap()
            .clip,
            held.clip
        );
        assert_eq!(resolution.point_checks, before);
        let distant = resolve(
            &mut resolution,
            request(34_200),
            || original.start().checked_add(original.duration()),
            |frame| admit(project, &original, frame),
        )
        .unwrap();
        assert_eq!(
            distant.command_frame, 34_200,
            "no connected-component restriction"
        );
        assert_eq!(resolution.point_checks, before + 1);
        let reversed = resolve(
            &mut resolution,
            request(3_200),
            || original.start().checked_add(original.duration()),
            |frame| admit(project, &original, frame),
        )
        .unwrap();
        assert_eq!(reversed.command_frame, 3_200);
        assert_eq!(&reversed.clip, &original);
        assert!(
            resolution.point_checks <= 12,
            "one invalid target has at most8 precision probes plus initial fallback"
        );
    }
    #[test]
    fn mode_change_does_not_reuse_trim_and_missing_fallback_does_not_cache() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 128,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let original = editor.project().clip(clip).unwrap().clone();
        let mut resolution = Resolution::default();
        let ordinary = ResolutionRequest {
            frame: 1,
            stretch: false,
        };
        let stretch = ResolutionRequest {
            frame: 2,
            stretch: true,
        };
        assert!(
            !resolve(
                &mut resolution,
                ordinary,
                || panic!("direct admission"),
                |_| Some(original.clone())
            )
            .unwrap()
            .stretch
        );
        assert!(resolve(&mut resolution, stretch, || None, |_| None).is_none());
        let admitted = resolve(
            &mut resolution,
            stretch,
            || panic!("direct admission"),
            |_| Some(original.clone()),
        )
        .unwrap();
        assert!(admitted.stretch);
        assert_eq!(admitted.command_frame, 2);
        let cached = resolve(
            &mut resolution,
            stretch,
            || panic!("cached request"),
            |_| panic!("cached request"),
        )
        .unwrap();
        assert_eq!(cached.clip, admitted.clip);
    }
}
