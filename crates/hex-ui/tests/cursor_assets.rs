//! Editable app cursor vectors consumed by the existing macOS GPUI cursor owner.

#[test]
fn cursor_vectors_rasterize_with_visible_contrast_at_one_and_two_times() {
    let assets: [(&str, &[u8]); 3] = [
        (
            "left bracket",
            include_bytes!("../../../assets/cursors/resize-left-bracket.svg"),
        ),
        (
            "right bracket",
            include_bytes!("../../../assets/cursors/resize-right-bracket.svg"),
        ),
        ("loop", include_bytes!("../../../assets/cursors/loop.svg")),
    ];
    for (name, svg) in assets {
        let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).unwrap();
        assert_eq!(
            (tree.size().width(), tree.size().height()),
            (24.0, 24.0),
            "{name}"
        );
        for scale in [1, 2] {
            let mut image = resvg::tiny_skia::Pixmap::new(24 * scale, 24 * scale).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32),
                &mut image.as_mut(),
            );
            let png = image.encode_png().unwrap();
            let decoded = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
            assert_eq!(
                (decoded.width(), decoded.height()),
                (24 * scale, 24 * scale),
                "{name}"
            );
            let pixels = decoded.data().chunks_exact(4);
            assert!(
                pixels.clone().any(|p| p[3] > 200 && p[0] > 200),
                "{name} white outline"
            );
            assert!(
                pixels.clone().any(|p| p[3] > 200 && p[0] < 30),
                "{name} dark stroke"
            );
            assert!(
                pixels.clone().any(|p| p[3] == 0),
                "{name} transparent background"
            );
        }
    }
}
