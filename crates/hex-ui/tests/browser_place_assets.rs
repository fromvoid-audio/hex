//! The same embedded catalog feeds both browser place rows and the icon grid.
use gpui::AssetSource;
use hex_ui::assets::HexAssets;
use std::collections::BTreeSet;

#[test]
fn browser_place_vectors_are_embedded_distinct_and_render_at_one_and_two_times() {
    let assets = HexAssets;
    let mut unique = BTreeSet::new();
    for id in [
        "folder",
        "samples",
        "devices",
        "projects",
        "collection",
        "waveform",
        "search",
    ] {
        let path = format!("icons/browser-place/{id}.svg");
        let bytes = assets.load(&path).unwrap().expect("known icon");
        assert!(unique.insert(bytes.to_vec()), "duplicate artwork for {id}");
        let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default()).unwrap();
        assert_eq!((tree.size().width(), tree.size().height()), (16.0, 16.0));
        for scale in [1, 2] {
            let mut pixmap = resvg::tiny_skia::Pixmap::new(16 * scale, 16 * scale).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
                &mut pixmap.as_mut(),
            );
            assert!(
                pixmap.data().chunks_exact(4).any(|pixel| pixel[3] > 128),
                "empty {id} at {scale}x"
            );
            assert!(
                pixmap.data().chunks_exact(4).any(|pixel| pixel[3] == 0),
                "opaque background for {id}"
            );
        }
    }
    assert!(
        assets
            .load("icons/browser-place/not-present.svg")
            .unwrap()
            .is_none()
    );
    assert_eq!(assets.list("icons/browser-place/").unwrap().len(), 7);
}
