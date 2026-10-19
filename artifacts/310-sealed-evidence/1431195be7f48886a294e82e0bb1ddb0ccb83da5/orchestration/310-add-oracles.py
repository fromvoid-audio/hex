from pathlib import Path
p=Path('crates/hex-ui/src/editor/adaptive_grid_density_tests.rs');s=p.read_text();needle='''                let work = GRID_RESOLUTION_WORK.with(std::cell::Cell::get);
''';s=s.replace(needle,needle+'''                // One natural-demand pass per operation, even when nearest/
                // directional lookup falls back to the painted identities.
                assert!(work[3] <= count.min(MAX_GRID_LINES as u64) as usize, "{operation}: {work:?}");
                assert!(work[2] <= 2, "{operation}: {work:?}");
''');p.write_text(s)
p=Path('/tmp/310-metadata-harness.py');s=p.read_text();s+='''
# Differential oracle compiles untouched baseline pure functions, not a
# rewritten mathematical stand-in. Identical real Project objects feed both.
baseline=subprocess.check_output(['git','show','712fdebaf5be412a1a4b9006176feb46c1ab6cca:crates/hex-ui/src/editor/musical_time.rs'],cwd=root).decode()
baseline=baseline[:baseline.index('impl super::EditorView {')]
for line in ['use crate::theme;\\n','use gpui::{Context, Div, Window, div, prelude::*, px, rgb};\\n','use hex_session::service::Request;\\n']:
 baseline=baseline.replace(line,'')
probe=r\'''\n#[cfg(test)]
pub(super) fn differential_output(project: &Project, start: u64, length: u64, width: f32, density: usize) -> Vec<String> {
 let viewport = Viewport { start, length };
 let grid = SnapGrid::Adaptive.with_density(AdaptiveGridDensity::ALL[density]);
 let mut result = vec![format!("{:?}", grid_lines(project, viewport, width, grid)), format!("{:?}", ruler_ticks(project, viewport, width)), format!("{:?}", adaptive_grid_bands_for_grid(project, viewport, width, grid))];
 for candidate in [start.saturating_sub(1), start, start.saturating_add(length / 2), start.saturating_add(length)] {
  result.push(format!("{:?}", (grid.snap_frame(project, candidate, viewport, width), grid.adjacent_frame(project, candidate, viewport, width, true), grid.adjacent_frame(project, candidate, viewport, width, false))));
 }
 result
}
\'''
(src/'baseline.rs').write_text(baseline+probe)
f=src/'musical_time.rs';f.write_text(f.read_text()+probe)
f=src/'lib.rs';s=f.read_text();s=s.replace(' #[path="../musical_time.rs"] mod musical_time;',' #[path="../musical_time.rs"] mod musical_time;\\n #[path="../baseline.rs"] mod baseline;'+r\'''
 #[test]
 fn literal_production_baseline_differential() {
  use hex_project::{Project, Editor, Command, BeatTime, Meter, Tempo};
  let mut comparisons = 0;
  for count in [1, 7, 129, 256, 1024] {
   for mixed in [false, true] {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let mut quarters = 0;
    for index in 0..count {
     if index > 0 { editor.execute(Command::SetMeterPoint { beat: BeatTime::from_quarters(quarters).unwrap(), meter: Meter::new(if index % 2 == 0 { 4 } else { 3 }, if index % 3 == 0 { 8 } else { 4 }).unwrap() }).unwrap(); }
     quarters += if mixed && index != count / 2 { 1 } else { 64 };
    }
    editor.execute(Command::SetTempoPoint { beat: BeatTime::from_ratio(3, 2).unwrap(), tempo: Tempo::new(91_000_000).unwrap() }).unwrap();
    let project = editor.project();
    let end = project.tempo_map().beat_to_frame(BeatTime::from_quarters(quarters).unwrap()).unwrap();
    for (start, length) in [(0, end), (1, end.saturating_sub(1)), (3000, end), (end / 2, 24000), (end, 1), (u64::MAX - 3, 8), (0, 0)] {
     for width in [1.0, 640.0, 100000.0, f32::NAN] {
      for density in 0..3 {
       assert_eq!(musical_time::differential_output(project, start, length, width, density), baseline::differential_output(project, start, length, width, density), "count={count} mixed={mixed} start={start} length={length} width={width} density={density}");
       comparisons += 1;
      }
     }
    }
   }
  }
  println!("LITERAL_BASELINE_DIFFERENTIAL {comparisons} complete operation result vectors equal");
 }
\''');f.write_text(s)
''';p.write_text(s)
