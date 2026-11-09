//! Transport controls share the embedded, editable vector catalog.
use gpui::AssetSource;
use hex_ui::assets::HexAssets;
use std::collections::BTreeSet;

#[test]
fn transport_vectors_decode_and_render_at_one_and_two_times() {
    let assets = HexAssets;
    let mut unique = BTreeSet::new();
    for id in [
        "play",
        "pause",
        "stop",
        "split",
        "undo",
        "redo",
        "add-track",
        "add-midi-clip",
        "remove-track",
    ] {
        let path = format!("icons/transport/{id}.svg");
        let bytes = assets.load(&path).unwrap().expect("known transport icon");
        assert!(unique.insert(bytes.to_vec()));
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
    assert_eq!(assets.list("icons/transport/").unwrap().len(), 9);
}
