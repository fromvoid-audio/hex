#![allow(dead_code)]
pub mod editor {
 #[path="../viewport.rs"] mod viewport;
 #[path="../musical_time.rs"] mod musical_time;
 #[path="../baseline.rs"] mod baseline;
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

}
