//! Select accepted audio roots for directed waveform loop previews.
use hex_project::{Clip, TempoMap};
use hex_session::{CachedWaveform, service::AudioClipWaveform};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) struct AcceptedLoopSourceRequest<'a> {
    pub(super) original: &'a Clip,
    pub(super) preview: &'a Clip,
    pub(super) tempo: &'a TempoMap,
    pub(super) asset: Option<&'a Arc<CachedWaveform>>,
    pub(super) prepared: Option<&'a AudioClipWaveform>,
}

pub(super) struct AcceptedLoopSource {
    pub(super) waveform: Arc<CachedWaveform>,
    pub(super) origin: i128,
    pub(super) prepared: bool,
}

/// A directed outer waveform already contains reflection. Loop gestures must
/// sample its explicitly accepted forward processing root instead of mapping
/// those finite output peaks as if they were untouched source content.
pub(super) fn accepted_loop_source(
    request: AcceptedLoopSourceRequest<'_>,
) -> Option<AcceptedLoopSource> {
    let (waveform, origin, prepared) = if let Some(old) = request.original.audio_edit() {
        let preview = request.preview.audio_edit()?;
        // Accepted root peaks already include the forward container fade.
        // Reflecting them would move that fade to the wrong side. Until a
        // destination-fade-aware preview exists, keep real resize geometry
        // and wait for the admitted candidate's waveform instead.
        if preview.content_view.is_some()
            && (!old.domain.fade.is_neutral() || !preview.domain.fade.is_neutral())
        {
            return None;
        }
        let accepted = request.prepared?;
        if old.placement.is_some() || old.content_view.is_some() {
            let root = accepted.preview_root.as_ref()?;
            (Arc::clone(&root.waveform), root.origin, true)
        } else {
            let (_, length, offset, _) = old.projected_domain(request.tempo).ok()?;
            if accepted.offset != offset
                || accepted.length != request.original.duration()
                || accepted.waveform.source_frames() as u64 != length
            {
                return None;
            }
            (
                Arc::clone(&accepted.waveform),
                if old.reference_start_raw().is_some() {
                    old.processing_origin().ok()?
                } else {
                    0
                },
                true,
            )
        }
    } else {
        (Arc::clone(request.asset?), 0, false)
    };
    Some(AcceptedLoopSource {
        waveform,
        origin,
        prepared,
    })
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::super::projection::LoopProjection;
    use super::{AcceptedLoopSourceRequest, accepted_loop_source};
    use hex_project::{
        AudioViewResizeOperation, Command, Created, Editor, Project, TrimEdge,
        preview_audio_view_resize,
    };
    use hex_session::{CachedPeak, CachedWaveform};
    use std::sync::Arc;

    fn directed_fixture() -> (Editor, hex_project::ClipId, Arc<CachedWaveform>) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        // Capture a retained full root before trimming; peaks below represent
        // the accepted forward processing root, including its hidden prefix.
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 2,
            })
            .unwrap();
        let waveform = Arc::new(
            CachedWaveform::from_peaks(
                8,
                (0..8)
                    .map(|n| {
                        [CachedPeak {
                            min: n as f32 / 10.0,
                            max: n as f32 / 10.0,
                        }; 2]
                    })
                    .collect(),
            )
            .unwrap(),
        );
        (editor, clip, waveform)
    }

    #[test]
    fn first_directed_repeat_preview_selects_explicit_forward_root() {
        let (mut editor, clip, root) = directed_fixture();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let original = editor.project().clip(clip).unwrap();
        assert!(original.audio_edit().unwrap().placement.is_none());
        let tempo = editor.project().tempo_map();
        let preview = preview_audio_view_resize(
            original,
            tempo,
            8,
            AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
        )
        .unwrap();
        // Accepted outer PCM is already reversed and cropped. It must never be
        // relabeled as the full forward processing root during the first loop.
        let outer = Arc::new(
            CachedWaveform::from_peaks(
                6,
                vec![
                    [CachedPeak {
                        min: -1.0,
                        max: -1.0
                    }; 2];
                    6
                ],
            )
            .unwrap(),
        );
        let accepted = hex_session::service::AudioClipWaveform {
            waveform: outer,
            offset: 0,
            length: original.duration(),
            preview_root: Some(hex_session::service::AudioPreviewWaveform {
                waveform: Arc::clone(&root),
                origin: 0,
            }),
        };
        let request = AcceptedLoopSourceRequest {
            original,
            preview: &preview,
            tempo,
            asset: None,
            prepared: Some(&accepted),
        };
        let chosen = accepted_loop_source(request).unwrap();
        assert!(Arc::ptr_eq(&chosen.waveform, &root));
        let projection = LoopProjection {
            original: original.clone(),
            preview: preview.clone(),
            tempo: tempo.clone(),
            prepared: chosen.prepared,
            accepted_origin: chosen.origin,
            retimed: false,
        };
        assert_eq!(
            projection.peaks(&chosen.waveform, 2..3).unwrap()[0].max,
            0.7
        );
        let missing = hex_session::service::AudioClipWaveform {
            preview_root: None,
            ..accepted.clone()
        };
        assert!(
            accepted_loop_source(AcceptedLoopSourceRequest {
                preview: &projection.preview,
                prepared: Some(&missing),
                ..request
            })
            .is_none()
        );
    }

    #[test]
    fn directed_loop_preview_refuses_to_reflect_an_asymmetric_container_fade() {
        let (mut editor, clip, _) = directed_fixture();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 4,
                fade_out: 0,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let original = editor.project().clip(clip).unwrap();
        let preview = preview_audio_view_resize(
            original,
            editor.project().tempo_map(),
            8,
            AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
        )
        .unwrap();
        // A constant source with a left fade has these forward root peaks.
        // Reflecting them would incorrectly put the fade on the right.
        let root = Arc::new(
            CachedWaveform::from_peaks(
                8,
                [0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0]
                    .into_iter()
                    .map(|value| {
                        [CachedPeak {
                            min: value,
                            max: value,
                        }; 2]
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let accepted = hex_session::service::AudioClipWaveform {
            waveform: Arc::clone(&root),
            offset: 0,
            length: original.duration(),
            preview_root: Some(hex_session::service::AudioPreviewWaveform {
                waveform: root,
                origin: 0,
            }),
        };
        assert!(
            accepted_loop_source(AcceptedLoopSourceRequest {
                original,
                preview: &preview,
                tempo: editor.project().tempo_map(),
                asset: None,
                prepared: Some(&accepted),
            })
            .is_none(),
            "post-fade root peaks cannot represent destination-faded reversed output"
        );
    }
}
