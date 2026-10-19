from pathlib import Path
import sys,json,subprocess,hashlib
root=Path('/workspace/hex');dest=root/'artifacts/310-metadata-harness'/sys.argv[1];src=dest/'src';src.mkdir(parents=True,exist_ok=True)
p=root/'crates/hex-ui/src/editor';s=(p/'musical_time.rs').read_text();pure=s[:s.index('impl super::EditorView {')];tests=s[s.index('#[cfg(test)]\nmod tests {'):]
for line in ['use crate::theme;\n','use gpui::{Context, Div, Window, div, prelude::*, px, rgb};\n','use hex_session::service::Request;\n']:
 pure=pure.replace(line,'')
(src/'musical_time.rs').write_text(pure+tests)
for file in ['viewport.rs','adaptive_grid_density.rs','adaptive_grid_density_tests.rs']:(src/file).write_bytes((p/file).read_bytes())
(src/'lib.rs').write_text('#![allow(dead_code)]\npub mod editor {\n #[path="../viewport.rs"] mod viewport;\n #[path="../musical_time.rs"] mod musical_time;\n}\n')
(src/'editor').mkdir(exist_ok=True)
(dest/'Cargo.toml').write_text('[package]\nname="hex-grid-metadata-probe-'+sys.argv[1]+'"\nversion="0.0.0"\nedition="2024"\n[workspace]\n[dependencies]\nhex-project={path="'+str(root/'crates/hex-project')+'"}\n')
(dest/'EXTRACTION.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root).decode().strip(),'production_sha256':hashlib.sha256(s.encode()).hexdigest(),'note':'Literal pure production functions, actual Project/Viewport. UI-only imports and EditorView controls omitted. Not GPUI/native timing.'},indent=2))
print(dest)

# Differential oracle compiles untouched baseline pure functions, not a
# rewritten mathematical stand-in. Identical real Project objects feed both.
baseline=subprocess.check_output(['git','show','712fdebaf5be412a1a4b9006176feb46c1ab6cca:crates/hex-ui/src/editor/musical_time.rs'],cwd=root).decode()
baseline=baseline[:baseline.index('impl super::EditorView {')]
for line in ['use crate::theme;\n','use gpui::{Context, Div, Window, div, prelude::*, px, rgb};\n','use hex_session::service::Request;\n']:
 baseline=baseline.replace(line,'')
probe=r'''
#[cfg(test)]
pub(super) fn differential_output(project: &Project, start: u64, length: u64, width: f32, density: usize) -> Vec<String> {
 let viewport = Viewport { start, length };
 let grid = SnapGrid::Adaptive.with_density(AdaptiveGridDensity::ALL[density]);
 let mut result = vec![format!("{:?}", grid_lines(project, viewport, width, grid)), format!("{:?}", ruler_ticks(project, viewport, width)), format!("{:?}", adaptive_grid_bands_for_grid(project, viewport, width, grid))];
 for candidate in [start.saturating_sub(1), start, start.saturating_add(length / 2), start.saturating_add(length)] {
  result.push(format!("{:?}", (grid.snap_frame(project, candidate, viewport, width), grid.adjacent_frame(project, candidate, viewport, width, true), grid.adjacent_frame(project, candidate, viewport, width, false))));
 }
 result
}
'''
(src/'baseline.rs').write_text(baseline+probe)
f=src/'musical_time.rs';f.write_text(f.read_text()+probe)
f=src/'lib.rs';s=f.read_text();s=s.replace(' #[path="../musical_time.rs"] mod musical_time;',' #[path="../musical_time.rs"] mod musical_time;\n #[path="../baseline.rs"] mod baseline;'+r'''
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
''');f.write_text(s)
