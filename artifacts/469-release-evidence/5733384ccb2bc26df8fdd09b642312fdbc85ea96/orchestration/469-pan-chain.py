from pathlib import Path
p=Path('crates/hex-ui/src/editor/resize_pan_tests.rs');s=p.read_text();a=s.index('#[gpui::test]\nfn resize_edge_pan_stops_after_original_audio_handle_leaves_view');prefix=s[:a];t=s[a:];t=t.replace('check_edge_pan_after_original_handle_leaves_view(cx, false, false, false);','check_edge_pan_after_original_handle_leaves_view(cx, false, false, false, false);').replace('check_edge_pan_after_original_handle_leaves_view(cx, true, false, false);','check_edge_pan_after_original_handle_leaves_view(cx, true, false, false, false);').replace('check_edge_pan_after_original_handle_leaves_view(cx, false, true, false);','check_edge_pan_after_original_handle_leaves_view(cx, false, true, false, false);').replace('check_edge_pan_after_original_handle_leaves_view(cx, false, false, true);','check_edge_pan_after_original_handle_leaves_view(cx, false, false, true, false);');t=t.replace('''    enabled_warp: bool,
) {''','''    enabled_warp: bool,
    outside_release: bool,
) {''',1);t=t.replace('''        visual.simulate_mouse_up(center, MouseButton::Left, modifiers);''','''        let release = if outside_release {
            let ruler = visual.debug_bounds("arrangement-loop-rail-row").expect("actual ruler receiver");
            let release = point(center.x, ruler.center().y);
            visual.simulate_mouse_move(release, MouseButton::Left, modifiers);
            visual.run_until_parked();
            release
        } else { center };
        visual.simulate_mouse_up(release, MouseButton::Left, modifiers);''',1);t+='''

#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_audio_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, false, false, true);
}
#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_midi_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, true, false, false, true);
}
#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_shift_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, true, false, true);
}
''';p.write_text(prefix+t)
