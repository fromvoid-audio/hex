from pathlib import Path
p=Path('crates/hex-ui/src/editor/musical_time.rs');s=p.read_text();anchor='const RULER_LABEL_SPACING_PX: f32 = 48.0;'
s=s.replace(anchor,anchor+'''

#[cfg(test)]
thread_local! {
    // Actual resolver work per test thread: step/base/share/natural demand.
    static GRID_RESOLUTION_WORK: std::cell::Cell<[usize; 4]> = const { std::cell::Cell::new([0; 4]) };
}

#[cfg(test)]
fn record_grid_work(kind: usize) {
    GRID_RESOLUTION_WORK.with(|work| {
        let mut counts = work.get();
        counts[kind] += 1;
        work.set(counts);
    });
}
''')
for name,kind in [('adaptive_grid_step',0),('adaptive_base_step',1),('capped_fair_share',2)]:
 start=s.index('fn '+name+'('); pos=s.index('{',start)+1;s=s[:pos]+f'\n    #[cfg(test)]\n    record_grid_work({kind});'+s[pos:]
s=s.replace('let demand = |relative: usize| {','let demand = |relative: usize| {\n                #[cfg(test)]\n                record_grid_work(3);')
p.write_text(s)
p=Path('crates/hex-ui/src/editor/adaptive_grid_density_tests.rs');p.write_text(p.read_text()+'''

#[test]
fn adaptive_operation_work_profile() {
    for count in [1, 129, 256, 1024] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        for index in 1..count {
            editor.execute(Command::SetMeterPoint {
                beat: BeatTime::from_quarters(index * 64).unwrap(),
                meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, 4).unwrap(),
            }).unwrap();
        }
        let project = editor.project();
        let length = project.tempo_map().beat_to_frame(BeatTime::from_quarters(count * 64).unwrap()).unwrap();
        let viewport = Viewport::fit(length);
        for density in AdaptiveGridDensity::ALL {
            let grid = SnapGrid::Adaptive.with_density(density);
            for operation in ["paint", "ruler", "snap", "next", "previous", "bands"] {
                GRID_RESOLUTION_WORK.with(|work| work.set([0; 4]));
                let start = std::time::Instant::now();
                match operation {
                    "paint" => { std::hint::black_box(grid_lines(project, viewport, 100_000.0, grid)); }
                    "ruler" => { std::hint::black_box(ruler_ticks(project, viewport, 100_000.0)); }
                    "snap" => { std::hint::black_box(grid.snap_frame(project, length / 2 + 123, viewport, 100_000.0)); }
                    "next" => { std::hint::black_box(grid.adjacent_frame(project, length / 2, viewport, 100_000.0, true)); }
                    "previous" => { std::hint::black_box(grid.adjacent_frame(project, length / 2, viewport, 100_000.0, false)); }
                    "bands" => { std::hint::black_box(adaptive_grid_bands_for_grid(project, viewport, 100_000.0, grid)); }
                    _ => unreachable!(),
                }
                let ns = start.elapsed().as_nanos();
                let work = GRID_RESOLUTION_WORK.with(std::cell::Cell::get);
                println!("GRID_WORK count={count} density={density:?} operation={operation} step={} base={} shares={} demand={} ns={ns}", work[0], work[1], work[2], work[3]);
            }
        }
    }
}
''')
