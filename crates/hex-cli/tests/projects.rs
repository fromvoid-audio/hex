use hex_media::{Limits, decode_wav_file};
use hex_project::{Command, Created, Editor, Project};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-cli-project-tests-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&48_000_u32.to_le_bytes());
    out.extend_from_slice(&96_000_u32.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn run(directory: &Path, args: &[&str]) -> Output {
    ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run hex-cli")
}

#[test]
fn explicit_cli_export_range_stems_and_overwrite_preserve_prior_output() {
    let directory = TempDir::new();
    fs::write(
        directory.path().join("source.wav"),
        wav(&[16384, 8192, -8192]),
    )
    .unwrap();
    assert!(
        run(
            &directory.0,
            &["import-project", "source.wav", "project.hex"]
        )
        .status
        .success()
    );
    let project = hex_storage::load(directory.path().join("project.hex")).unwrap();
    let id = project.tracks()[0].id().value();
    let scope = format!("--stems={id}");
    let stems = run(
        &directory.0,
        &["export-project", "project.hex", "batch", "1", "3", &scope],
    );
    assert!(
        stems.status.success(),
        "{}",
        String::from_utf8_lossy(&stems.stderr)
    );
    let entries: Vec<_> = fs::read_dir(directory.path().join("batch"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    let data = fs::read(&entries[0]).unwrap();
    assert_eq!(data.len(), 52);
    assert_eq!(i16::from_le_bytes([data[44], data[45]]), 8192);
    fs::write(directory.path().join("existing.wav"), b"old recording").unwrap();
    let refusal = run(
        &directory.0,
        &["export-project", "project.hex", "existing.wav", "1", "3"],
    );
    assert!(!refusal.status.success());
    assert_eq!(
        fs::read(directory.path().join("existing.wav")).unwrap(),
        b"old recording"
    );
    let replaced = run(
        &directory.0,
        &[
            "export-project",
            "project.hex",
            "existing.wav",
            "1",
            "3",
            "--overwrite",
        ],
    );
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    let text = String::from_utf8(replaced.stdout).unwrap();
    let recovery = text
        .lines()
        .find_map(|line| line.strip_prefix("previous file preserved at "))
        .unwrap();
    assert_eq!(
        fs::read(directory.path().join(recovery)).unwrap(),
        b"old recording"
    );
    assert_eq!(
        &fs::read(directory.path().join("existing.wav")).unwrap()[44..],
        &data[44..]
    );
    assert!(
        !run(
            &directory.0,
            &["export-project", "project.hex", "refused.wav", "3", "1"]
        )
        .status
        .success()
    );
    assert!(!directory.path().join("refused.wav").exists());
}

fn valid_project_with_missing_media(path: &str) -> Project {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
    match editor
        .execute(Command::ImportAudioTrack {
            name: "imported audio".into(),
            path: path.into(),
            sample_rate: 48_000,
            frames: 3,
        })
        .expect("metadata import")
    {
        Created::Imported { .. } => editor.project().clone(),
        result => panic!("unexpected result: {result:?}"),
    }
}

#[test]
fn imports_and_renders_project_with_case_preserved_media_path() {
    let directory = TempDir::new();
    let source = "Source-MiXeD.WAV";
    fs::write(directory.path().join(source), wav(&[-16_384, 0, 16_384]))
        .expect("write wav fixture");

    let imported = run(
        &directory.0,
        &["import-project", source, "Session-MiXeD.hex"],
    );
    assert!(
        imported.status.success(),
        "{}{}",
        String::from_utf8_lossy(&imported.stdout),
        String::from_utf8_lossy(&imported.stderr)
    );
    let project =
        hex_storage::load(directory.path().join("Session-MiXeD.hex")).expect("load saved project");
    assert_eq!(project.sample_rate(), 48_000);
    assert_eq!(project.tracks().len(), 1);
    assert_eq!(project.tracks()[0].name(), "imported audio");
    assert_eq!(
        project.assets().next().expect("asset").path(),
        fs::canonicalize(directory.path())
            .expect("canonical test path")
            .join(source)
    );
    assert_eq!(project.tracks()[0].clips()[0].duration(), 3);

    let rendered = run(
        &directory.0,
        &["render-project", "Session-MiXeD.hex", "Rendered-MiXeD.WAV"],
    );
    assert!(
        rendered.status.success(),
        "{}{}",
        String::from_utf8_lossy(&rendered.stdout),
        String::from_utf8_lossy(&rendered.stderr)
    );
    let output = decode_wav_file(
        directory.path().join("Rendered-MiXeD.WAV"),
        Limits::new(4096, 32),
    )
    .expect("decode rendered wav");
    assert_eq!(output.sample_rate(), 48_000);
    assert_eq!(output.frames(), 3);
    let samples = output.audio().frames();
    assert!(samples[0][0] < -0.4);
    assert_eq!(samples[1], [0.0, 0.0]);
    assert!(samples[2][0] > 0.4);
}

#[test]
fn missing_referenced_media_fails_before_creating_output() {
    let directory = TempDir::new();
    let project = valid_project_with_missing_media("Missing-MiXeD.WAV");
    hex_storage::save_new(directory.path().join("Missing.hex"), &project)
        .expect("save project metadata");
    let output_path = directory.path().join("Should-Not-Exist.wav");
    let result = run(
        &directory.0,
        &["render-project", "Missing.hex", "Should-Not-Exist.wav"],
    );
    assert!(!result.status.success());
    assert!(!output_path.exists());
    let error = String::from_utf8_lossy(&result.stderr).to_ascii_lowercase();
    assert!(error.contains("no such file or directory"), "{error}");
}

#[test]
fn both_project_import_and_render_refuse_overwrites() {
    let directory = TempDir::new();
    fs::write(directory.path().join("Source.wav"), wav(&[8_192, -8_192]))
        .expect("write wav fixture");
    fs::write(directory.path().join("Existing.hex"), b"keep project")
        .expect("write existing project");
    let import = run(
        &directory.0,
        &["import-project", "Source.wav", "Existing.hex"],
    );
    assert!(!import.status.success());
    assert_eq!(
        fs::read(directory.path().join("Existing.hex")).expect("read target"),
        b"keep project"
    );

    let created = run(
        &directory.0,
        &["import-project", "Source.wav", "Project.hex"],
    );
    assert!(created.status.success());
    fs::write(directory.path().join("Existing.wav"), b"keep audio").expect("write existing output");
    let render = run(
        &directory.0,
        &["render-project", "Project.hex", "Existing.wav"],
    );
    assert!(!render.status.success());
    assert_eq!(
        fs::read(directory.path().join("Existing.wav")).expect("read target"),
        b"keep audio"
    );
}

#[test]
fn command_usage_errors_are_lowercase_and_write_no_output() {
    let directory = TempDir::new();
    let result = run(&directory.0, &["render-project"]);
    assert!(!result.status.success());
    assert!(
        fs::read_dir(directory.path())
            .expect("list dir")
            .next()
            .is_none()
    );
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("usage: hex-cli render-project"));
    assert!(
        error
            .to_lowercase()
            .contains("usage: hex-cli render-project")
    );
}

#[test]
fn renders_converted_source_at_project_rate_and_refuses_changed_source_shape() {
    let directory = TempDir::new();
    let source = directory.path().join("Source.wav");
    let samples: Vec<i16> = (0..480).map(|i| ((i % 80) as i16 - 40) * 400).collect();
    fs::write(&source, wav(&samples)).expect("source WAV");
    let mut editor = Editor::new(Project::new(44_100).expect("project"), 4).expect("editor");
    editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "converted".into(),
            path: "Source.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 480,
            playback_frames: 441,
            conversion_version: hex_project::SINC_CONVERSION_V1,
        })
        .expect("converted metadata");
    hex_storage::save_new(directory.path().join("Converted.hex"), editor.project()).expect("save");
    let rendered = run(
        directory.path(),
        &["render-project", "Converted.hex", "Rendered.wav"],
    );
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let output = decode_wav_file(
        directory.path().join("Rendered.wav"),
        Limits::new(4096, 441),
    )
    .expect("output");
    assert_eq!(output.sample_rate(), 44_100);
    assert_eq!(output.frames(), 441);
    let input = decode_wav_file(&source, Limits::new(4096, 480)).expect("source");
    let reference = hex_media::resample(
        input.audio(),
        44_100,
        hex_media::ResampleLimits {
            max_source_frames: 480,
            max_output_frames: 441,
        },
    )
    .expect("conversion reference");
    for (rendered, reference) in output.audio().frames().iter().zip(reference.frames()) {
        for channel in 0..2 {
            assert!((rendered[channel] - reference[channel]).abs() <= 1.0 / 32768.0);
        }
    }
    fs::write(&source, wav(&samples[..479])).expect("changed source");
    let refused = run(
        directory.path(),
        &["render-project", "Converted.hex", "Refused.wav"],
    );
    assert!(!refused.status.success());
    assert!(!directory.path().join("Refused.wav").exists());
}

#[test]
fn recovers_prior_project_elsewhere_with_original_media_origin_even_without_target() {
    let directory = TempDir::new();
    let original = directory.path().join("Original-MiXeD");
    let elsewhere = directory.path().join("Elsewhere");
    fs::create_dir(&original).expect("original directory");
    fs::create_dir(&elsewhere).expect("other directory");
    let target = original.join("Session.hex");
    let prior = valid_project_with_missing_media("Media/Drum-MiXeD.WAV");
    hex_storage::save_new(&target, &prior).expect("initial project");
    let (_, token) = hex_storage::load_with_token(&target).expect("initial token");
    hex_storage::replace_existing(&target, &token, &Project::new(44_100).expect("new project"))
        .expect("replacement");
    let backup = hex_storage::recovery_path(&target).expect("recovery path");
    let target_bytes = fs::read(&target).expect("target bytes");
    let backup_bytes = fs::read(&backup).expect("backup bytes");
    for output in ["Recovered.hex", "After-Deletion.hex"] {
        let result = run(
            &elsewhere,
            &["recover-project", "../Original-MiXeD/Session.hex", output],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let restored = hex_storage::load(elsewhere.join(output)).expect("restored project");
        assert_eq!(restored.sample_rate(), prior.sample_rate());
        assert_eq!(restored.tracks(), prior.tracks());
        let media = restored.assets().next().expect("retained media").path();
        assert_eq!(
            media,
            fs::canonicalize(&elsewhere)
                .expect("working directory")
                .join("../Original-MiXeD/Media/Drum-MiXeD.WAV")
        );
        assert!(
            !media.exists(),
            "missing media does not prevent metadata recovery"
        );
        assert_eq!(fs::read(&backup).expect("unchanged backup"), backup_bytes);
        if output == "Recovered.hex" {
            assert_eq!(fs::read(&target).expect("unchanged target"), target_bytes);
            fs::remove_file(&target).expect("remove own fixture target");
        }
    }
    assert!(!target.exists());
}

#[test]
fn recovery_refuses_collisions_and_unknown_backups_without_mutating_inputs() {
    let directory = TempDir::new();
    let target = directory.path().join("Session.hex");
    hex_storage::save_new(&target, &Project::new(48_000).expect("project")).expect("initial save");
    let (_, token) = hex_storage::load_with_token(&target).expect("token");
    hex_storage::replace_existing(
        &target,
        &token,
        &Project::new(44_100).expect("next project"),
    )
    .expect("replacement");
    let backup = hex_storage::recovery_path(&target).expect("recovery path");
    let original = fs::read(&target).expect("target bytes");
    let retained = fs::read(&backup).expect("backup bytes");
    fs::write(directory.path().join("Existing.hex"), b"keep this").expect("existing destination");
    let collision = run(
        directory.path(),
        &["recover-project", "Session.hex", "Existing.hex"],
    );
    assert!(!collision.status.success());
    assert_eq!(
        fs::read(directory.path().join("Existing.hex")).expect("destination"),
        b"keep this"
    );
    assert_eq!(fs::read(&backup).expect("backup"), retained);
    fs::write(&backup, b"unknown backup format").expect("malformed fixture");
    let malformed = run(
        directory.path(),
        &["recover-project", "Session.hex", "New.hex"],
    );
    assert!(!malformed.status.success());
    assert!(!directory.path().join("New.hex").exists());
    assert_eq!(fs::read(&backup).expect("backup"), b"unknown backup format");
    assert_eq!(fs::read(&target).expect("target"), original);
}

#[test]
fn persisted_balance_and_master_gain_reach_the_shared_wav_renderer() {
    let directory = TempDir::new();
    let source = directory.path().join("Stereo-Input.WAV");
    fs::write(&source, wav(&[-8192, 4096, 16384])).expect("input WAV");
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 4).expect("editor");
    let Created::Imported { track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "mix".into(),
            path: "Stereo-Input.WAV".into(),
            sample_rate: 48_000,
            frames: 3,
        })
        .expect("import metadata")
    else {
        panic!("import result")
    };
    editor
        .execute(Command::Balance {
            track,
            balance: 0.5,
        })
        .expect("balance");
    editor
        .execute(Command::MasterGain { gain: 1.5 })
        .expect("master");
    hex_storage::save_new(directory.path().join("Mix.hex"), editor.project()).expect("save mix");
    let result = run(
        directory.path(),
        &["render-project", "Mix.hex", "Mixed.wav"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let rendered = decode_wav_file(directory.path().join("Mixed.wav"), Limits::new(4096, 3))
        .expect("rendered WAV");
    let expected = [[-0.1875, -0.375], [0.09375, 0.1875], [0.375, 0.75]];
    for (actual, expected) in rendered.audio().frames().iter().zip(expected) {
        for channel in 0..2 {
            assert!((actual[channel] - expected[channel]).abs() <= 1.0 / 32768.0);
        }
    }
    assert_eq!(rendered.frames(), 3);
}
