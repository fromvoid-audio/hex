//! Standalone rendered Operator focus traversal with a real Session.
use gpui::TestAppContext;

#[gpui::test]
fn operator_tab_cycles_rendered_controls_in_both_directions_without_project_edits(
    cx: &mut TestAppContext,
) {
    hex_ui::exercise_operator_focus(cx);
}
