//! Persisted musical edits must not retime or warp frame-anchored source audio.
use hex_project::{
    Command, Editor, Project,
    timebase::{BeatTime, Meter, Tempo},
};
use std::{fs, path::PathBuf, process::Command as ProcessCommand};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("hex-cli-timebase-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let fixture = Self(path);
        fixture.run(&["demo", "Original-MiXeD.wav"]);
        fixture.run(&["import-project", "Original-MiXeD.wav", "initial.hex"]);
        fixture
    }
    fn run(&self, args: &[&str]) {
        let result = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn render(&self, project: &Project, stem: &str) -> Vec<u8> {
        let metadata = format!("{stem}.hex");
        let output = format!("{stem}.wav");
        hex_storage::save_new(self.0.join(&metadata), project).unwrap();
        assert_eq!(hex_storage::load(self.0.join(&metadata)).unwrap(), *project);
        self.run(&["render-project", &metadata, &output]);
        fs::read(self.0.join(output)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn saved_tempo_and_fragmentary_meter_edits_preserve_frame_audio_through_history() {
    let fixture = Fixture::new();
    let original = hex_storage::load(fixture.0.join("initial.hex")).unwrap();
    let clips = original.tracks()[0].clips().to_vec();
    let baseline = fixture.render(&original, "baseline");
    let mut editor = Editor::new(original, 16).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(2).unwrap(),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(4).unwrap())
            .unwrap(),
        112_000
    );
    editor
        .execute(Command::SetMeterPoint {
            beat: BeatTime::from_quarters(3).unwrap(),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    let fragment = editor
        .project()
        .meter_map()
        .position(BeatTime::from_quarters(2).unwrap())
        .unwrap();
    assert!(fragment.fragmentary());
    assert_eq!(
        fragment.bar_end(),
        Some(BeatTime::from_quarters(3).unwrap())
    );
    let next = editor
        .project()
        .meter_map()
        .position(BeatTime::from_quarters(3).unwrap())
        .unwrap();
    assert_eq!((next.bar(), next.beat()), (2, 1));
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(150_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(4).unwrap())
            .unwrap(),
        102_400
    );
    assert_eq!(editor.project().tracks()[0].clips(), clips);
    assert_eq!(fixture.render(editor.project(), "mapped"), baseline);
    assert!(editor.try_undo().unwrap());
    assert_eq!(
        editor
            .project()
            .tempo_map()
            .beat_to_frame(BeatTime::from_quarters(4).unwrap())
            .unwrap(),
        112_000
    );
    assert_eq!(fixture.render(editor.project(), "undo"), baseline);
    assert!(editor.try_redo().unwrap());
    assert_eq!(fixture.render(editor.project(), "redo"), baseline);
}
