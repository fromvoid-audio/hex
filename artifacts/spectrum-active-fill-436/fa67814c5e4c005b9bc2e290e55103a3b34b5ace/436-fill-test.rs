
struct BudgetFixture {
    layers: SpectrumPlotLayers,
}
impl Render for BudgetFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        layered_spectrum_plot(self.layers.clone(), STYLE)
    }
}

#[gpui::test]
fn production_active_fill_budget_ignores_silent_and_invalid_layers(cx: &mut TestAppContext) {
    let handle = *cx.add_window(|_, _| Empty);
    let fixture = cx.update_window(handle, |_, window, cx| {
        window.replace_root(cx, |_, _| BudgetFixture { layers: SpectrumPlotLayers::default() })
    }).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(440.0), px(200.0)));
    visual.run_until_parked();
    let active = SpectrumPlotLayer { points: points(), color: 0xff6655 };
    let floor = SpectrumPlotLayer { points: vec![[0.0, 0.0], [1.0, 0.0]], color: 0x55ddff };
    let invalid = SpectrumPlotLayer { color: 0x1000000, ..active.clone() };
    let mut baseline_geometry = None;
    for (case, active_count, floor_count, invalid_count) in [
        ("single", 1, 0, 0),
        ("silent", 1, 256, 0),
        ("mixed", 1, 128, 128),
        ("two-active", 2, 127, 128),
        ("all-inactive", 0, 128, 129),
    ] {
        let layers = [vec![active.clone(); active_count], vec![floor.clone(); floor_count], vec![invalid.clone(); invalid_count]].concat();
        assert!(layers.len() <= 257);
        RECORDS.with(|r| *r.borrow_mut() = Some(Vec::new()));
        fixture.update(cx, |view, cx| {
            view.layers = SpectrumPlotLayers::new(layers).unwrap();
            cx.notify();
        });
        visual.run_until_parked();
        let records = RECORDS.with(|r| r.borrow_mut().take().unwrap());
        let fills = records.iter().filter(|r| r.pass == Pass::Fill).collect::<Vec<_>>();
        if active_count == 0 {
            assert!(fills.is_empty(), "{case}: inactive layers must not paint a fill");
            continue;
        }
        assert!(!fills.is_empty(), "{case}: actual production path must paint");
        assert_eq!(fills.len() % active_count, 0);
        for fill in fills {
            let expected = if active_count == 1 { 0.14 } else { 0.12 };
            assert_eq!(fill.alpha, expected, "{case}: only valid nonfloor spectra spend the fill budget");
            assert_eq!(fill.color, active.color);
            assert_eq!(fill.mask, fill.plot);
            let geometry = format!("{:?}", fill.commands);
            if let Some(baseline) = &baseline_geometry { assert_eq!(&geometry, baseline, "{case}: active path geometry stays exact"); }
            else { baseline_geometry = Some(geometry); }
        }
        assert!(records.iter().all(|r| r.color <= 0xffffff));
    }
}
