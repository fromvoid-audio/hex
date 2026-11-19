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
fn explicit_cli_export_range_stems_and_overwrite_preserve_previous_file() {
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
    let stem_sample = i16::from_le_bytes([data[44], data[45]]);
    assert!(
        (i32::from(stem_sample) - 8192).abs() <= 1,
        "dithered {stem_sample}"
    );
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
    let retained = text
        .lines()
        .find_map(|line| line.strip_prefix("previous file preserved at "))
        .expect("confirmed overwrite reports the retained file");
    let retained = directory.path().join(retained);
    assert_eq!(fs::read(&retained).unwrap(), b"old recording");
    let staging: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hex-export-")
        })
        .collect();
    assert_eq!(staging, vec![retained]);
    // Main and stem files carry independent ±1 LSB TPDF dither streams.
    let replaced = fs::read(directory.path().join("existing.wav")).unwrap();
    assert_eq!(replaced.len(), data.len());
    for (main, stem) in replaced[44..]
        .chunks_exact(2)
        .zip(data[44..].chunks_exact(2))
    {
        let (main, stem) = (
            i16::from_le_bytes([main[0], main[1]]),
            i16::from_le_bytes([stem[0], stem[1]]),
        );
        assert!(
            (i32::from(main) - i32::from(stem)).abs() <= 2,
            "{main} vs {stem}"
        );
    }
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
            // Decode scale (1 LSB) plus rounding and ±1 LSB TPDF dither (1.5 LSB).
            assert!((rendered[channel] - reference[channel]).abs() <= 2.6 / 32_768.0);
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

#[test]
fn path_bearing_errors_keep_the_exact_mixed_case_path() {
    let directory = TempDir::new();
    let project = directory.path().join("MiXeD Case").join("Huge.HEX");
    fs::create_dir(project.parent().unwrap()).expect("mixed-case directory");
    fs::File::create(&project)
        .and_then(|file| file.set_len(1 << 30))
        .expect("sparse oversized project");
    let output = directory.path().join("Out.wav");
    let result = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .arg("render-project")
        .arg(&project)
        .arg(&output)
        .output()
        .expect("run cli");
    assert!(!result.status.success());
    let stderr = String::from_utf8(result.stderr).expect("utf-8 stderr");
    assert!(
        stderr.contains(project.to_str().expect("utf-8 path")),
        "{stderr}"
    );
    assert!(!output.exists());
}

/// Save the imported one-track project with a default reverb on its track, so
/// the automatic export range must include the reverb's declared finite tail.
fn reverb_tail_project(directory: &Path) -> (Project, PathBuf) {
    fs::write(directory.join("source.wav"), wav(&[16_384, -8_192, 4_096])).unwrap();
    assert!(
        run(directory, &["import-project", "source.wav", "dry.hex"])
            .status
            .success()
    );
    let mut editor = Editor::new(hex_storage::load(directory.join("dry.hex")).unwrap(), 4).unwrap();
    let track = editor.project().tracks()[0].id();
    let state = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.reverb",
    ))
    .unwrap();
    let Created::Device(_) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("reverb")
    };
    hex_storage::save_new(directory.join("tail.hex"), editor.project()).unwrap();
    (editor.project().clone(), directory.join("source.wav"))
}

#[test]
fn render_project_includes_effect_tail_and_matches_canonical_export_bytes() {
    let directory = TempDir::new();
    let (project, source) = reverb_tail_project(directory.path());
    let rendered = run(&directory.0, &["render-project", "tail.hex", "cli.wav"]);
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );

    // The UI's ExportRange { frames: None } path for an ordinary in-memory project.
    let media = decode_wav_file(&source, Limits::new(4096, 3)).unwrap();
    let asset = project.assets().next().unwrap().id();
    let bank = std::collections::BTreeMap::from([(asset, media.audio().clone())]);
    let prepared = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &bank,
            std::sync::Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let signal = hex_export::ExportSignal::Main;
    let frames =
        hex_export::controlled_frame_count(prepared.arrangement(), 0, None, &signal).unwrap();
    assert!(frames > 3, "reverb tail must extend past the 3-frame clip");
    let canonical = directory.path().join("canonical.wav");
    hex_export::render_prepared_controlled(
        &canonical,
        prepared,
        0,
        frames,
        signal,
        &hex_export::ExportControl::default(),
    )
    .unwrap();

    let cli = fs::read(directory.path().join("cli.wav")).unwrap();
    assert_eq!(u64::try_from(cli.len() - 44).unwrap(), frames * 4);
    assert_eq!(cli, fs::read(canonical).unwrap());
}

#[test]
fn export_project_overwrite_publishes_to_an_absent_destination() {
    let directory = TempDir::new();
    fs::write(directory.path().join("source.wav"), wav(&[8_192, -8_192])).unwrap();
    assert!(
        run(
            &directory.0,
            &["import-project", "source.wav", "project.hex"]
        )
        .status
        .success()
    );
    let result = run(
        &directory.0,
        &[
            "export-project",
            "project.hex",
            "new.wav",
            "0",
            "2",
            "--overwrite",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read(directory.path().join("new.wav")).unwrap().len(),
        52
    );
}

#[test]
fn export_project_format_option_writes_pcm24_and_float32_that_decode_to_the_render() {
    let directory = TempDir::new();
    fs::write(
        directory.path().join("source.wav"),
        wav(&[16_384, 8_192, -8_192]),
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
    let id = hex_storage::load(directory.path().join("project.hex"))
        .unwrap()
        .tracks()[0]
        .id()
        .value();
    let stems = format!("--stems={id}");
    let export = |destination: &str, extra: &[&str]| {
        let mut args = vec!["export-project", "project.hex", destination, "0", "3"];
        args.extend_from_slice(extra);
        let result = run(&directory.0, &args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    export("float.wav", &["--format=float32"]);
    export("pcm24.wav", &["--overwrite", "--format=pcm24"]);
    export("stems", &["--format=float32", &stems]);
    let decode = |path: PathBuf| {
        decode_wav_file(path, Limits::new(4096, 3))
            .expect("decode export")
            .audio()
            .frames()
            .to_vec()
    };
    let float = decode(directory.path().join("float.wav"));
    // Float output is bit-exact: the 16-bit source values pass through at unity.
    assert_eq!(float, [[0.5, 0.5], [0.25, 0.25], [-0.25, -0.25]]);
    let pcm24 = decode(directory.path().join("pcm24.wav"));
    for (packed, exact) in pcm24.iter().flatten().zip(float.iter().flatten()) {
        assert!(
            (packed - exact).abs() <= 1.0 / 8_388_608.0,
            "{packed} not within one 24-bit lsb of {exact}"
        );
    }
    assert_eq!(
        fs::read(directory.path().join("pcm24.wav")).unwrap().len(),
        44 + 3 * 6
    );
    let stem = fs::read_dir(directory.path().join("stems"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .next()
        .unwrap();
    assert_eq!(decode(stem), float, "stems honour the selected format");
    let refused = run(
        &directory.0,
        &[
            "export-project",
            "project.hex",
            "bad.wav",
            "0",
            "3",
            "--format=pcm8",
        ],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("unknown export format"));
    assert!(!directory.path().join("bad.wav").exists());
}
