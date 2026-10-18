from pathlib import Path
p=Path('crates/hex-ui/src/editor/changelog.rs');s=p.read_text();a=s.index('pub(super) struct State');s=s[:a]+'''#[cfg(all(test, feature = "ui-test-support"))]
type PaintSamples = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, (gpui::Hsla, gpui::SharedString)>>>;

'''+s[a:]
a=s.index('    painted: std::rc::Rc<');b=s.index('\n    scroll:',a);s=s[:a]+'    painted: PaintSamples,'+s[b:]
i=s.index('\n}\n\n#[cfg(all(test, feature = "ui-test-support"))]\nmod ui_tests');s=s[:i]+'''
    #[test]
    fn category_projection_preserves_validation_and_group_order() {
        let raw = r#"{"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":[{"category":"fixes","text":"first fix"},{"category":"features","text":"first feature"},{"category":"fixes","text":"second fix"}]}}]}"#;
        let parsed=notes(raw).unwrap();
        assert_eq!(parsed.batches[0].groups[0],["first feature"]);
        assert!(parsed.batches[0].groups[1].is_empty());
        assert_eq!(parsed.batches[0].groups[2],["first fix","second fix"]);
        for value in [raw.replace("first fix",""),raw.replace("first fix","Uppercase"),raw.replace("first fix","line\\\\nbreak"),raw.replace("first fix",&"x".repeat(321)),raw.replace("fixes","unknown"),raw.replace("0123456789012345678901234567890123456789","invalid"),raw.replace("\\"format\\":1","\\"format\\":2")] { assert!(notes(&value).is_err()); }
        assert!(notes(&raw.replace("first fix",&"é".repeat(320))).is_ok());
    }
'''+s[i:]
i=s.rfind('\n}');s=s[:i]+'''
    fn install(editor: &Entity<EditorView>, visual: &mut VisualTestContext, raw: &str) {
        editor.update(visual, |view,cx| {
            view.changelog.document=notes(raw).unwrap_or_else(|error|Document::message(format!("changelog unavailable · {error}")));
            view.changelog.scroll.set_offset(point(px(0.0),px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
    }
    fn raw_entries(entries: serde_json::Value) -> String {
        serde_json::json!({"format":1,"batches":[{"input":{"version":"0.1.0","source":"0123456789012345678901234567890123456789"},"result":{"entries":entries}}]}).to_string()
    }

    #[gpui::test]
    fn changelog_empty_groups_unavailable_and_malformed_render_truthfully(cx:&mut TestAppContext) {
        let (_directory,mut visual,editor)=fixture(cx);
        click(&mut visual,"open-changelog");
        for raw in ["".to_owned(),"not json".to_owned(),r#"{"format":1,"batches":[]}"#.to_owned()] {
            install(&editor,&mut visual,&raw);
            assert!(visual.debug_bounds("changelog-unavailable").is_some());
            assert!(visual.debug_bounds("changelog-heading-0-features").is_none());
            assert!(visual.debug_bounds("changelog-empty").is_none());
        }
        install(&editor,&mut visual,&raw_entries(serde_json::json!([])));
        assert!(visual.debug_bounds("changelog-empty").is_some());
        assert!(visual.debug_bounds("changelog-unavailable").is_none());
        install(&editor,&mut visual,&raw_entries(serde_json::json!([{"category":"fixes","text":"one complete fix"}])));
        assert!(visual.debug_bounds("changelog-heading-0-fixes").is_some());
        assert!(visual.debug_bounds("changelog-heading-0-features").is_none());
        assert!(visual.debug_bounds("changelog-heading-0-improvements").is_none());
        visual.simulate_keystrokes("escape");
        editor.update_in(&mut visual,|view,window,_|assert!(view.focus.is_focused(window)));
    }

    #[gpui::test]
    fn changelog_long_note_wraps_at_compact_and_normal_width_without_hover_reflow(cx:&mut TestAppContext) {
        let (_directory,mut visual,editor)=fixture(cx);
        click(&mut visual,"open-changelog");
        let raw=raw_entries(serde_json::json!([{"category":"fixes","text":"a readable long note ".repeat(15)},{"category":"fixes","text":"é".repeat(320)}]));
        install(&editor,&mut visual,&raw);
        for (width,height) in [(480.0,360.0),(880.0,540.0),(1280.0,800.0)] {
            visual.simulate_resize(size(px(width),px(height)));visual.run_until_parked();
            let reader=visual.debug_bounds("changelog-reader").unwrap();
            let viewport=visual.debug_bounds("changelog-notes").unwrap();
            let row=visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
            let second=visual.debug_bounds("changelog-note-0-fixes-1").unwrap();
            let close=visual.debug_bounds("close-changelog").unwrap();
            assert!(reader.size.width<=px(640.0) && reader.right()<=px(width));
            assert!(viewport.size.height>px(100.0) && viewport.bottom()<=px(height));
            assert!(row.left()>=viewport.left() && row.right()<=viewport.right());
            assert!(second.left()>=viewport.left() && second.right()<=viewport.right());
            assert!(row.size.height>px(30.0),"long text actually wraps");
            assert!(close.right()<=reader.right() && close.bottom()<=viewport.top());
            visual.simulate_mouse_move(row.origin+point(px(4.0),px(4.0)),None,Modifiers::default());visual.run_until_parked();
            assert_eq!(visual.debug_bounds("changelog-note-0-fixes-0"),Some(row));
            assert_eq!(visual.debug_bounds("changelog-note-0-fixes-1"),Some(second));
        }
    }

    #[gpui::test]
    fn changelog_scrolling_and_reused_rows_do_not_retain_old_item_hover(cx:&mut TestAppContext) {
        let (_directory,mut visual,editor)=fixture(cx);
        let entries:Vec<_>=(0..60).map(|n|serde_json::json!({"category":"fixes","text":format!("a complete short fixture note {n}")})).collect();
        install(&editor,&mut visual,&raw_entries(serde_json::json!(entries)));
        click(&mut visual,"open-changelog");
        let first=visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
        let pointer=first.center();
        visual.simulate_mouse_move(pointer,None,Modifiers::default());visual.run_until_parked();
        assert_eq!(painted(&editor,&visual,"changelog-note-0-fixes-0"),rgb(theme::color(PaletteRole::Secondary)).into());
        visual.simulate_event(gpui::ScrollWheelEvent { position:pointer,delta:gpui::ScrollDelta::Pixels(point(px(0.0),px(-150.0))),..Default::default() });
        visual.run_until_parked();
        assert!(editor.read_with(&visual,|view,_|view.changelog.scroll.offset().y)<px(0.0));
        let mut hovered=None;
        for n in 1..15 {
            let selector: &'static str=Box::leak(format!("changelog-note-0-fixes-{n}").into_boxed_str());
            if let Some(bounds)=visual.debug_bounds(selector) {
                if bounds.contains(&pointer) { hovered=Some(selector);break; }
            }
        }
        let hovered=hovered.expect("another real row scrolled below stationary pointer");
        assert_eq!(painted(&editor,&visual,hovered),rgb(theme::color(PaletteRole::Secondary)).into());
        visual.simulate_mouse_move(point(px(950.0),px(710.0)),None,Modifiers::default());visual.run_until_parked();
        assert_eq!(painted(&editor,&visual,hovered),rgb(theme::color(PaletteRole::Text)).into());
        // Reuse the same row ID for fresh content after leaving the viewport.
        install(&editor,&mut visual,&raw_entries(serde_json::json!([{"category":"fixes","text":"replacement content at the same row identity"}])));
        assert_eq!(painted(&editor,&visual,"changelog-note-0-fixes-0"),rgb(theme::color(PaletteRole::Text)).into());
        let row=visual.debug_bounds("changelog-note-0-fixes-0").unwrap();
        let before=editor.read_with(&visual,|view,_|(view.worker_requests_pending,view.pending_requests.len(),view.offline_cursor));
        visual.simulate_click(row.center(),Modifiers::default());visual.run_until_parked();
        editor.read_with(&visual,|view,_|assert_eq!((view.worker_requests_pending,view.pending_requests.len(),view.offline_cursor),before));
        visual.simulate_keystrokes("tab enter");visual.run_until_parked();
        editor.read_with(&visual,|view,_|assert!(!view.changelog.open));
    }
'''+s[i:];p.write_text(s)
