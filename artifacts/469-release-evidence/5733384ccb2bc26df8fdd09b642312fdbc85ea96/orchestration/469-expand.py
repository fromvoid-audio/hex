from pathlib import Path
p=Path('crates/hex-ui/src/editor/resize_release_tests.rs');s=p.read_text();a=s.index('fn review_other_track_release(');b=s.index('#[gpui::test]\nfn browser_release_review_midi_edge_admits',a);t=s[a:b]
t=t.replace('fn review_other_track_release(cx: &mut TestAppContext, midi: bool, outside: bool) {','''fn review_other_track_release(cx: &mut TestAppContext, midi: bool, outside: bool) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        captured_release_case(cx, midi, edge, false, false, if outside { 1 } else { 0 });
    }
}

fn captured_release_case(
    cx: &mut TestAppContext,
    midi: bool,
    edge: TrimEdge,
    stretch: bool,
    enabled_warp: bool,
    destination: u8,
) {
    let outside = destination != 0;''')
t=t.replace('''    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {''','''    if enabled_warp {
        let settings = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            hex_project::default_audio_warp(p.clip(clip).unwrap(), p.tempo_map(), hex_project::WarpMode::Beats {
                preserve: hex_project::BeatsPreserve::Transients,
                tail: hex_project::BeatsTail::Forward,
                decay: 100,
            }).unwrap()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp { clip, settings: Some(settings) }))));
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {''',1)
t=t.replace('format!("{prefix}-trim-right-{}", clip.value())','format!("{prefix}-trim-{}-{}", if edge == TrimEdge::Left { "left" } else { "right" }, clip.value())')
t=t.replace('    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());','''    let modifiers = Modifiers { shift: stretch, ..Modifiers::default() };
    let direction = if edge == TrimEdge::Left { 1.0 } else { -1.0 };
    visual.simulate_mouse_down(down, MouseButton::Left, modifiers);''')
t=t.replace('down.x - px(12.0)','down.x + px(12.0 * direction)').replace('down.x - px(40.0)','down.x + px(40.0 * direction)')
t=t.replace('Modifiers::default(),\n    );','modifiers,\n    );')
t=t.replace('''    let release = if outside {''','''    let release = if destination >= 2 {
        let selector = match destination {
            2 => Box::leak(format!("arrangement-track-header-{}", other.value()).into_boxed_str()),
            3 => "arrangement-loop-rail-row",
            4 => "editor-status-bar",
            5 => "browser-audition-waveform",
            _ => unreachable!("private fixture destination"),
        };
        let bounds = visual.debug_bounds(selector).expect("actual rendered receiver");
        if matches!(destination, 3 | 4) {
            point(down.x + px(40.0 * direction), bounds.center().y)
        } else {
            bounds.center()
        }
    } else if outside {''')
t=t.replace('''        assert_eq!(target, if outside { None } else { Some(other) });''','''        if destination <= 1 {
            assert_eq!(target, if outside { None } else { Some(other) });
        }''')
t=t.replace('visual.simulate_mouse_move(release, MouseButton::Left, Modifiers::default());','visual.simulate_mouse_move(release, MouseButton::Left, modifiers);').replace('visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());','visual.simulate_mouse_up(release, MouseButton::Left, modifiers);')
t=t.replace('''assert!(matches!(v.last_enqueued_edit, Some(Command::TrimClipEdge { clip: id, .. }) if id == clip));''','''assert!(if stretch {
                matches!(v.last_enqueued_edit, Some(Command::StretchAudioClipEdge { clip: id, edge: actual, .. }) if id == clip && actual == edge)
            } else {
                matches!(v.last_enqueued_edit, Some(Command::TrimClipEdge { clip: id, edge: actual, .. }) if id == clip && actual == edge)
            });''')
s=s[:a]+t+s[b:]
s+='''

#[gpui::test]
fn captured_release_audio_over_header_ruler_status_and_browser(cx: &mut TestAppContext) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for destination in 2..=5 { captured_release_case(cx, false, edge, false, false, destination); }
    }
}
#[gpui::test]
fn captured_release_midi_over_header_ruler_status_and_browser(cx: &mut TestAppContext) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for destination in 2..=5 { captured_release_case(cx, true, edge, false, false, destination); }
    }
}
#[gpui::test]
fn captured_release_shift_over_other_track_and_foreign_panels(cx: &mut TestAppContext) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for destination in 0..=5 { captured_release_case(cx, false, edge, true, false, destination); }
    }
}
#[gpui::test]
fn captured_release_enabled_warp_over_other_track_and_foreign_panels(cx: &mut TestAppContext) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for destination in 0..=5 { captured_release_case(cx, false, edge, false, true, destination); }
    }
}
'''
p.write_text(s)
