from pathlib import Path
p=Path('crates/hex-ui/src/editor/tracks.rs');s=p.read_text();a=s.index('        let header_content = if live_volume');b=s.index('        } else {\n            let track_controls =',a);seg=s[a:b];seg=seg.replace('let utilities = div()\n                .flex()','let utilities = div()\n                .on_click(|_, _, cx| cx.stop_propagation())\n                .flex()',1);seg=seg.replace('                .on_click(|_, _, cx| cx.stop_propagation())\n                .child(', '                .child(',1);seg=seg.replace('div()\n                        .w(px(122.0))','div()\n                        .on_click(|_, _, cx| cx.stop_propagation())\n                        .w(px(122.0))',1);s=s[:a]+seg+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/return_tracks.rs');s=p.read_text();old=__import__('subprocess').check_output(['git','show','HEAD:crates/hex-ui/src/editor/return_tracks.rs'],text=True);a=old.index('                .child(\n                    return_button(\n                        format!("return-gain-down-');b=old.index('                .child(\n                    return_button(\n                        format!("return-mute-',a);oldcontrols=old[a:b];idx=s.index('            let row = div()',s.index('for track in returns'));s=s[:idx]+'''            let gain = track.gain();
            let gain_control = if self.document_audio_mode == hex_session::DocumentAudioMode::Resident {
                div().w(px(82.0)).flex().flex_shrink_0().child(self.return_volume(track, cx))
            } else {
                // Preserve the real explicit command path when held previews are unavailable.
                div().flex().items_center().gap(px(3.0))
'''+oldcontrols+'''            };
'''+s[idx:];oldnew='''                .child(
                    div()
                        .w(px(82.0))
                        .flex_shrink_0()
                        .child(self.return_volume(&track, cx)),
                )''';assert oldnew in s;s=s.replace(oldnew,'                .child(gain_control)',1);p.write_text(s)
p=Path('crates/hex-ui/src/editor/track_gain/tests.rs');s=p.read_text();a=s.index('fn header_volume_is_one_filled_numeric_field_not_a_separate_rail');i=s.index('    let (_browser',a);s=s[:i]+'''    #[cfg(target_os = "macos")]
    {
        cx.use_native_text_system().unwrap();
        cx.update(|app| crate::theme::load_fonts(app).unwrap());
    }
'''+s[i:];s=s.replace('    assert!(field.size.height >= px(16.0));\n}', '''    assert!(field.size.height >= px(16.0));
    let header = self::bounds(&mut visual, "arrangement-track-header", track);
    let pan = self::bounds(&mut visual, "track-pan", track);
    let activator = self::bounds(&mut visual, "track-mute", track);
    let solo = self::bounds(&mut visual, "track-solo", track);
    let arm = self::bounds(&mut visual, "track-arm-unavailable", track);
    assert_eq!(activator.top(), solo.top());
    assert_eq!(solo.top(), arm.top());
    assert_eq!(whole.top(), pan.top());
    assert!((activator.left() - whole.left()).abs() <= px(1.0));
    assert!(activator.bottom() <= whole.top());
    assert_eq!(arm.right(), pan.right());
    for rect in [whole, pan, activator, solo, arm] {
        assert!(rect.left() >= header.left() && rect.right() <= header.right());
        assert!(rect.top() >= header.top() && rect.bottom() <= header.bottom());
    }
    let mut scene = Vec::new();
    for suffix in ["track-mute", "track-solo", "track-arm-unavailable", "track-volume", "track-pan"] {
        let id = Box::leak(format!("{suffix}-{}", track.value()).into_boxed_str());
        let rect = visual.debug_bounds(id).unwrap();
        scene.push(serde_json::json!({"id": id, "bounds": [f32::from(rect.left()), f32::from(rect.top()), f32::from(rect.size.width), f32::from(rect.size.height)], "background": visual.rendered_background(id)}));
    }
    let activator_id = Box::leak(format!("track-mute-{}", track.value()).into_boxed_str());
    let solo_id = Box::leak(format!("track-solo-{}", track.value()).into_boxed_str());
    assert_eq!(visual.rendered_background(activator_id), Some(gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into()));
    assert_ne!(visual.rendered_background(activator_id), visual.rendered_background(solo_id));
    println!("HEX439_RENDERED_SCENE {}", serde_json::to_string(&scene).unwrap());
}''',1)
# Add an ordinary name click after the existing control/non-selection oracle.
needle='''    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("track-arm-unavailable-{}", track.value()).into_boxed_str()
            ))
            .is_some()
    );'''
assert needle in s;s=s.replace(needle,needle+'''
    let name = bounds(&mut visual, "track-name", track).center();
    visual.simulate_mouse_down(name, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(name, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| assert_eq!(view.selected_track, Some(track)));
''',1);p.write_text(s)
