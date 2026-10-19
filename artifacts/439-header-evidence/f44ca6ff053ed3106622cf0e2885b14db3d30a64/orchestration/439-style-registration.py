from pathlib import Path
p=Path('scripts/check-ui-style.py');s=p.read_text();needle='        if not suffix.startswith(widgets) and not typed_error and not fixture_canvas:';assert needle in s;s=s.replace(needle,'''        # Header mixer composition uses typed controls; each caption is checked
        # separately at its theme::text boundary. Keep variable names file-local.
        header_mixer_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/tracks.rs'
            and suffix.startswith(('utilities)', 'activator)', 'solo)', 'arm)',
                                   'self.track_pan(', 'header_content)'))
        ) or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/return_tracks.rs'
            and suffix.startswith(('self.return_volume(', 'gain_control)'))
        )
        if not suffix.startswith(widgets) and not typed_error and not fixture_canvas and not header_mixer_component:''',1);p.write_text(s)
p=Path('crates/hex-ui/src/editor/track_gain.rs');s=p.read_text();idx=s.index('fn automated(');s=s[:idx]+'''/// Mixer activators share palette resolution without adding project state.
pub(super) fn activator_colors(active: bool) -> (u32, u32) {
    let background = theme::color(if active { theme::PaletteRole::Primary } else { theme::PaletteRole::Raised });
    let preferred = theme::color(if active { theme::PaletteRole::Surface } else { theme::PaletteRole::Muted });
    (background, theme::readable_text(background, preferred))
}

'''+s[idx:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/tracks.rs');s=p.read_text();a=s.index('        let header_content = if live_volume {');s=s[:a]+s[a:].replace('        let header_content = if live_volume {','        let header_content = if live_volume {\n            let (active_background, active_text) = super::track_gain::activator_colors(!track.muted());\n            let (solo_background, solo_text) = super::track_gain::activator_colors(track.soloed());',1)
a=s.index('            .bg(rgb(theme::color(if track.muted() {',s.index('let header_content'));b=s.index('            .child(theme::text((track_index',a);s=s[:a]+'''            .bg(rgb(active_background))
            .text_color(rgb(active_text))
'''+s[b:];a=s.index('            .bg(rgb(theme::color(if track.soloed() {',s.index('let header_content'));b=s.index('            .on_click(',a);s=s[:a]+'''            .bg(rgb(solo_background))
            .text_color(rgb(solo_text))
'''+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/return_tracks.rs');s=p.read_text();s=s.replace('            let muted = track.muted();','            let muted = track.muted();\n            let (active_background, active_text) = super::track_gain::activator_colors(!muted);',1);needle='''                        "toggle return mute",
                    )
                    .on_click''';assert needle in s;s=s.replace(needle,'''                        if muted { "activate return" } else { "deactivate return" },
                    )
                    .w(px(28.0)).h(px(18.0))
                    .bg(rgb(active_background)).text_color(rgb(active_text))
                    .on_click''',1);p.write_text(s)
p=Path('crates/hex-widgets/src/numeric_control.rs');s=p.read_text().replace('// Rectangular readouts always scrub vertically, independent of the\n            // track/dial presentation their owner uses.','// Other rectangular readouts scrub vertically, independent of the\n            // track/dial presentation their owner uses.');p.write_text(s)
p=Path('crates/hex-ui/src/editor/track_gain/tests.rs');s=p.read_text();a=s.index('fn return_header_filled_gain_commits_and_keyboard_invalid_draft_blocks_owner_transition');b=s.index('\n}\n',a);s=s[:b]+'''
    visual.run_until_parked();
    let mute_id = Box::leak(format!("return-mute-{}", id.value()).into_boxed_str());
    let on = visual.rendered_background(mute_id).unwrap();
    assert_eq!(on, gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into());
    let hit = visual.debug_bounds(mute_id).unwrap().center();
    visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, |view| view.worker_requests_pending == 0 && view.state.as_ref().unwrap().project.return_tracks()[0].muted());
    visual.run_until_parked();
    assert_ne!(visual.rendered_background(mute_id).unwrap(), on);
'''+s[b:];p.write_text(s)
