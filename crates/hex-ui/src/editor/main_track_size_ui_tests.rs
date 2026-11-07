//! Main shares local row sizing, overview and rollback with ordinary tracks.
use super::*;
fn main(visual: &mut VisualTestContext) -> Bounds<Pixels> {
    visual.debug_bounds("pinned-main-track").unwrap()
}
fn edge(visual: &mut VisualTestContext) -> gpui::Point<Pixels> {
    visual
        .debug_bounds("main-track-height-resize")
        .expect("main height handle")
        .center()
}
#[gpui::test]
fn main_shrinks_to_header_only_expands_and_keeps_pin_alignment_and_controls(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _tracks) = fixture(cx, 2);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let original = main(&mut visual);
    let from = edge(&mut visual);
    // Main is pinned at the bottom: its top edge follows an upwards expansion.
    drag(&mut visual, from, from + point(px(0.), px(1000.)));
    let compact = main(&mut visual);
    assert_eq!(
        compact.size.height,
        px(super::super::track_layout::HEADER_ONLY_HEIGHT)
    );
    assert_eq!(compact.bottom(), original.bottom());
    let header = visual.debug_bounds("main-track-header").unwrap();
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    assert_eq!(header.size.height, lane.size.height);
    assert_eq!(header.top(), lane.top());
    assert!(
        visual.debug_bounds("main-track-controls").is_none(),
        "compact controls have no hidden hits"
    );
    let from = edge(&mut visual);
    drag(&mut visual, from, from - point(px(0.), px(70.)));
    let expanded = main(&mut visual);
    assert_eq!(expanded.size.height, compact.size.height + px(70.));
    assert_eq!(expanded.bottom(), original.bottom());
    let controls = visual.debug_bounds("main-track-controls").unwrap();
    let handle = visual.debug_bounds("main-track-height-resize").unwrap();
    assert!(controls.top() >= handle.bottom());
    assert!(controls.bottom() <= expanded.bottom());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.worker_requests_pending, 0);
    });
    let gain = visual.debug_bounds("main-gain-up").unwrap().center();
    visual.simulate_click(gain, Modifiers::default());
    editor.read_with(&visual, |v, _| assert!(v.last_enqueued_edit.is_some()));
}
#[gpui::test]
fn main_all_track_resize_and_overview_share_rollback_and_restore_normal_heights(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    let original = main(&mut visual);
    let from = edge(&mut visual);
    let to = from - point(px(0.), px(30.));
    visual.simulate_mouse_down(from, MouseButton::Left, alt);
    visual.simulate_mouse_move(to, MouseButton::Left, alt);
    assert_heights(&mut visual, &tracks, &[74.; 2]);
    assert_eq!(main(&mut visual).size.height, px(74.));
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(to, MouseButton::Left, alt);
    assert_heights(&mut visual, &tracks, &[44.; 2]);
    assert_eq!(main(&mut visual), original);
    let from = edge(&mut visual);
    drag(&mut visual, from, from - point(px(0.), px(30.)));
    assert_eq!(main(&mut visual).size.height, px(74.));
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.; 2]);
    assert_eq!(main(&mut visual).size.height, px(23.));
    let from = edge(&mut visual);
    drag(&mut visual, from, from - point(px(0.), px(10.)));
    assert_eq!(main(&mut visual).size.height, px(33.));
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[44.; 2]);
    assert_eq!(main(&mut visual).size.height, px(74.));
    visual.simulate_keystrokes("h");
    assert_heights(&mut visual, &tracks, &[23.; 2]);
    assert_eq!(main(&mut visual).size.height, px(23.));
}

#[gpui::test]
fn main_resize_after_window_shrink_starts_from_painted_height(cx: &mut TestAppContext) {
    let (mut visual, editor, _) = fixture(cx, 0);
    let from = edge(&mut visual);
    drag(&mut visual, from, from - point(px(0.), px(1000.)));
    visual.simulate_resize(size(px(880.), px(540.)));
    visual.run_until_parked();
    let displayed = main(&mut visual);
    let detail = visual.debug_bounds("clip-view").unwrap();
    assert!(displayed.size.height >= px(23.));
    assert!(displayed.bottom() <= detail.top());
    let from = edge(&mut visual);
    drag(&mut visual, from, from + point(px(0.), px(10.)));
    let next = main(&mut visual);
    assert_eq!(
        next.size.height,
        (displayed.size.height - px(10.)).max(px(23.))
    );
    assert_eq!(next.bottom(), displayed.bottom());
    editor.read_with(&visual, |v, _| assert_eq!(v.worker_requests_pending, 0));
}

#[gpui::test]
fn section_header_follows_current_main_height_when_overview_compacts_it(cx: &mut TestAppContext) {
    let mut model = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    model
        .execute(Command::AddArrangementSection {
            name: "section".into(),
            color: hex_project::ElementColor::Blue,
            start: hex_project::BeatTime::ZERO,
            end: hex_project::BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap();
    let (mut visual, editor) = project_window(cx, model.project().clone());
    let from = edge(&mut visual);
    drag(&mut visual, from, from - point(px(0.), px(100.)));
    // Leave the large row's previous measurement installed, then perform one H edit.
    focus_workspace(&mut visual, &editor);
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    let header = visual.debug_bounds("arrangement-section-header-0").unwrap();
    let expected = super::super::time_selection::header_hit_height(f32::from(lane.size.height));
    assert!(
        (f32::from(header.size.height) - expected).abs() <= 1.,
        "header must use the current row: {header:?}, lane {lane:?}"
    );
}
