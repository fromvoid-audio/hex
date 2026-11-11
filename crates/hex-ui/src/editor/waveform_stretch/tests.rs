use super::*;
use hex_project::{Command, Created, Editor, Project};
use hex_session::{CachedPeak, CachedWaveform};

fn fixture() -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "peaks".into(),
            path: "original.wav".into(),
            sample_rate: 44_100,
            frames: 88_200,
        })
        .unwrap()
    else {
        panic!("import")
    };
    (editor, clip)
}
fn projection(
    editor: &Editor,
    id: hex_project::ClipId,
    edge: TrimEdge,
    frame: u64,
) -> StretchProjection {
    let original = editor.project().clip(id).unwrap().clone();
    let tempo = editor.project().tempo_map().clone();
    let preview = preview_audio_clip_stretch(&original, edge, frame, &tempo, 88_200).unwrap();
    StretchProjection {
        prepared: original.audio_edit().is_some(),
        original,
        preview,
        tempo,
    }
}

#[test]
fn geometry_cache_replaces_preview_snapshot_and_distinguishes_source_and_map() {
    use crate::editor::waveform_geometry::{WaveformGeometryRequest, cached_waveform_geometry};
    use gpui::{Bounds, point, px, size};
    let (editor, id) = fixture();
    let source = Arc::new(
        CachedWaveform::from_peaks(
            88_200,
            (0..128)
                .map(|i| {
                    [CachedPeak {
                        min: -(i as f32 / 128.0),
                        max: i as f32 / 128.0,
                    }; 2]
                })
                .collect(),
        )
        .unwrap(),
    );
    let cache = RefCell::new(WaveformGeometryCache::default());
    let mut previous = None;
    for length in 88_201..88_241 {
        let mapping =
            WaveformMapping::Stretch(Arc::new(projection(&editor, id, TrimEdge::Right, length)));
        let request = WaveformGeometryRequest {
            amplitude_scale: crate::editor::waveform::WaveformAmplitudeScale::from_decibels(false),
            mapping: &mapping,
            visible_start: 0,
            visible_end: length,
            bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(100.0))),
            scale: 1.0,
            columns: 200,
            grid: None,
        };
        let geometry = cached_waveform_geometry(&cache, &source, &request).unwrap();
        assert!(Arc::ptr_eq(
            &geometry,
            &cached_waveform_geometry(&cache, &source, &request).unwrap()
        ));
        if let Some(old) = previous {
            assert!(!Arc::ptr_eq(&geometry, &old));
        }
        previous = Some(geometry);
        assert_eq!(
            cache.borrow().entry_count(),
            1,
            "one metadata snapshot, not a motion history"
        );
    }
}
