from pathlib import Path
p=Path('crates/hex-ui/src/editor/tracks.rs');s=p.read_text();a=s.index('        let track_controls = div()');b=s.index('        let header_focus =',a);old=s[a:b];s=s[:a]+'''        let header_content = if live_volume {
            let utilities = div().flex().gap(px(2.0))
                .child(button("a", "show track gain automation", ("gain-lane", track_id.value()))
                    .h(px(18.0))
                    .debug_selector(move || format!("gain-lane-show-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, window, cx| view.show_gain_lane(track_id, window, cx))))
                .child(button("↗", "edit return sends", ("track-return-sends", track_id.value()))
                    .h(px(18.0))
                    .debug_selector(move || format!("track-return-sends-{}", track_id.value()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        let next = if view.send_menu_track == Some(track_id) { None } else { Some(track_id) };
                        if next != view.send_menu_track && !super::device_panel::finalize_controls(view, cx) {
                            cx.notify(); return;
                        }
                        view.send_menu_track = next;
                        cx.notify();
                    })));
            let activator = button("", if track.muted() { "activate track" } else { "deactivate track" },
                ("track-mute", track_id.value()))
                .debug_selector(move || format!("track-mute-{}", track_id.value()))
                .flex_1().min_w_0().h(px(18.0))
                .bg(rgb(theme::color(if track.muted() { theme::PaletteRole::Raised } else { theme::PaletteRole::Primary })))
                .text_color(rgb(theme::color(if track.muted() { theme::PaletteRole::Muted } else { theme::PaletteRole::Surface })))
                .child(theme::text((track_index + 1).to_string()))
                .on_click(cx.listener(move |view, _, _, cx| { view.toggle_mute(track_id); cx.notify(); }));
            let solo = button("s", "solo track · s · cmd+click additive", ("track-solo", track_id.value()))
                .debug_selector(move || format!("track-solo-{}", track_id.value()))
                .w(px(28.0)).h(px(18.0))
                .bg(rgb(theme::color(if track.soloed() { theme::PaletteRole::Primary } else { theme::PaletteRole::Raised })))
                .text_color(rgb(theme::color(if track.soloed() { theme::PaletteRole::Surface } else { theme::PaletteRole::Muted })))
                .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                    view.toggle_solo(track_id, !event.modifiers().platform); cx.notify();
                }));
            let arm = div().id(("track-arm-unavailable", track_id.value()))
                .debug_selector(move || format!("track-arm-unavailable-{}", track_id.value()))
                .w(px(28.0)).h(px(18.0)).flex_shrink_0().flex().items_center().justify_center()
                .text_xs().text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                .cursor(gpui::CursorStyle::Arrow).opacity(0.45)
                .tooltip(|_, cx| cx.new(|_| ClipTooltip { name: "recording unavailable · planned for hex 0.2".into() }).into())
                .child(theme::text("○"));
            div().id(("arrangement-track-controls", track_id.value()))
                .debug_selector(move || format!("track-controls-{}", track_id.value()))
                .w_full().flex().items_center().gap(px(2.0))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0))
                    .child(track_name_view).child(utilities))
                .child(div().w(px(122.0)).flex_shrink_0().flex().flex_col().gap(px(2.0))
                    .child(div().w_full().flex().gap(px(2.0)).child(activator).child(solo).child(arm))
                    .child(div().w_full().flex().gap(px(2.0))
                        .child(self.track_volume(track, cx)).child(self.track_pan(track, cx))))
        } else {
'''+old+'''            div().w_full().flex().flex_col().child(track_name_view).child(track_controls)
        };
'''+s[b:];s=s.replace('.w(px(self.track_header_width() - 10.0))','.w_full()',1);s=s.replace('            .child(track_name_view)\n            .child(track_controls)','            .child(header_content)',1);p.write_text(s)
p=Path('crates/hex-ui/src/editor/return_tracks.rs');s=p.read_text();a=s.index('                .child(\n                    return_button(\n                        format!("return-gain-down-');b=s.index('                .child(\n                    return_button(\n                        format!("return-mute-',a);s=s[:a]+'''                .child(div().w(px(82.0)).flex_shrink_0().child(self.return_volume(&track, cx)))
'''+s[b:];s=s.replace('            let gain = track.gain();\n','',1);p.write_text(s)
