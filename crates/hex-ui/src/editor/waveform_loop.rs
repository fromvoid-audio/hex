//! Bounded visual projection of accepted processing-root peaks through the
//! canonical placement query. This never prepares audio or owns a DSP clock.
use super::waveform::{WaveformGrid, WaveformPreview, waveform_canvas};
use super::waveform_geometry::{WaveformMapping, WaveformViewCache};
use gpui::Div;
use hex_project::{AudioViewResizeOperation, preview_audio_view_resize};
use std::{rc::Rc, sync::Arc};

mod accepted_root;
mod projection;
use accepted_root::{AcceptedLoopSourceRequest, accepted_loop_source};
pub(super) use projection::LoopProjection;

pub(super) fn loop_waveform_view(
    request: WaveformPreview<'_>,
    start: u64,
    end: u64,
    cache: Rc<WaveformViewCache>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    request.preview.audio_edit()?.placement?;
    let asset_frames = request.asset.map_or(
        request
            .original
            .source_offset()
            .checked_add(request.original.source_length())?,
        |source| source.source_frames() as u64,
    );
    let same_root = request
        .original
        .audio_edit()
        .zip(request.preview.audio_edit())
        .is_some_and(|(a, b)| Arc::ptr_eq(&a.domain, &b.domain));
    let captured_markers = request.original.audio_edit().is_none()
        && [
            hex_project::AudioViewProjection::new(request.preview, request.tempo)
                .ok()
                .and_then(|view| view.play_start_position().ok().flatten())
                .map(|position| AudioViewResizeOperation::PlayStart { position }),
            request
                .preview
                .audio_edit()
                .map(|edit| AudioViewResizeOperation::ScopeRange {
                    start: edit.view_start,
                    end: edit.view_end,
                    looping: edit
                        .placement
                        .is_some_and(|p| p.mode == hex_project::AudioViewMode::Repeat),
                }),
        ]
        .into_iter()
        .flatten()
        .any(|operation| {
            preview_audio_view_resize(request.original, request.tempo, asset_frames, operation)
                .ok()
                .as_ref()
                == Some(request.preview)
        });
    let mut retimed = false;
    if !same_root && !captured_markers {
        let endpoint = request
            .preview
            .start()
            .checked_add(request.preview.duration())?;
        let repeated = preview_audio_view_resize(
            request.original,
            request.tempo,
            asset_frames,
            AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: endpoint,
            },
        )
        .ok();
        if repeated.as_ref() != Some(request.preview) {
            let edge = if request.original.start() == request.preview.start() {
                hex_project::TrimEdge::Right
            } else {
                hex_project::TrimEdge::Left
            };
            let frame = if edge == hex_project::TrimEdge::Right {
                endpoint
            } else {
                request.preview.start()
            };
            let stretched = hex_project::preview_audio_clip_stretch(
                request.original,
                edge,
                frame,
                request.tempo,
                asset_frames,
            )
            .ok()?;
            if &stretched != request.preview {
                return None;
            }
            retimed = true;
        }
    }
    let source = accepted_loop_source(AcceptedLoopSourceRequest {
        original: request.original,
        preview: request.preview,
        tempo: request.tempo,
        asset: request.asset,
        prepared: request.prepared,
    })?;
    let projection = Arc::new(LoopProjection {
        original: request.original.clone(),
        preview: request.preview.clone(),
        tempo: request.tempo.clone(),
        prepared: source.prepared,
        accepted_origin: source.origin,
        retimed,
    });
    Some(waveform_canvas(
        source.waveform,
        WaveformMapping::Loop(projection),
        start,
        end,
        Some(cache),
        grid,
        request.ink,
    ))
}

#[cfg(all(test, feature = "ui-test-support"))]
mod marker_tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    use hex_session::{CachedPeak, CachedWaveform};

    #[gpui::test]
    fn fresh_audio_marker_preview_draws_shifted_root_peaks(_cx: &mut gpui::TestAppContext) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "marker peaks".into(),
                path: "never-opened.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        let original = editor.project().clip(clip).unwrap();
        let tempo = editor.project().tempo_map();
        let waveform = Arc::new(
            CachedWaveform::from_peaks(
                8,
                (0..8)
                    .map(|frame| {
                        [CachedPeak {
                            min: frame as f32 / 10.0,
                            max: frame as f32 / 10.0,
                        }; 2]
                    })
                    .collect(),
            )
            .unwrap(),
        );
        for operation in [
            AudioViewResizeOperation::PlayStart { position: 2 },
            AudioViewResizeOperation::ScopeRange {
                start: 2,
                end: 6,
                looping: true,
            },
        ] {
            let preview = preview_audio_view_resize(original, tempo, 8, operation).unwrap();
            assert!(
                loop_waveform_view(
                    WaveformPreview {
                        original,
                        preview: &preview,
                        tempo,
                        asset: Some(&waveform),
                        prepared: None,
                        ink: 0xffffff,
                    },
                    preview.start(),
                    preview.start() + preview.duration(),
                    Default::default(),
                    None
                )
                .is_some(),
                "first marker edit must draw a waveform without a prior prepared root"
            );
            let projection = LoopProjection {
                original: original.clone(),
                preview: preview.clone(),
                tempo: tempo.clone(),
                prepared: false,
                accepted_origin: 0,
                retimed: false,
            };
            assert_eq!(
                projection
                    .peaks_with_detail(&waveform, preview.start()..preview.start() + 1, 0)
                    .unwrap()[0]
                    .max,
                0.2
            );
            if matches!(operation, AudioViewResizeOperation::PlayStart { .. }) {
                assert!(
                    projection.peaks_with_detail(&waveform, 6..8, 0).is_none(),
                    "finite read leaves the fixed gate's tail silent"
                );
            }
        }
    }
}
