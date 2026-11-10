//! The note-audition speaker is embedded and renders at normal display scales.
use gpui::AssetSource;
use hex_ui::assets::HexAssets;

#[test]
fn audition_speaker_vector_is_embedded_and_renders() {
    let bytes = HexAssets.load("icons/midi/audition.svg").unwrap().unwrap();
    let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default()).unwrap();
    assert_eq!((tree.size().width(), tree.size().height()), (16.0, 16.0));
    for scale in [1, 2] {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(16 * scale, 16 * scale).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
            &mut pixmap.as_mut(),
        );
        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] > 128));
        assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] == 0));
    }
}
