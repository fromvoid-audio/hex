//! Isolated rendered check of the recorder's bundled-font controls.
use gpui::TestAppContext;

#[gpui::test]
fn recorder_header_fits_the_button_labels_and_hit_targets(cx: &mut TestAppContext) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-recorder-layout-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    hex_ui::exercise_sample_recorder_layout(cx, root.clone());
    std::fs::remove_dir_all(root).unwrap();
}
