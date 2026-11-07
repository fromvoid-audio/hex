//! Composition ownership: one bottom map, with no duplicate chrome above time.
use super::clip_drag_ui_test::editor_window;
use gpui::{TestAppContext, VisualTestContext, px, size};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::sync::Arc;

#[gpui::test]
fn ruler_touches_spectrum_and_only_the_bottom_map_remains(cx: &mut TestAppContext) {
    for populated in [false, true] {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        if populated {
            let Created::Track(track) = model
                .execute(Command::AddTrack {
                    name: "synth".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            model
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ONE,
                    length: BeatTime::from_quarters(4).unwrap(),
                })
                .unwrap();
        }
        let original = model.project().snapshot();
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        for dimensions in [(880.0, 540.0), (1440.0, 800.0)] {
            visual.simulate_resize(size(px(dimensions.0), px(dimensions.1)));
            visual.run_until_parked();
            assert!(visual.debug_bounds("arrangement-overview-map").is_none());
            let spectrum = visual.debug_bounds("project-spectrum").unwrap();
            let ruler = visual.debug_bounds("timeline-ruler").unwrap();
            assert_eq!(
                ruler.top(),
                spectrum.bottom(),
                "the removed 24px strip is reclaimed"
            );
            let map = visual.debug_bounds("main-arrangement-map").unwrap();
            let main = visual.debug_bounds("main-track-lane").unwrap();
            assert_eq!(map, main);
            assert_eq!(map.left(), ruler.left());
            assert_eq!(map.size.width, ruler.size.width);
            assert!(map.top() > ruler.bottom());
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), original);
            });
        }
    }
}
